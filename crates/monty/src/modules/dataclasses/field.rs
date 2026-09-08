//! The `dataclasses.Field` objects making up a class's `__dataclass_fields__`:
//! the type itself, the `field()` constructor, and the accessors that decode one
//! back out of the heap. The decorator that builds and adopts them lives in the
//! parent module.

use std::fmt::Write;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::{StaticStrings, StringId},
    types::{LazyHeapSet, PyTrait, Type, str::allocate_string},
    value::{EitherStr, Marker, Value},
};

/// One entry of a class's `__dataclass_fields__`: CPython's `dataclasses.Field`.
///
/// Also what `dataclasses.field(...)` returns, as in CPython — the decorator
/// fills in the `name` and `annotation` of the very object the class body bound
/// and stores it, so `C.__dataclass_fields__['x'] is f` holds. Until then both
/// are `None`, which is what an un-adopted `field()` result reports.
///
/// **Owns heap references** (`annotation`, `default`, `default_factory`),
/// reported in `py_dec_ref_ids` below and `heap::for_each_child_id` — a default
/// or a factory can reach back to its class, closing a cycle. Only the
/// attributes Monty can model are stored; the rest are constants or refused
/// (see `py_getattr` below).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassField {
    /// The interned field name (from an `__annotations__` key, always interned),
    /// or `None` before a decorator adopts the field.
    name: Option<StringId>,
    /// `Field.type`: the annotation as source text, never evaluated. `None`
    /// until adopted, alongside `name`.
    annotation: Option<Value>,
    /// The default **captured when `@dataclass` ran**, or `None` for a required
    /// field — rebinding the class attribute afterwards must not change it.
    default: Option<Value>,
    /// `field(default_factory=...)`: a callable invoked once per construction to
    /// build this field's default. Mutually exclusive with `default`.
    default_factory: Option<Value>,
}

impl DataclassField {
    /// Builds an adopted field, taking ownership of `annotation` and `default`.
    ///
    /// For a plain `x: int = 5`, where no `field()` object exists at all.
    #[must_use]
    pub fn new(name: StringId, annotation: Value, default: Option<Value>) -> Self {
        Self {
            name: Some(name),
            annotation: Some(annotation),
            default,
            default_factory: None,
        }
    }

    /// Builds the un-adopted field `field(...)` returns, owning `default` and
    /// `default_factory`. The decorator supplies the name and annotation later.
    #[must_use]
    pub fn new_spec(default: Option<Value>, default_factory: Option<Value>) -> Self {
        Self {
            name: None,
            annotation: None,
            default,
            default_factory,
        }
    }

    /// The interned field name, as the synthesized `__init__` binds it, or
    /// `None` for a `field()` result no decorator has adopted.
    #[must_use]
    pub fn name(&self) -> Option<StringId> {
        self.name
    }

    /// `Field.type`, borrowed — the annotation the class was defined with, or
    /// `None` for a `field()` result no decorator has adopted.
    #[must_use]
    pub fn annotation(&self) -> Option<&Value> {
        self.annotation.as_ref()
    }

    /// The captured default, or `None` for a required field.
    #[must_use]
    pub fn default(&self) -> Option<&Value> {
        self.default.as_ref()
    }

    /// The captured `default_factory`, or `None` when the field has none.
    #[must_use]
    pub fn default_factory(&self) -> Option<&Value> {
        self.default_factory.as_ref()
    }

    /// Whether the field is optional in the synthesized `__init__`: it has
    /// either a captured default or a factory to call.
    #[must_use]
    pub fn has_default(&self) -> bool {
        self.default.is_some() || self.default_factory.is_some()
    }

    /// Adopts a `field()` result into a class, taking ownership of `annotation`.
    ///
    /// Mirrors CPython, which sets `f.name`/`f.type` on the object the class
    /// body bound rather than building a new one. One `field()` object can be
    /// adopted more than once — bound under two names, or by two classes — so
    /// the annotation it already held is returned for the caller to release.
    #[must_use]
    pub fn adopt(&mut self, name: StringId, annotation: Value) -> Option<Value> {
        self.name = Some(name);
        self.annotation.replace(annotation)
    }

    /// Every stored value that is a heap reference: the field's children, for
    /// the cycle collector's walk.
    pub fn ref_children(&self) -> impl Iterator<Item = HeapId> + '_ {
        [&self.annotation, &self.default, &self.default_factory]
            .into_iter()
            .flatten()
            .filter_map(|value| match value {
                Value::Ref(id) => Some(*id),
                _ => None,
            })
    }
}

/// Releases a field that never reached the heap — one the decorator collected
/// before rejecting the class.
impl<C: ContainsHeap> DropWithContext<C> for DataclassField {
    fn drop_with(self, ctx: &mut C) {
        self.annotation.drop_with(ctx);
        self.default.drop_with(ctx);
        self.default_factory.drop_with(ctx);
    }
}

/// `dataclasses.field(...)`: builds the [`DataclassField`] a class body binds,
/// which `@dataclass` then adopts by filling in its name and annotation.
///
/// Legal outside a class body too (`f = field(default=1)`), which is why it is
/// a value rather than a decoration-time form.
pub(super) fn field(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FieldArgs {
        default,
        default_factory,
        init,
        repr,
        compare,
        hash,
        metadata,
        kw_only,
    } = FieldArgs::from_args(args, vm)?;
    // Arguments Monty does not implement are refused when set away from their
    // CPython default, as `@dataclass(...)` refuses its own. That includes the
    // three flags: nothing consults them when the dunders are synthesized, so
    // accepting `init=False` would silently give the field an `__init__`
    // parameter anyway. Listed in signature order, which is the order CPython
    // would hit them.
    // Truthiness runs user code — a `__bool__` may raise — so the flags are
    // guarded before the first read.
    let mut guard = DropGuard::new([init, repr, hash, compare, metadata, kw_only], vm);
    let (flags, vm) = guard.as_parts();
    let [init, repr, hash, compare, metadata, kw_only] = flags;
    let unimplemented = [
        ("init", !init.py_bool(vm)?),
        ("repr", !repr.py_bool(vm)?),
        ("hash", !matches!(hash, Value::None)),
        ("compare", !compare.py_bool(vm)?),
        ("metadata", !matches!(metadata, Value::None)),
        ("kw_only", !is_missing(kw_only)),
    ]
    .into_iter()
    .find(|&(_, given)| given);
    let (flags, vm) = guard.into_parts();
    flags.drop_with(vm);

    // `MISSING` means "not given", since `None` is a legitimate default.
    let spec = DataclassField::new_spec(
        (!is_missing(&default)).then_some(default),
        (!is_missing(&default_factory)).then_some(default_factory),
    );
    let mut guard = DropGuard::new(spec, vm);
    let (spec, _) = guard.as_parts();
    // Both given is CPython's own error, so it outranks the Monty-only refusals.
    if spec.default().is_some() && spec.default_factory().is_some() {
        return Err(ExcType::value_error("cannot specify both default and default_factory"));
    }
    if let Some((name, _)) = unimplemented {
        return Err(ExcType::not_implemented(format!("field() does not yet support the {name} argument")).into());
    }
    let (spec, vm) = guard.into_parts();
    Ok(vm.heap.allocate_as(spec).into_value())
}

/// Whether an argument was left at the `MISSING` sentinel `field()` defaults to.
fn is_missing(value: &Value) -> bool {
    matches!(value, Value::Marker(Marker(StaticStrings::Missing)))
}

/// `field()` arguments, mirroring CPython's all-keyword-only signature.
/// `MISSING` is the "not given" sentinel, since `None` is a legitimate default.
#[derive(FromArgs)]
#[from_args(name = "field", style = def)]
struct FieldArgs {
    #[from_args(kw_only, default = Value::Marker(Marker(StaticStrings::Missing)))]
    default: Value,
    #[from_args(kw_only, default = Value::Marker(Marker(StaticStrings::Missing)))]
    default_factory: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    init: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    repr: Value,
    #[from_args(kw_only, default = Value::None)]
    hash: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    compare: Value,
    #[from_args(kw_only, default = Value::None)]
    metadata: Value,
    #[from_args(kw_only, default = Value::Marker(Marker(StaticStrings::Missing)))]
    kw_only: Value,
}

/// The `Field` a heap id points at, if it is one.
pub(super) fn field_at_id<'a>(vm: &'a VM<'_>, field_id: HeapId) -> Option<&'a DataclassField> {
    match vm.heap.get(field_id) {
        HeapData::DataclassField(field) => Some(field),
        _ => None,
    }
}

/// The heap `Field` a class-body value is, when the body bound a `field()`
/// result the decorator should adopt rather than treat as a plain default.
pub(super) fn adoptable_field(bound: Option<&Value>, vm: &VM<'_>) -> Option<HeapId> {
    match bound {
        Some(Value::Ref(id)) if matches!(vm.heap.get(*id), HeapData::DataclassField(_)) => Some(*id),
        _ => None,
    }
}

/// The `Field` at `idx` in a `__dataclass_fields__` dict, in definition order.
pub(super) fn field_at<'a>(vm: &'a VM<'_>, fields_id: HeapId, idx: usize) -> Option<&'a DataclassField> {
    let HeapData::Dict(fields) = vm.heap.get(fields_id) else {
        return None;
    };
    match fields.value_at(idx) {
        Some(Value::Ref(id)) => field_at_id(vm, *id),
        _ => None,
    }
}

/// `Field` attributes CPython has but Monty has no object for, paired with what
/// is missing. Refused rather than faked.
const UNMODELLED_ATTRS: [(&str, &str); 2] = [
    ("metadata", "types.MappingProxyType"),
    ("_field_type", "dataclasses._FIELD"),
];

/// `dataclasses.MISSING`, the sentinel CPython reports for an unset `default` or
/// `default_factory`.
fn missing() -> Value {
    Value::Marker(Marker(StaticStrings::Missing))
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, DataclassField> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DataclassField
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // `Field` defines no `__eq__`, so it compares by identity, which
        // `Value::py_eq_impl` resolves before ever reaching here.
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// CPython's `Field.__repr__`, attribute for attribute. The spellings Monty
    /// cannot reproduce are documented divergences: `type` is annotation text,
    /// and `MISSING` prints as a bare name rather than the sentinel object.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;
        // Cloned out first: recursing into `py_repr_fmt` needs the heap mutably.
        let (name, annotation, default, factory, adopted) = {
            let this = self.get(vm.heap);
            (
                this.name.map(Value::InternString),
                this.annotation.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                this.default.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                this.default_factory.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                this.name.is_some(),
            )
        };
        defer_drop!(annotation, vm);
        defer_drop!(default, vm);
        defer_drop!(factory, vm);
        f.write_str("Field(name=")?;
        write_or(name.as_ref(), "None", f, vm, heap_ids)?;
        f.write_str(",type=")?;
        write_or(annotation.as_ref(), "None", f, vm, heap_ids)?;
        f.write_str(",default=")?;
        write_or(default.as_ref(), "MISSING", f, vm, heap_ids)?;
        f.write_str(",default_factory=")?;
        write_or(factory.as_ref(), "MISSING", f, vm, heap_ids)?;
        // Constants, because `field()` refuses every argument that would vary
        // them — as `py_getattr` below notes.
        f.write_str(",init=True,repr=True,hash=None,compare=True,")?;
        // A field CPython has not adopted still reports `kw_only` as MISSING and
        // `_field_type` as None; adoption is what fills them in.
        let (kw_only, field_type) = if adopted {
            ("False", "_FIELD")
        } else {
            ("MISSING", "None")
        };
        Ok(write!(
            f,
            "metadata=mappingproxy({{}}),kw_only={kw_only},doc=None,_field_type={field_type})"
        )?)
    }

    /// Everything but `name`/`type`/`default`/`default_factory` is a constant,
    /// because the arguments that would vary them are refused by `field()`, so
    /// every field Monty builds carries CPython's defaults.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        let value = match attr_str {
            // `None` until a decorator adopts the field, as in CPython.
            "name" => match self.get(vm.heap).name {
                Some(name) => {
                    let name = vm.interns.get_str(name).to_owned();
                    allocate_string(name, vm.heap)
                }
                None => Value::None,
            },
            "type" => match self.get(vm.heap).annotation.as_ref() {
                Some(annotation) => annotation.clone_with_heap(vm.heap),
                None => Value::None,
            },
            // An unset default or factory *is* `MISSING` in CPython.
            "default" => clone_or_missing(self.get(vm.heap).default.as_ref(), vm),
            "default_factory" => clone_or_missing(self.get(vm.heap).default_factory.as_ref(), vm),
            "init" | "repr" | "compare" => Value::Bool(true),
            "kw_only" => Value::Bool(false),
            "hash" | "doc" => Value::None,
            _ => match UNMODELLED_ATTRS.iter().find(|&&(name, _)| name == attr_str) {
                Some((name, missing)) => return Err(unmodelled_attr_error(name, missing)),
                None => return Err(ExcType::attribute_error("Field", attr_str)),
            },
        };
        Ok(Some(CallResult::Value(value)))
    }
}

/// `NotImplementedError` for an attribute CPython has: reading it is a gap in
/// Monty, not a mistake in the calling code.
fn unmodelled_attr_error(attr: &str, missing: &str) -> RunError {
    ExcType::not_implemented(format!(
        "Field.{attr} is not yet supported, {missing} is not implemented"
    ))
    .into()
}

/// Writes a repr slot, or `absent` when it holds nothing — `None` for the two
/// adoption fills, `MISSING` for the two CPython sentinels.
fn write_or(
    value: Option<&Value>,
    absent: &str,
    f: &mut impl Write,
    vm: &mut VM<'_>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    match value {
        Some(value) => value.py_repr_fmt(f, vm, heap_ids),
        None => Ok(f.write_str(absent)?),
    }
}

/// Clones a stored slot for `py_getattr`, reporting `MISSING` when it is unset.
fn clone_or_missing(value: Option<&Value>, vm: &VM<'_>) -> Value {
    value.map_or_else(missing, |value| value.clone_with_heap(vm.heap))
}

impl HeapItem for DataclassField {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for value in [&mut self.annotation, &mut self.default, &mut self.default_factory]
            .into_iter()
            .flatten()
        {
            value.py_dec_ref_ids(stack);
        }
    }
}
