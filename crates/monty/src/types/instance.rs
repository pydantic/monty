use std::{borrow::Cow, fmt::Write, mem};

use super::{Dict, LazyHeapSet, PyTrait, Type};
use crate::{
    args::{ArgValues, KwargsValues},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, DropWithHeap, Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput,
        heap_read_ref_as_field_mut,
    },
    intern::Interns,
    resource::ResourceTracker,
    types::allocate_string,
    value::{EitherStr, Value},
};

/// An instance of a user-defined class.
///
/// Holds a reference to its [`Class`](super::Class) (whose `HeapId` is the type
/// identity used by `type()`/`isinstance`) and an `attrs` [`Dict`] — the instance
/// `__dict__`. Attribute reads fall through to the class namespace for methods and
/// class variables; attribute writes only ever touch `attrs`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Instance {
    /// The class this is an instance of (a `HeapData::Class`).
    class: HeapId,
    /// Instance attributes (`__dict__`).
    attrs: Dict,
}

impl Instance {
    /// Creates a new instance of `class` with the given initial attributes.
    #[must_use]
    pub fn new(class: HeapId, attrs: Dict) -> Self {
        Self { class, attrs }
    }

    /// Returns the `HeapId` of the instance's class object.
    #[must_use]
    pub fn class(&self) -> HeapId {
        self.class
    }

    /// Returns a reference to the instance's attribute dict (`__dict__`).
    #[must_use]
    pub fn attrs(&self) -> &Dict {
        &self.attrs
    }
}

/// A method bound to an instance, produced by `obj.method` (without calling it).
///
/// Calling a `BoundMethod` prepends `instance` to the argument list and invokes
/// `func`. The common `obj.method()` path skips this allocation by binding and
/// calling directly in [`Instance::py_call_attr`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct BoundMethod {
    /// The bound `self` (a `Value::Ref` to the instance).
    pub instance: Value,
    /// The underlying function (`DefFunction`/`Closure`/...).
    pub func: Value,
}

impl<'h> HeapRead<'h, Instance> {
    fn attrs_mut(&mut self) -> BorrowedHeapReadMut<'_, 'h, Dict> {
        heap_read_ref_as_field_mut!(self, Instance, attrs)
    }

    /// Sets an instance attribute, returning the previous value (if any) for the
    /// caller to drop. Takes ownership of both `name` and `value`.
    pub fn set_attr(
        &mut self,
        name: Value,
        value: Value,
        vm: &mut VM<'h, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        self.attrs_mut().set(name, value, vm)
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Instance> {
    fn py_type(&self, vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Instance(self.get(vm.heap).class)
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        // Identity equality, resolved by `Value::py_eq_impl` before reaching here.
        Ok(None)
    }

    fn py_hash(&self, self_id: HeapId, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        // Instances hash by identity (CPython's default for objects without `__hash__`).
        Ok(Some(identity_hash(self_id)))
    }

    /// Heap-level `repr` fallback.
    ///
    /// Real `repr()`/`str()` (including dispatch to a user `__repr__`/`__str__`)
    /// is handled at the `Value` level — see [`instance_repr`] / [`instance_str`] —
    /// because it needs the instance's `HeapId` to pass `self`, which this method
    /// does not receive. This produces a best-effort default and is essentially
    /// never reached.
    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        let class_id = self.get(vm.heap).class;
        Ok(write!(f, "<{} object>", class_name(class_id, vm.heap, vm.interns))?)
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);

        // 1. An instance attribute shadows class methods; call it as-is (unbound).
        if let Some(callable) = self
            .get(vm.heap)
            .attrs
            .get_by_str(attr_str, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap))
        {
            defer_drop!(callable, vm);
            return vm.call_function(callable, args);
        }

        // 2. A class member: bind `self` for methods, call data attributes as-is.
        let class_id = self.get(vm.heap).class;
        if let Some(member) = class_member(class_id, attr_str, vm) {
            defer_drop!(member, vm);
            return call_member_bound(member, self_id, args, vm);
        }

        // 3. `obj.__class__(...)` constructs a new instance — the callable form of
        // the `obj.__class__` special attribute (see `instance_getattr` step 3).
        // Checked after the dict/namespace lookups so a same-named member wins.
        // The class value is a fresh owned ref (inc_ref) dropped by the guard once
        // `call_function` has borrowed it; `instantiate_class` takes its own ref
        // for the new instance.
        if attr_str == "__class__" {
            vm.heap.inc_ref(class_id);
            let class_val = Value::Ref(class_id);
            defer_drop!(class_val, vm);
            return vm.call_function(class_val, args);
        }

        // 4. No such attribute.
        args.drop_with_heap(vm);
        Err(ExcType::attribute_error(
            class_name(class_id, vm.heap, vm.interns),
            attr_str,
        ))
    }

    fn py_is_context_manager(&self, vm: &VM<'h, impl ResourceTracker>) -> bool {
        // CPython names `__exit__` in the protocol TypeError, so that is the
        // dunder the `BeforeWith` gate checks; a class with `__exit__` but no
        // `__enter__` passes the gate and gets the "missed __enter__ method"
        // error from `py_enter` instead — matching CPython's check order.
        // Special-method lookup goes through the class only, never the
        // instance `__dict__` (CPython looks these up on the type).
        let class_id = self.get(vm.heap).class;
        match vm.heap.get(class_id) {
            HeapData::Class(class) => class.namespace().get_by_str("__exit__", vm.heap, vm.interns).is_some(),
            _ => false,
        }
    }

    fn py_enter(&mut self, self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<CallResult> {
        let class_id = self.get(vm.heap).class;
        let Some(enter) = class_member(class_id, "__enter__", vm) else {
            return Err(ExcType::type_error_not_context_manager(
                class_name(class_id, vm.heap, vm.interns),
                "__enter__",
            ));
        };
        defer_drop!(enter, vm);
        // A plain-function `__enter__` runs as a real pushed frame
        // (`CallResult::FramePushed`), so — unlike `__repr__`/`__str__` — it
        // can suspend on external/OS calls; the frame's return value becomes
        // the `as` target via the normal `ReturnValue` push.
        call_member_bound(enter, self_id, ArgValues::Empty, vm)
    }

    fn py_exit(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        exc: Option<HeapId>,
    ) -> RunResult<CallResult> {
        let class_id = self.get(vm.heap).class;
        let Some(exit) = class_member(class_id, "__exit__", vm) else {
            // Defensive tripwire — unreachable via `with`. `py_is_context_manager`
            // gates on `__exit__` being present at entry, and Monty has no `del`,
            // so the member cannot be removed mid-body. A reassignment (e.g. to a
            // non-callable like `None`) keeps the member present, so this branch
            // is not taken — that case fails later in `call_member_bound` as a
            // `TypeError: 'NoneType' object is not callable`.
            return Err(ExcType::attribute_error(
                class_name(class_id, vm.heap, vm.interns),
                "__exit__",
            ));
        };
        defer_drop!(exit, vm);
        // Build CPython's `(exc_type, exc_value, traceback)` triple. The type
        // is constructed as `Builtins::ExcType` — the same value the bare
        // exception name resolves to — so the idiomatic `if typ is ValueError:`
        // works inside a user `__exit__`. Monty has no traceback objects, so
        // the third slot is always `None` (see limitations/with.md).
        let (typ, val) = match exc {
            Some(exc_id) => {
                let HeapData::Exception(e) = vm.heap.get(exc_id) else {
                    // Instances only receive `Some(exc)` from `WithExceptStart`,
                    // which always passes the in-flight exception object
                    // (explicit `obj.__exit__(...)` calls go through normal
                    // method dispatch, never this trait hook).
                    unreachable!("Instance py_exit called with a non-exception heap id");
                };
                vm.heap.inc_ref(exc_id);
                (Value::Builtin(Builtins::ExcType(e.exc_type())), Value::Ref(exc_id))
            }
            None => (Value::None, Value::None),
        };
        let args = ArgValues::ArgsKargs {
            args: vec![typ, val, Value::None],
            kwargs: KwargsValues::Empty,
        };
        call_member_bound(exit, self_id, args, vm)
    }
}

impl HeapItem for Instance {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.attrs.py_estimate_size()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.class);
        self.attrs.py_dec_ref_ids(stack);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, BoundMethod> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        // Monty has no dedicated `method` type; bound methods report `function`.
        Type::Function
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, self_id: HeapId, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        // Bound methods hash by identity, consistent with their identity-only
        // equality (CPython hashes by `(instance, func)` — see limitations/classes.md).
        Ok(Some(identity_hash(self_id)))
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        Ok(write!(f, "<bound method>")?)
    }
}

impl HeapItem for BoundMethod {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.instance.py_dec_ref_ids(stack);
        self.func.py_dec_ref_ids(stack);
    }
}

/// Reads an instance attribute for `obj.attr` (the `LoadAttr` path).
///
/// Mirrors Python's lookup order: the instance `__dict__` first, then the class
/// namespace, then the `__class__` special case. A class method becomes a
/// [`BoundMethod`] (binding `self`); a class variable is returned as-is. A missing
/// attribute raises `AttributeError` with the real class name. Takes `self_id`
/// (available at the `Value` level) because binding a method needs the instance's
/// `HeapId`.
pub(crate) fn instance_getattr(
    self_id: HeapId,
    attr: &EitherStr,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<CallResult> {
    let attr_str = attr.as_str(vm.interns);

    // 1. Instance dict.
    if let HeapReadOutput::Instance(inst) = vm.heap.read(self_id)
        && let Some(value) = inst
            .get(vm.heap)
            .attrs
            .get_by_str(attr_str, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap))
    {
        return Ok(CallResult::Value(value));
    }

    // 2. Class namespace: bind methods, return class variables as-is.
    let class_id = instance_class(self_id, vm);
    if let Some(member) = class_member(class_id, attr_str, vm) {
        if is_method_value(&member, vm) {
            vm.heap.inc_ref(self_id);
            let bound = BoundMethod {
                instance: Value::Ref(self_id),
                func: member,
            };
            let id = vm.heap.allocate(HeapData::BoundMethod(bound))?;
            Ok(CallResult::Value(Value::Ref(id)))
        } else {
            Ok(CallResult::Value(member))
        }
    } else if attr_str == "__class__" {
        // 3. `obj.__class__` returns the class object itself (`obj.__class__ is Foo`).
        // Checked after the dict/namespace lookups so an explicit member of the
        // same name wins, mirroring the `__name__` handling on class objects.
        vm.heap.inc_ref(class_id);
        Ok(CallResult::Value(Value::Ref(class_id)))
    } else {
        Err(ExcType::attribute_error(
            class_name(class_id, vm.heap, vm.interns),
            attr_str,
        ))
    }
}

/// Produces `repr(instance)`, dispatching to a user `__repr__` if the class
/// defines one, otherwise the default `<ClassName object at 0x..>`.
pub(crate) fn instance_repr(self_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    match instance_call_str_dunder(self_id, "__repr__", vm)? {
        Some(s) => Ok(s),
        None => Ok(allocate_string(default_repr(self_id, vm), vm.heap)?),
    }
}

/// Produces `str(instance)`, dispatching to a user `__str__` if defined, else
/// falling back to `repr` (which itself falls back to the default).
pub(crate) fn instance_str(self_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    match instance_call_str_dunder(self_id, "__str__", vm)? {
        Some(s) => Ok(s),
        None => instance_repr(self_id, vm),
    }
}

/// Calls a user-defined string dunder (`__repr__`/`__str__`) on the instance and
/// validates that it returned a `str`.
///
/// Returns `Ok(None)` if the class does not define the dunder (caller uses the
/// default). The method runs to completion synchronously via `evaluate_function`,
/// so — unlike `__init__` — it cannot suspend on external/OS calls (see
/// `limitations/classes.md`). Recursion (e.g. a `__repr__` that reprs `self`)
/// re-enters the VM on the *Rust* stack; `evaluate_function`'s re-entry guard
/// bounds it with a catchable `RecursionError` — lower than CPython's depth for
/// deep-but-finite chains, a documented divergence (`limitations/classes.md`).
fn instance_call_str_dunder(
    self_id: HeapId,
    dunder: &'static str,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<Value>> {
    let class_id = instance_class(self_id, vm);
    let Some(func) = class_member(class_id, dunder, vm) else {
        return Ok(None);
    };
    defer_drop!(func, vm);
    // Only a plain function binds `self` as a descriptor (CPython's method
    // lookup protocol, mirrored by `call_member_bound`); an already-bound
    // method or other callable value is invoked with no extra argument.
    let args = if is_method_value(func, vm) {
        vm.heap.inc_ref(self_id);
        ArgValues::One(Value::Ref(self_id))
    } else {
        ArgValues::Empty
    };
    let result = vm.evaluate_function(dunder, func, args)?;
    // CPython requires `__repr__`/`__str__` to return a `str`; reject any other
    // type with the same TypeError, dropping the offending return value.
    if result.is_str(vm.heap) {
        Ok(Some(result))
    } else {
        let exc = ExcType::type_error(format!(
            "{dunder} returned non-string (type {})",
            result.py_type_name(vm)
        ));
        result.drop_with_heap(vm);
        Err(exc)
    }
}

/// The default `repr` for an instance with no user `__repr__`.
fn default_repr(self_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> String {
    let class_id = instance_class(self_id, vm);
    format!(
        "<{} object at 0x{:x}>",
        class_name(class_id, vm.heap, vm.interns),
        self_id.index()
    )
}

/// Returns the `HeapId` of `self_id`'s class object.
fn instance_class(self_id: HeapId, vm: &VM<'_, impl ResourceTracker>) -> HeapId {
    match vm.heap.get(self_id) {
        HeapData::Instance(inst) => inst.class,
        _ => unreachable!("instance_class called on non-instance heap value"),
    }
}

/// Looks up a member in a class namespace and clones it out, or `None` if absent.
fn class_member(class_id: HeapId, name: &str, vm: &VM<'_, impl ResourceTracker>) -> Option<Value> {
    match vm.heap.get(class_id) {
        HeapData::Class(class) => class
            .namespace()
            .get_by_str(name, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap)),
        _ => None,
    }
}

/// Returns a class object's name for error messages / repr.
///
/// Takes `heap` + `interns` rather than a `&VM` so heap-only contexts (e.g.
/// `Type::name`) can resolve names. The result borrows only the interner —
/// interned names are `Cow::Borrowed`, while heap-owned names (classes created
/// by the 3-arg `type()` form) are cloned into `Cow::Owned` here, so either way
/// the result survives subsequent heap mutation.
///
/// # Panics
/// If `class_id` does not refer to a `Class` heap entry — every producer of a
/// class id (`Instance.class`, class values) guarantees it does, so this is a
/// programmer-error tripwire.
pub(crate) fn class_name<'i>(
    class_id: HeapId,
    heap: &Heap<impl ResourceTracker>,
    interns: &'i Interns,
) -> Cow<'i, str> {
    match heap.get(class_id) {
        HeapData::Class(class) => match class.name() {
            EitherStr::Interned(id) => Cow::Borrowed(interns.get_str(*id)),
            EitherStr::Heap(s) => Cow::Owned(s.clone()),
        },
        _ => unreachable!("class_name called with a non-class heap id"),
    }
}

/// Calls a class member with CPython's descriptor-binding semantics: a
/// plain user-defined function binds `self` (prepended to `args`), while any
/// other callable value is called as-is. Shared by `py_call_attr` and the
/// context-manager hooks (`py_enter`/`py_exit`) so dunder invocation and
/// ordinary method calls dispatch identically.
fn call_member_bound(
    member: &Value,
    self_id: HeapId,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<CallResult> {
    if is_method_value(member, vm) {
        vm.heap.inc_ref(self_id);
        vm.call_function(member, args.prepend(Value::Ref(self_id)))
    } else {
        vm.call_function(member, args)
    }
}

/// Whether a value is a user-defined function (so it should bind `self` when
/// accessed as a method). Class variables that are not functions are returned
/// unbound.
fn is_method_value(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match value {
        Value::DefFunction(_) => true,
        Value::Ref(id) => matches!(vm.heap.get(*id), HeapData::Closure(_) | HeapData::FunctionDefaults(_)),
        _ => false,
    }
}
