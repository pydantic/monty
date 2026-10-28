//! The methods `@dataclass` writes into a class namespace, and the runtime
//! behind each of them.
//!
//! CPython's decorator generates `__init__`, `__repr__`, `__eq__`, `__hash__`
//! the ordering dunders and the frozen `__setattr__`/`__delattr__` as real
//! functions with the decoration's choices built in. A [`DataclassMethod`] is Monty's equivalent:
//! a callable holding a snapshot of what the class was decorated with, so
//! rebinding `__dataclass_fields__` or `__dataclass_params__` afterwards changes
//! nothing, as in CPython. The VM's dunder paths recognise these objects and call
//! the native code directly; explicit calls (`C.__eq__(a, b)`) go through `py_call`.

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    expressions::CmpOperator,
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::{Interns, StaticStrings, StringId},
    types::{
        CmpOrder, LazyHeapSet, PyTrait, TupleVec, Type, allocate_tuple,
        host_class::write_dataclass_repr,
        instance::{class_dunder, class_name, instance_attr},
        py_trait::PyObjectIdentity,
        str::allocate_string,
    },
    value::Value,
};

/// One method `@dataclass` generated, stored in the class namespace under the
/// dunder its [`MethodKind`] names.
///
/// **Owns heap references**: an `__init__` holds the defaults and factories it
/// was decorated with (see [`InitPlan`]), reported in `py_dec_ref_ids` and
/// `heap::for_each_child_id`. Reports itself as a `function`, as CPython's
/// generated methods are.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassMethod {
    /// The decorated class's name, fixed at decoration as CPython's generated
    /// `__qualname__` is; used in `repr()` and `__init__`'s argument errors.
    class_name: String,
    /// What the method does, with the decoration-time data it needs.
    kind: MethodKind,
}

/// The behaviour of one [`DataclassMethod`].
///
/// The field lists are the names `@dataclass` saw, in definition order; the
/// methods read each field as an attribute (`self.x`), as CPython's do.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum MethodKind {
    /// `__init__`: binds arguments to the fields, then runs `__post_init__`.
    Init(InitPlan),
    /// `__repr__`: `C(x=1, y=2)`.
    Repr(Vec<StringId>),
    /// `__eq__`: field-wise, against an instance of the same class only.
    Eq(Vec<StringId>),
    /// `__hash__` (`eq=True, frozen=True`, or `unsafe_hash=True`): the hash of
    /// the field tuple.
    Hash(Vec<StringId>),
    /// `frozen=True`'s `__setattr__`: refuses every assignment.
    FrozenSetattr,
    /// `frozen=True`'s `__delattr__`: refuses every deletion.
    FrozenDelattr,
    /// `order=True`'s `__lt__`: the field tuples compared with `<`.
    Lt(Vec<StringId>),
    /// `order=True`'s `__le__`.
    Le(Vec<StringId>),
    /// `order=True`'s `__gt__`.
    Gt(Vec<StringId>),
    /// `order=True`'s `__ge__`.
    Ge(Vec<StringId>),
}

impl MethodKind {
    /// Every parameter, `self` first, as the generated `def` declares them: an
    /// `__init__`'s positional fields, then its keyword-only ones.
    fn params<'a>(&self, interns: &'a Interns) -> Vec<Param<'a>> {
        let mut params = vec![Param::required(self.self_param(interns))];
        match self {
            Self::Init(plan) => params.extend(plan.param_order().into_iter().map(|idx| {
                let field = &plan.fields[idx];
                Param {
                    name: interns.get_str(field.name),
                    optional: field.init.is_optional(),
                    kw_only: field.kw_only,
                }
            })),
            Self::Repr(_) | Self::Hash(_) => {}
            Self::Eq(_) | Self::Lt(_) | Self::Le(_) | Self::Gt(_) | Self::Ge(_) => {
                params.push(Param::required("other"));
            }
            Self::FrozenSetattr => params.extend([Param::required("name"), Param::required("value")]),
            Self::FrozenDelattr => params.push(Param::required("name")),
        }
        params
    }

    /// The name of the `self` parameter: CPython's `__init__` renames it to
    /// `__dataclass_self__` when a field is called `self`.
    fn self_param(&self, interns: &Interns) -> &'static str {
        match self {
            Self::Init(plan) if plan.fields.iter().any(|field| interns.get_str(field.name) == "self") => {
                "__dataclass_self__"
            }
            _ => "self",
        }
    }

    /// The dunder this method is stored under, and named by.
    pub fn dunder(&self) -> StaticStrings {
        match self {
            Self::Init(_) => StaticStrings::DunderInit,
            Self::Repr(_) => StaticStrings::DunderRepr,
            Self::Eq(_) => StaticStrings::DunderEq,
            Self::Hash(_) => StaticStrings::DunderHash,
            Self::FrozenSetattr => StaticStrings::DunderSetattr,
            Self::FrozenDelattr => StaticStrings::DunderDelattr,
            Self::Lt(_) => StaticStrings::DunderLt,
            Self::Le(_) => StaticStrings::DunderLe,
            Self::Gt(_) => StaticStrings::DunderGt,
            Self::Ge(_) => StaticStrings::DunderGe,
        }
    }

    /// The field names a repr/eq/hash/ordering method reads, in order.
    fn field_names(&self) -> Option<&[StringId]> {
        match self {
            Self::Repr(names)
            | Self::Eq(names)
            | Self::Hash(names)
            | Self::Lt(names)
            | Self::Le(names)
            | Self::Gt(names)
            | Self::Ge(names) => Some(names),
            Self::Init(_) | Self::FrozenSetattr | Self::FrozenDelattr => None,
        }
    }

    /// The operator an ordering method applies to the field tuples.
    fn order_operator(&self) -> Option<CmpOperator> {
        match self {
            Self::Lt(_) => Some(CmpOperator::Lt),
            Self::Le(_) => Some(CmpOperator::LtE),
            Self::Gt(_) => Some(CmpOperator::Gt),
            Self::Ge(_) => Some(CmpOperator::GtE),
            _ => None,
        }
    }
}

/// One parameter of a generated method's signature.
#[derive(Debug, Clone, Copy)]
struct Param<'a> {
    name: &'a str,
    /// Whether the parameter has a default (or a factory standing in for one).
    optional: bool,
    /// Whether the parameter follows the `*` and can only be passed by keyword.
    kw_only: bool,
}

impl<'a> Param<'a> {
    /// A required positional-or-keyword parameter.
    fn required(name: &'a str) -> Self {
        Self {
            name,
            optional: false,
            kw_only: false,
        }
    }
}

/// What a generated `__init__` was decorated with.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct InitPlan {
    /// One entry per field the body handles, in definition order — the order
    /// the body stores them and `__post_init__` receives the `InitVar`s.
    fields: Vec<InitField>,
    /// Whether the class defined `__post_init__` when decorated. CPython's
    /// generated `__init__` calls `self.__post_init__()` only if so, looking the
    /// hook up at each call.
    post_init: bool,
    /// Whether the decoration was `frozen=True`. A frozen `__init__` writes
    /// through `object.__setattr__`; any other assigns as `self.x = x` does, so
    /// a later frozen re-decoration makes construction raise, as in CPython.
    frozen: bool,
}

impl InitPlan {
    /// The indices of the parameter fields in signature order: CPython's
    /// positional fields, then the keyword-only ones, each in definition order.
    fn param_order(&self) -> Vec<usize> {
        let params = || {
            self.fields
                .iter()
                .enumerate()
                .filter(|(_, field)| field.role != InitRole::Computed)
        };
        params()
            .filter(|(_, field)| !field.kw_only)
            .chain(params().filter(|(_, field)| field.kw_only))
            .map(|(idx, _)| idx)
            .collect()
    }
}

/// One field a generated `__init__` handles.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct InitField {
    name: StringId,
    /// The default or factory captured at decoration; [`FieldInit::Unsupplied`]
    /// for a required field.
    init: FieldInit,
    /// Whether the parameter is keyword-only.
    #[serde(default)]
    kw_only: bool,
    /// What the body does with the field.
    #[serde(default)]
    role: InitRole,
}

/// What a generated `__init__` does with one [`InitField`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum InitRole {
    /// An ordinary field: a parameter, stored on the instance.
    #[default]
    Stored,
    /// An `InitVar`: a parameter passed on to `__post_init__`, never stored.
    InitVar,
    /// An `init=False` field with a `default_factory`: not a parameter, but
    /// stored from a fresh factory call. Other `init=False` fields are not in
    /// the plan at all, since CPython's `__init__` never touches them.
    Computed,
}

impl InitField {
    /// A field for [`DataclassMethod::init`], taking ownership of `init`.
    pub fn new(name: StringId, init: FieldInit, kw_only: bool, role: InitRole) -> Self {
        Self {
            name,
            init,
            kw_only,
            role,
        }
    }
}

/// What a generated `__init__` supplies for a field the call left unbound.
///
/// Stored in the [`InitPlan`], and cloned per construction before any factory
/// runs: a factory re-enters the VM, where sandbox code can rebind `__init__`
/// and free the plan it came from.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum FieldInit {
    /// Nothing to supply — the field is required, or the call bound it.
    Unsupplied,
    /// The default captured at decoration.
    Default(Value),
    /// The `default_factory` callable, called once per construction.
    Factory(Value),
}

impl FieldInit {
    /// Whether the parameter is optional: a factory counts, as it does in
    /// CPython's generated signature.
    pub fn is_optional(&self) -> bool {
        !matches!(self, Self::Unsupplied)
    }

    /// An owned copy, taking a fresh reference to any captured value.
    fn clone_with_heap(&self, heap: &impl ContainsHeap) -> Self {
        match self {
            Self::Unsupplied => Self::Unsupplied,
            Self::Default(value) => Self::Default(value.clone_with_heap(heap)),
            Self::Factory(value) => Self::Factory(value.clone_with_heap(heap)),
        }
    }

    /// The captured value, if any.
    fn value(&self) -> Option<&Value> {
        match self {
            Self::Unsupplied => None,
            Self::Default(value) | Self::Factory(value) => Some(value),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for FieldInit {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Unsupplied => {}
            Self::Default(value) | Self::Factory(value) => value.drop_with(ctx),
        }
    }
}

impl DataclassMethod {
    /// A generated `__init__` taking ownership of the captured defaults in `fields`.
    pub fn init(class_name: String, fields: Vec<InitField>, post_init: bool, frozen: bool) -> Self {
        Self::new(
            class_name,
            MethodKind::Init(InitPlan {
                fields,
                post_init,
                frozen,
            }),
        )
    }

    /// Any other generated method.
    pub fn new(class_name: String, kind: MethodKind) -> Self {
        Self { class_name, kind }
    }

    /// Every captured value that is a heap reference, for the cycle collector:
    /// a default or factory can reach back to its class.
    pub fn ref_children(&self) -> impl Iterator<Item = HeapId> + '_ {
        let fields = match &self.kind {
            MethodKind::Init(plan) => plan.fields.as_slice(),
            _ => &[],
        };
        fields.iter().filter_map(|field| match field.init.value() {
            Some(Value::Ref(id)) => Some(*id),
            _ => None,
        })
    }

    /// The dunder this method is stored under.
    pub fn dunder(&self) -> StaticStrings {
        self.kind.dunder()
    }

    /// `C.__init__`, as CPython's generated `__qualname__` reads.
    fn qualname(&self) -> String {
        let dunder: &'static str = self.kind.dunder().into();
        format!("{}.{dunder}", self.class_name)
    }
}

/// Releases a method that never reached the heap.
impl<C: ContainsHeap> DropWithContext<C> for DataclassMethod {
    fn drop_with(self, ctx: &mut C) {
        if let MethodKind::Init(plan) = self.kind {
            for field in plan.fields {
                field.init.drop_with(ctx);
            }
        }
    }
}

impl HeapItem for DataclassMethod {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let MethodKind::Init(plan) = &mut self.kind {
            for field in &mut plan.fields {
                if let FieldInit::Default(value) | FieldInit::Factory(value) = &mut field.init {
                    value.py_dec_ref_ids(stack);
                }
            }
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, DataclassMethod> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Function
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // Functions compare by identity, which `Value::py_eq_impl` resolves first.
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let qualname = self.get(vm.heap).qualname();
        Ok(write!(
            f,
            "<function '{qualname}' at 0x{:x}>",
            self.py_identity().encoded()
        )?)
    }

    /// An explicit call, `self` first: `C.__eq__(a, b)` or a bound `a.__eq__(b)`.
    /// Implicit dispatch (`a == b`, `C(...)`) skips this for the direct paths below.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        call_method(self.id(), args, vm).map(CallResult::Value)
    }
}

/// Runs an explicitly called generated method.
///
/// Arguments bind as the generated `def` would — `self` first, by position or
/// keyword — with CPython's wording. `self` must then be an instance of a
/// sandbox class: CPython's methods accept any object with the right
/// attributes, a documented divergence.
fn call_method(method_id: HeapId, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let interns = vm.interns;
    let method = method_at(method_id, vm);
    let qualname = method.qualname();
    let dunder = method.dunder();
    let params = method.kind.params(interns);
    // Binding leaves every required slot filled, `self` among them; only an
    // `__init__` field with a default can still be empty.
    let order_op = method.kind.order_operator();
    let mut slots = bind_def(&qualname, &params, false, args, vm)?.into_iter();
    let this = slots.next().flatten().expect("binding fills the required self");
    let rest: Vec<Option<Value>> = slots.collect();
    defer_drop_mut!(rest, vm);
    defer_drop!(this, vm);
    let self_id = require_instance(this, &qualname, vm)?;
    match dunder {
        StaticStrings::DunderInit => {
            init_instance(method_id, self_id, mem::take(rest), vm)?;
            Ok(Value::None)
        }
        StaticStrings::DunderRepr => {
            let mut repr = String::new();
            let names = field_names(method_id, vm);
            dataclass_repr_fmt(self_id, &names, &mut repr, vm, &mut LazyHeapSet::default())?;
            Ok(allocate_string(repr, vm.heap))
        }
        StaticStrings::DunderEq => {
            let names = field_names(method_id, vm);
            let other = required(rest, 0).clone_with_heap(vm.heap);
            defer_drop!(other, vm);
            Ok(dataclass_eq(self_id, &names, other, vm)?.map_or(Value::NotImplemented, Value::Bool))
        }
        StaticStrings::DunderHash => {
            let names = field_names(method_id, vm);
            let hash = dataclass_hash(self_id, &names, vm)?;
            Ok(Value::Int(hash.raw().cast_signed()))
        }
        StaticStrings::DunderSetattr => Err(frozen_field_error("assign to", required(rest, 0), vm)?),
        StaticStrings::DunderDelattr => Err(frozen_field_error("delete", required(rest, 0), vm)?),
        _ => {
            let op = order_op.expect("the remaining generated methods are the ordering ones");
            let names = field_names(method_id, vm);
            let other = required(rest, 0).clone_with_heap(vm.heap);
            defer_drop!(other, vm);
            let Some((lhs, rhs)) = order_operands(self_id, &names, other, vm)? else {
                return Ok(Value::NotImplemented);
            };
            defer_drop!(lhs, vm);
            defer_drop!(rhs, vm);
            Ok(Value::Bool(vm.cmp_values(op, lhs, rhs)?))
        }
    }
}

/// The bound value of the required parameter at `idx` after `self`.
fn required(slots: &[Option<Value>], idx: usize) -> &Value {
    slots[idx].as_ref().expect("binding fills required parameters")
}

/// The `DataclassMethod` at `method_id`.
///
/// # Panics
/// If `method_id` is not one — callers reach here only through a method they
/// already matched.
fn method_at<'a>(method_id: HeapId, vm: &'a VM<'_>) -> &'a DataclassMethod {
    match vm.heap.get(method_id) {
        HeapData::DataclassMethod(method) => method,
        _ => unreachable!("dataclass method dispatch on another heap value"),
    }
}

/// A repr/eq/hash/ordering method's field names, cloned out so the heap is
/// free while the fields are read (which can run user `__eq__`/`__repr__`).
fn field_names(method_id: HeapId, vm: &VM<'_>) -> Vec<StringId> {
    method_at(method_id, vm)
        .kind
        .field_names()
        .map_or_else(Vec::new, <[_]>::to_vec)
}

/// The instance `self` names, or the `TypeError` Monty raises for any other `self`.
fn require_instance(this: &Value, qualname: &str, vm: &VM<'_>) -> RunResult<HeapId> {
    match this {
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Instance(_)) => Ok(*id),
        _ => Err(ExcType::type_error(format!(
            "{qualname}() requires a class instance as self, not '{}'",
            this.py_type_name(vm)
        ))),
    }
}

/// The generated method `class_id` stores under `dunder`, if that member is one
/// of the matching kind — the test the VM's dunder paths make before taking
/// the direct route.
pub(crate) fn generated_method(class_id: HeapId, dunder: &str, vm: &VM<'_>) -> Option<HeapId> {
    match class_dunder(class_id, dunder, vm) {
        Some(Value::Ref(id)) => match vm.heap.get(*id) {
            HeapData::DataclassMethod(method) if <&'static str>::from(method.kind.dunder()) == dunder => Some(*id),
            _ => None,
        },
        _ => None,
    }
}

/// [`generated_method`] for a repr/eq/hash/ordering dunder, returning its field
/// names.
pub(crate) fn generated_fields(class_id: HeapId, dunder: &str, vm: &VM<'_>) -> Option<Vec<StringId>> {
    generated_method(class_id, dunder, vm).map(|id| field_names(id, vm))
}

/// Whether `value` is a generated `__init__`, which construction calls directly.
pub(crate) fn is_generated_init(value: &Value, vm: &VM<'_>) -> bool {
    matches!(value, Value::Ref(id) if matches!(
        vm.heap.get(*id),
        HeapData::DataclassMethod(method) if matches!(method.kind, MethodKind::Init(_))
    ))
}

/// The error assigning `name` raises on an instance of `class_id`, or `None`
/// when the write may proceed: CPython's frozen `__setattr__`, which refuses
/// every attribute, declared field or not.
///
/// Every instance assignment passes through here, so it is one namespace lookup
/// for a plain class.
pub(crate) fn frozen_assignment_error(class_id: HeapId, name: &Value, vm: &mut VM<'_>) -> Option<RunError> {
    generated_method(class_id, "__setattr__", vm)?;
    Some(match frozen_field_error("assign to", name, vm) {
        Ok(error) | Err(error) => error,
    })
}

/// `FrozenInstanceError: cannot {action} field {name!r}`, as CPython's frozen
/// `__setattr__`/`__delattr__` raise it. `Err` only when `repr(name)` raised.
fn frozen_field_error(action: &str, name: &Value, vm: &mut VM<'_>) -> RunResult<RunError> {
    let name = name.py_repr(vm)?;
    defer_drop!(name, vm);
    let message = format!("cannot {action} field {}", name.to_str(vm)?);
    Ok(SimpleException::new_msg(ExcType::FrozenInstanceError, message).into())
}

/// Constructs an instance with a generated `__init__`, taking ownership of the
/// freshly allocated `instance` and handing it back on success.
///
/// `init` is the caller's reference to the method, which keeps its plan alive
/// while factories and `__post_init__` re-enter the VM.
pub(crate) fn run_init(vm: &mut VM<'_>, init: &Value, instance: Value, args: ArgValues) -> RunResult<Value> {
    let &Value::Ref(method_id) = init else {
        unreachable!("is_generated_init matched a heap method")
    };
    let mut guard = DropGuard::new(instance, vm);
    let (instance, vm) = guard.as_parts();
    let &Value::Ref(instance_id) = instance else {
        unreachable!("the caller just allocated the instance")
    };
    let method = method_at(method_id, vm);
    let qualname = method.qualname();
    let params = method.kind.params(vm.interns);
    // `self` is the new instance, already bound; its slot holds a placeholder.
    let mut slots = bind_def(&qualname, &params, true, args, vm)?.into_iter();
    slots.next().flatten().drop_with(vm);
    init_instance(method_id, instance_id, slots.collect(), vm)?;
    Ok(guard.into_inner())
}

/// The body of a generated `__init__`, given its bound arguments in signature
/// order: supplies the defaults and factories for the unbound fields, stores
/// one attribute per stored field, then calls `__post_init__` with the
/// `InitVar`s if the class had one when decorated.
///
/// Runs through [`VM::evaluate_function`] for factories and the hook, so neither
/// can suspend on an external or OS call — see `limitations/dataclasses.md`.
fn init_instance(method_id: HeapId, instance_id: HeapId, params: Vec<Option<Value>>, vm: &mut VM<'_>) -> RunResult<()> {
    let MethodKind::Init(plan) = &method_at(method_id, vm).kind else {
        unreachable!("init_instance on a non-__init__ method")
    };
    let (post_init, frozen) = (plan.post_init, plan.frozen);
    let fields: Vec<(StringId, InitRole)> = plan.fields.iter().map(|field| (field.name, field.role)).collect();
    // Back into definition order, which is how the body walks the fields; a
    // `Computed` field has no parameter and starts unbound.
    let mut slots: Vec<Option<Value>> = (0..fields.len()).map(|_| None).collect();
    for (field_idx, value) in plan.param_order().into_iter().zip(params) {
        slots[field_idx] = value;
    }
    let mut guard = DropGuard::new(slots, vm);
    let (slots, vm) = guard.as_parts_mut();
    fill_unbound_fields(method_id, slots, vm)?;
    let (slots, vm) = guard.into_parts();
    let mut stored = Vec::with_capacity(fields.len());
    let mut init_vars = Vec::new();
    for ((name, role), value) in fields.into_iter().zip(slots) {
        let value = value.expect("every field is bound or filled");
        match role {
            InitRole::InitVar => init_vars.push(value),
            InitRole::Stored | InitRole::Computed => stored.push((name, value)),
        }
    }
    let mut init_vars = DropGuard::new(init_vars, vm);
    let (_, vm) = init_vars.as_parts_mut();
    store_fields(instance_id, stored, frozen, vm)?;
    let (init_vars, vm) = init_vars.into_parts();
    if post_init {
        run_post_init(instance_id, init_vars, vm)
    } else {
        // CPython's generated `__init__` accepts them and lets them go.
        init_vars.drop_with(vm);
        Ok(())
    }
}

/// Fills each unbound slot from the field's captured default, or by calling its
/// `default_factory`, in field order.
///
/// Everything needed is cloned out of the plan before a single factory runs:
/// past that point nothing reads the method, so a factory that rebinds
/// `__init__` cannot leave the loop reading a freed plan. Each entry is taken
/// as it is used, so an error part-way leaves the guard only those to come.
fn fill_unbound_fields(method_id: HeapId, slots: &mut [Option<Value>], vm: &mut VM<'_>) -> RunResult<()> {
    let plan = snapshot_field_inits(method_id, slots, vm);
    defer_drop_mut!(plan, vm);
    for idx in 0..plan.len() {
        // Each factory re-enters `run()` with a fresh dispatch countdown, so a
        // short one reaches no checkpoint: this is the construction's only clock
        // poll, as it is for the sorting key loop.
        vm.heap.tracker.check_time_every(idx)?;
        if slots[idx].is_some() {
            continue;
        }
        slots[idx] = Some(match mem::replace(&mut plan[idx], FieldInit::Unsupplied) {
            FieldInit::Default(default) => default,
            FieldInit::Factory(factory) => {
                defer_drop!(factory, vm);
                vm.evaluate_function(factory, ArgValues::Empty)?
            }
            FieldInit::Unsupplied => unreachable!("binding rejected every unbound required field"),
        });
    }
    Ok(())
}

/// Clones every unbound field's default or factory out of the plan, in field
/// order. A slot the call already bound is skipped, so a construction that
/// supplies every argument clones nothing.
fn snapshot_field_inits(method_id: HeapId, slots: &[Option<Value>], vm: &VM<'_>) -> Vec<FieldInit> {
    let MethodKind::Init(plan) = &method_at(method_id, vm).kind else {
        unreachable!("snapshot_field_inits on a non-__init__ method")
    };
    slots
        .iter()
        .zip(&plan.fields)
        .map(|(bound, field)| match bound {
            Some(_) => FieldInit::Unsupplied,
            None => field.init.clone_with_heap(vm.heap),
        })
        .collect()
}

/// Calls `self.__post_init__(*init_vars)`, as CPython's generated `__init__`
/// ends by doing, passing the `InitVar`s positionally in field order. A hook
/// removed since decoration raises `AttributeError`; its return value is
/// discarded.
fn run_post_init(instance_id: HeapId, init_vars: Vec<Value>, vm: &mut VM<'_>) -> RunResult<()> {
    let mut args = DropGuard::new(init_vars, vm);
    let (_, vm) = args.as_parts_mut();
    let Some(post_init) = instance_attr(instance_id, "__post_init__", vm) else {
        let class_name = class_name(instance_class(instance_id, vm), vm.heap, vm.interns).into_owned();
        return Err(ExcType::attribute_error(&class_name, "__post_init__"));
    };
    let (init_vars, vm) = args.into_parts();
    defer_drop!(post_init, vm);
    let args = if init_vars.is_empty() {
        ArgValues::Empty
    } else {
        ArgValues::ArgsKargs {
            args: init_vars,
            kwargs: KwargsValues::Empty,
        }
    };
    let result = vm.evaluate_function(post_init, args)?;
    result.drop_with(vm);
    Ok(())
}

/// Stores each `(name, value)` on the instance, consuming `fields`.
///
/// A frozen `__init__` writes straight to `__dict__` (CPython's
/// `object.__setattr__`); any other goes through the class's assignment check,
/// so a frozen `__setattr__` installed by a later decoration refuses it.
fn store_fields(instance_id: HeapId, fields: Vec<(StringId, Value)>, frozen: bool, vm: &mut VM<'_>) -> RunResult<()> {
    let (names, values): (Vec<StringId>, Vec<Value>) = fields.into_iter().unzip();
    let values = values.into_iter();
    defer_drop_mut!(values, vm);
    let HeapReadOutput::Instance(instance) = vm.heap.read(instance_id) else {
        unreachable!("init_instance runs on an instance")
    };
    for (name, value) in names.into_iter().zip(values.by_ref()) {
        let name = Value::InternString(name);
        let replaced = if frozen {
            instance.set_attr_unchecked(name, value, vm)?
        } else {
            instance.set_attr(name, value, vm)?
        };
        replaced.drop_with(vm);
    }
    Ok(())
}

/// Binds `args` to `params` as a generated `def` would, returning one slot per
/// parameter: every required one filled, an optional one left `None` if unbound.
///
/// `self_bound` marks `self` (the first parameter) as bound before the call, as
/// a constructor binds the new instance: its slot holds a `None` placeholder,
/// and CPython's arity messages and `self=` duplicate error count it. Errors
/// follow CPython's order: keywords first, then too many positionals, then
/// missing positional, then missing keyword-only ones. Bound arguments are
/// guarded, so every error releases them.
///
/// Every key is a string here: `**` unpacking is the only way a non-string one
/// could arrive, and `DictMerge` raises `keywords must be strings` while
/// building the call's kwargs — before any binder runs, as CPython does.
///
/// TODO(dataclass-keywords): a third binder alongside `args::bind_native` and
/// `args::bind_python`, all three reproducing CPython's error wording and able
/// to drift. Fold into `Signature::bind` — not a drop-in, as that needs a
/// static name and spec where this binds a signature known only at decoration.
fn bind_def(
    qualname: &str,
    params: &[Param<'_>],
    self_bound: bool,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<Vec<Option<Value>>> {
    // Keyword-only parameters always follow the positional ones.
    let n_positional = params.iter().take_while(|param| !param.kw_only).count();
    let (pos_iter, kwargs) = args.into_parts();
    let mut slots: Vec<Option<Value>> = (0..params.len()).map(|_| None).collect();
    let first = usize::from(self_bound);
    if self_bound {
        slots[0] = Some(Value::None);
    }
    let mut guard = DropGuard::new(slots, vm);
    let (slots, vm) = guard.as_parts_mut();

    // Positionals fill the leftmost free slots; one beyond the last positional
    // parameter is dropped here and only counted, since CPython binds keywords
    // first: `P(1, 2, 3, y=4)` is "multiple values for argument 'y'", not an
    // arity error.
    let mut n_pos = first;
    for (slot, value) in (first..).zip(pos_iter) {
        n_pos += 1;
        if slot < n_positional {
            slots[slot] = Some(value);
        } else {
            value.drop_with(vm);
        }
    }

    bind_keyword_args(slots, params, kwargs, qualname, vm)?;

    let (positional, keyword) = params.split_at(n_positional);
    if n_pos > n_positional {
        let required = positional.iter().filter(|param| !param.optional).count();
        let kwonly_given = slots[n_positional..].iter().filter(|slot| slot.is_some()).count();
        return Err(ExcType::type_error_too_many_positional_range(
            qualname,
            required,
            n_positional,
            n_pos,
            kwonly_given,
        ));
    }

    let missing_positional = missing_names(positional, &slots[..n_positional]);
    if !missing_positional.is_empty() {
        return Err(ExcType::type_error_missing_positional_with_names(
            qualname,
            &missing_positional,
        ));
    }
    let missing_keyword = missing_names(keyword, &slots[n_positional..]);
    if !missing_keyword.is_empty() {
        return Err(ExcType::type_error_missing_kwonly_with_names(
            qualname,
            &missing_keyword,
        ));
    }
    Ok(guard.into_inner())
}

/// The required parameters among `params` whose slot the call left unbound.
fn missing_names<'a>(params: &[Param<'a>], slots: &[Option<Value>]) -> Vec<&'a str> {
    params
        .iter()
        .zip(slots)
        .filter(|(param, slot)| !param.optional && slot.is_none())
        .map(|(param, _)| param.name)
        .collect()
}

/// Fills the named slots from the keyword arguments.
///
/// The iterator is guarded, so an error mid-mapping raises on the spot and the
/// pairs it never reached are still released. Keys are known to be strings —
/// the caller rejects a non-string one before any binding happens.
fn bind_keyword_args(
    slots: &mut [Option<Value>],
    params: &[Param<'_>],
    kwargs: KwargsValues,
    qualname: &str,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, vm);
    for (key, value) in kwargs.by_ref() {
        defer_drop!(key, vm);
        let name = key
            .as_either_str(vm.heap)
            .expect("DictMerge rejects non-string keys before the call")
            .as_str(vm.interns)
            .to_owned();
        // Resolved before the value is guarded, which borrows the VM.
        let slot = params.iter().position(|param| param.name == name);
        let value = DropGuard::new(value, vm);
        match slot {
            Some(idx) if slots[idx].is_none() => slots[idx] = Some(value.into_inner()),
            Some(_) => return Err(ExcType::type_error_duplicate_arg(qualname, &name)),
            None => return Err(ExcType::type_error_unexpected_keyword(qualname, &name)),
        }
    }
    Ok(())
}

/// Field-wise `__eq__`: equal only to an instance of the *same* class with equal
/// fields. Any other operand is `NotImplemented` (`Ok(None)`).
///
/// Mirrors CPython 3.14's generated `__eq__`: identity first, then
/// `self.a == other.a and ...` left to right, stopping at the first unequal
/// pair, each compared with the `==` *operator* rather than container equality.
pub(crate) fn dataclass_eq(
    self_id: HeapId,
    field_names: &[StringId],
    other: &Value,
    vm: &mut VM<'_>,
) -> RunResult<Option<bool>> {
    let &Value::Ref(other_id) = other else {
        return Ok(None);
    };
    if other_id == self_id {
        return Ok(Some(true));
    }
    let class_id = instance_class(self_id, vm);
    if !matches!(vm.heap.get(other_id), HeapData::Instance(inst) if inst.class() == class_id) {
        return Ok(None);
    }
    // Charge a recursion level: two distinct cyclic dataclasses (`a.x = a;
    // b.x = b; a == b`) re-enter here per level and would otherwise overflow
    // the host stack.
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    for name_id in field_names {
        let field_name = vm.interns.get_str(*name_id).to_owned();
        // Both reads are guarded so a failing comparison below (or an early
        // return) cannot strand either value.
        let a = instance_attr(self_id, &field_name, vm);
        defer_drop!(a, vm);
        let b = instance_attr(other_id, &field_name, vm);
        defer_drop!(b, vm);
        match (a, b) {
            (Some(a), Some(b)) if !a.py_eq_operator(b, vm)? => return Ok(Some(false)),
            (Some(_), Some(_)) => {}
            // The generated `__eq__` reads each field as an attribute, so one
            // that resolves nowhere raises rather than comparing unequal.
            _ => {
                let class_name = class_name(class_id, vm.heap, vm.interns).into_owned();
                return Err(ExcType::attribute_error(&class_name, &field_name));
            }
        }
    }
    Ok(Some(true))
}

/// Three-way comparison for `order=True`, as `sorted()`, `min()` and the
/// operators see it: `None` when `other` is not an instance of the same class,
/// which is CPython's `NotImplemented`.
///
/// CPython's generated `__lt__` compares `(self.a, ...)` with `(other.a, ...)`,
/// so the field values are gathered into two tuples and compared as tuples are.
pub(crate) fn dataclass_cmp(
    self_id: HeapId,
    field_names: &[StringId],
    other: &Value,
    vm: &mut VM<'_>,
) -> RunResult<Option<CmpOrder>> {
    let Some((lhs, rhs)) = order_operands(self_id, field_names, other, vm)? else {
        return Ok(None);
    };
    defer_drop!(lhs, vm);
    defer_drop!(rhs, vm);
    lhs.py_cmp(rhs, vm).map(Some)
}

/// The two field tuples an ordering method compares, or `None` when `other` is
/// not an instance of `self`'s class.
///
/// Charges a recursion level, as `dataclass_eq` does: a field holding another
/// ordered instance re-enters here per level.
fn order_operands(
    self_id: HeapId,
    field_names: &[StringId],
    other: &Value,
    vm: &mut VM<'_>,
) -> RunResult<Option<(Value, Value)>> {
    let class_id = instance_class(self_id, vm);
    let &Value::Ref(other_id) = other else {
        return Ok(None);
    };
    if !matches!(vm.heap.get(other_id), HeapData::Instance(inst) if inst.class() == class_id) {
        return Ok(None);
    }
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    let lhs = field_tuple(self_id, class_id, field_names, vm)?;
    let mut lhs = DropGuard::new(lhs, vm);
    let (_, vm) = lhs.as_parts_mut();
    let rhs = field_tuple(other_id, class_id, field_names, vm)?;
    Ok(Some((lhs.into_inner(), rhs)))
}

/// `(self.a, self.b, ...)` for `field_names`, raising `AttributeError` for a
/// field the instance does not have.
fn field_tuple(self_id: HeapId, class_id: HeapId, field_names: &[StringId], vm: &mut VM<'_>) -> RunResult<Value> {
    let mut items = DropGuard::new(TupleVec::with_capacity(field_names.len()), vm);
    for name_id in field_names {
        let (items, vm) = items.as_parts_mut();
        let interns = vm.interns;
        let field_name = interns.get_str(*name_id);
        let Some(value) = instance_attr(self_id, field_name, vm) else {
            let class_name = class_name(class_id, vm.heap, vm.interns).into_owned();
            return Err(ExcType::attribute_error(&class_name, field_name));
        };
        items.push(value);
    }
    let (items, vm) = items.into_parts();
    Ok(allocate_tuple(items, vm.heap))
}

/// Generated `__hash__` for `eq=True, frozen=True`: the hash of the field values
/// as a tuple, matching CPython's `hash((self.f1, ...))`.
///
/// An unhashable field raises naming *its* type, as hashing that tuple would;
/// fields are read as attributes, so an uninitialised one raises too.
pub(crate) fn dataclass_hash(self_id: HeapId, field_names: &[StringId], vm: &mut VM<'_>) -> RunResult<HashValue> {
    let class_id = instance_class(self_id, vm);
    // Charge a recursion level, as `dataclass_eq` does: instances can cycle
    // through a field, which would otherwise re-enter here per level.
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    // Same walk as `Tuple::py_hash`, so `hash(Point(1, 2)) == hash((1, 2))`.
    let mut hasher = DefaultHasher::new();
    for name_id in field_names {
        let field_name = vm.interns.get_str(*name_id).to_owned();
        let value = instance_attr(self_id, &field_name, vm);
        defer_drop!(value, vm);
        let Some(value) = value else {
            let class_name = class_name(class_id, vm.heap, vm.interns).into_owned();
            return Err(ExcType::attribute_error(&class_name, &field_name));
        };
        match value.py_hash(vm)? {
            Some(h) => h.hash(&mut hasher),
            None => return Err(ExcType::type_error_unhashable(&value.py_type_name(vm))),
        }
    }
    Ok(HashValue::new(hasher.finish()))
}

/// Generated `__repr__`: `ClassName(f1=v1, ...)`, with an instance already being
/// printed rendered as `...` as CPython's `recursive_repr` wrapper does.
/// Formatting is shared with the host-supplied `Dataclass` via
/// [`write_dataclass_repr`] so the two cannot drift.
pub(crate) fn dataclass_repr_fmt(
    self_id: HeapId,
    field_names: &[StringId],
    f: &mut impl Write,
    vm: &mut VM<'_>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    if heap_ids.contains(&self_id) {
        return Ok(f.write_str("...")?);
    }
    let class_id = instance_class(self_id, vm);
    let name = class_name(class_id, vm.heap, vm.interns).to_string();
    heap_ids.insert(self_id);
    // Each field is read as an attribute at the point it is written, so a field
    // whose `__repr__` mutates a later one is observed, exactly as CPython's
    // generated f-string does. A field that resolves nowhere raises.
    let result = write_dataclass_repr(f, &name, field_names.len(), vm, heap_ids, |i, vm| {
        let field_name = vm.interns.get_str(field_names[i]).to_owned();
        match instance_attr(self_id, &field_name, vm) {
            Some(value) => Ok((field_name, Some(value))),
            None => Err(ExcType::attribute_error(&name, &field_name)),
        }
    });
    heap_ids.remove(&self_id);
    result
}

/// Returns the `HeapId` of an instance's class object.
///
/// # Panics
/// If `self_id` is not an `Instance` — callers check, or reached here through
/// a dunder lookup on one.
fn instance_class(self_id: HeapId, vm: &VM<'_>) -> HeapId {
    match vm.heap.get(self_id) {
        HeapData::Instance(inst) => inst.class(),
        _ => unreachable!("dataclass dispatch on a non-instance"),
    }
}
