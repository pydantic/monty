//! The `dataclasses.Field` objects making up a class's `__dataclass_fields__`:
//! the type itself, the `field()` constructor, and the accessors that decode one
//! back out of the heap. The decorator that builds and adopts them lives in the
//! parent module.

use std::fmt::Write;

use super::options::python_bool;
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead},
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
/// **Owns heap references** (`annotation`, `default`, `default_factory`, `doc`),
/// reported in `py_dec_ref_ids` below and `heap::for_each_child_id` — a default
/// or a factory can reach back to its class, closing a cycle. `metadata` is not
/// stored: `field()` refuses it (see `py_getattr` below).
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
    /// The switches the generated methods consult. Dumps written before they
    /// existed only held fields `field()` had refused any switch for.
    #[serde(default)]
    flags: FieldFlags,
    /// Whether the entry is a real field or an `InitVar` pseudo-field.
    #[serde(default)]
    kind: FieldKind,
    /// `field(doc=...)`, stored as given; `None` when unset. CPython only reads
    /// it to document `__slots__`, which Monty does not have.
    #[serde(default)]
    doc: Option<Value>,
}

/// What a `__dataclass_fields__` entry is: CPython's `Field._field_type`.
///
/// `ClassVar` has no variant because Monty drops those entries entirely (see
/// `limitations/dataclasses.md`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FieldKind {
    /// An ordinary field: stored on the instance and seen by every method.
    #[default]
    Field,
    /// An `InitVar[...]` pseudo-field: an `__init__` parameter, passed on to
    /// `__post_init__` and never stored.
    InitVar,
}

impl FieldKind {
    /// The `dataclasses` sentinel CPython reports as `Field._field_type`.
    #[must_use]
    pub fn sentinel(self) -> StaticStrings {
        match self {
            Self::Field => StaticStrings::FieldSentinel,
            Self::InitVar => StaticStrings::InitVarSentinel,
        }
    }
}

/// The per-field switches `field()` records, read by the decorator when it
/// builds the generated methods.
///
/// `Copy` and heap-free, so the decorator can read them out of the heap
/// alongside the field name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct FieldFlags {
    /// `field(init=)`: whether the field is an `__init__` parameter.
    pub init: bool,
    /// `field(repr=)`: whether the field appears in the generated `__repr__`.
    pub repr: bool,
    /// `field(compare=)`: whether the field takes part in `__eq__` and ordering.
    pub compare: bool,
    /// `field(hash=)`. `None` is CPython's default: follow `compare`.
    pub hash: Option<bool>,
    /// `field(kw_only=)`. `None` is CPython's `MISSING`, which adoption replaces
    /// with the class's answer.
    pub kw_only: Option<bool>,
}

impl Default for FieldFlags {
    /// CPython's `field()` defaults, which a plain annotated attribute also gets.
    fn default() -> Self {
        Self {
            init: true,
            repr: true,
            compare: true,
            hash: None,
            kw_only: None,
        }
    }
}

impl FieldFlags {
    /// Whether the field takes part in the generated `__hash__`: the `hash`
    /// switch, falling back to `compare` when unset.
    #[must_use]
    pub fn hashed(self) -> bool {
        self.hash.unwrap_or(self.compare)
    }

    /// Whether an adopted field is keyword-only. A field from a dump written
    /// before `kw_only` was recorded has none, and was positional.
    #[must_use]
    pub fn is_kw_only(self) -> bool {
        self.kw_only.unwrap_or(false)
    }
}

impl DataclassField {
    /// Builds an adopted field, taking ownership of `annotation` and `default`.
    ///
    /// For a plain `x: int = 5`, where no `field()` object exists at all, so
    /// `kw_only` comes from the class.
    #[must_use]
    pub fn new(name: StringId, annotation: Value, default: Option<Value>, kw_only: bool, kind: FieldKind) -> Self {
        Self {
            name: Some(name),
            annotation: Some(annotation),
            default,
            default_factory: None,
            flags: FieldFlags {
                kw_only: Some(kw_only),
                ..FieldFlags::default()
            },
            kind,
            doc: None,
        }
    }

    /// Builds the un-adopted field `field(...)` returns, owning `default`,
    /// `default_factory` and `doc`. The decorator supplies the name and
    /// annotation later.
    #[must_use]
    pub fn new_spec(default: Option<Value>, default_factory: Option<Value>, doc: Option<Value>) -> Self {
        Self {
            name: None,
            annotation: None,
            default,
            default_factory,
            flags: FieldFlags::default(),
            // Only the decorator sees the annotation that makes an `InitVar`.
            kind: FieldKind::Field,
            doc,
        }
    }

    /// The switches the generated methods consult for this field.
    #[must_use]
    pub fn flags(&self) -> FieldFlags {
        self.flags
    }

    /// Whether this entry is a real field or an `InitVar` pseudo-field.
    #[must_use]
    pub fn kind(&self) -> FieldKind {
        self.kind
    }

    /// The interned field name, as the generated `__init__` binds it, or
    /// `None` for a `field()` result no decorator has adopted.
    #[must_use]
    pub fn name(&self) -> Option<StringId> {
        self.name
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

    /// Whether the field is optional in the generated `__init__`: it has
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
    ///
    /// An unset `kw_only` takes the class's answer, as CPython's `_get_field`
    /// writes it into the field; a second adoption keeps the first's.
    #[must_use]
    pub fn adopt(&mut self, name: StringId, annotation: Value, kw_only: bool, kind: FieldKind) -> Option<Value> {
        self.name = Some(name);
        self.kind = kind;
        self.flags.kw_only.get_or_insert(kw_only);
        self.annotation.replace(annotation)
    }

    /// Every stored value that is a heap reference: the field's children, for
    /// the cycle collector's walk.
    pub fn ref_children(&self) -> impl Iterator<Item = HeapId> + '_ {
        [&self.annotation, &self.default, &self.default_factory, &self.doc]
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
        self.doc.drop_with(ctx);
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
        doc,
    } = FieldArgs::from_args(args, vm)?;
    // `MISSING` means "not given", since `None` is a legitimate default.
    let spec = DataclassField::new_spec(
        (!is_missing(&default)).then_some(default),
        (!is_missing(&default_factory)).then_some(default_factory),
        (!matches!(doc, Value::None)).then_some(doc),
    );
    // Guarded together, because the spec owns the captured default and factory
    // before anything below has run and reading a flag's truthiness can raise
    // (`NotImplemented` refuses it), which would otherwise strand them.
    let mut guard = DropGuard::new((spec, [init, repr, hash, compare, metadata, kw_only]), vm);
    let ((spec, flags), vm) = guard.as_parts();
    // Both given is CPython's own error, raised before it builds the `Field` at
    // all — so it outranks the Monty-only refusals below, and no flag may be
    // read ahead of it.
    if spec.default().is_some() && spec.default_factory().is_some() {
        return Err(ExcType::value_error("cannot specify both default and default_factory"));
    }
    let [init, repr, hash, compare, metadata, kw_only] = flags;
    // Refused where it is written rather than accepted and dropped: `metadata`
    // needs `types.MappingProxyType`.
    if !matches!(metadata, Value::None) {
        return Err(ExcType::not_implemented("field() does not yet support the metadata argument").into());
    }
    // CPython stores the arguments as given and its generated code takes their
    // truthiness later; Monty takes it here. `hash` and `kw_only` keep their
    // unset state, which means something other than `False` for both.
    let field_flags = FieldFlags {
        init: init.py_bool(vm)?,
        repr: repr.py_bool(vm)?,
        compare: compare.py_bool(vm)?,
        hash: match hash {
            Value::None => None,
            hash => Some(hash.py_bool(vm)?),
        },
        kw_only: if is_missing(kw_only) {
            None
        } else {
            Some(kw_only.py_bool(vm)?)
        },
    };
    let ((mut spec, flags), vm) = guard.into_parts();
    flags.drop_with(vm);
    spec.flags = field_flags;
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
    /// CPython's optional per-field docstring, stored but never read.
    #[from_args(kw_only, default = Value::None)]
    doc: Value,
}

/// The `Field` a heap id points at, if it is one.
pub(super) fn field_at_id(heap: &Heap, field_id: HeapId) -> Option<&DataclassField> {
    match heap.get(field_id) {
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

/// `Field` attributes CPython has but Monty has no object for, paired with what
/// is missing. Refused rather than faked.
const UNMODELLED_ATTRS: [(&str, &str); 1] = [("metadata", "types.MappingProxyType")];

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

    /// CPython's `Field.__repr__`, attribute for attribute. `type` is annotation
    /// text, the one spelling Monty cannot reproduce (a documented divergence).
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;
        // Cloned out first: recursing into `py_repr_fmt` needs the heap mutably.
        let (name, annotation, default, factory, doc, flags, kind) = {
            let this = self.get(vm.heap);
            (
                this.name.map(Value::InternString),
                this.annotation.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                this.default.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                this.default_factory.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                this.doc.as_ref().map(|v| v.clone_with_heap(vm.heap)),
                this.flags,
                this.kind,
            )
        };
        let adopted = name.is_some();
        defer_drop!(annotation, vm);
        defer_drop!(default, vm);
        defer_drop!(factory, vm);
        defer_drop!(doc, vm);
        f.write_str("Field(name=")?;
        name.as_ref().unwrap_or(&Value::None).py_repr_fmt(f, vm, heap_ids)?;
        f.write_str(",type=")?;
        annotation
            .as_ref()
            .unwrap_or(&Value::None)
            .py_repr_fmt(f, vm, heap_ids)?;
        f.write_str(",default=")?;
        default.as_ref().unwrap_or(&missing()).py_repr_fmt(f, vm, heap_ids)?;
        f.write_str(",default_factory=")?;
        factory.as_ref().unwrap_or(&missing()).py_repr_fmt(f, vm, heap_ids)?;
        let hash = flags.hash.map_or("None", python_bool);
        write!(
            f,
            ",init={},repr={},hash={hash},compare={},metadata=mappingproxy({{}}),kw_only=",
            python_bool(flags.init),
            python_bool(flags.repr),
            python_bool(flags.compare),
        )?;
        // An un-adopted field reports `kw_only` as MISSING when `field()` left
        // it unset, and `_field_type` as None; adoption fills both in.
        match flags.kw_only {
            Some(kw_only) => f.write_str(python_bool(kw_only))?,
            None if adopted => f.write_str("False")?,
            None => missing().py_repr_fmt(f, vm, heap_ids)?,
        }
        f.write_str(",doc=")?;
        doc.as_ref().unwrap_or(&Value::None).py_repr_fmt(f, vm, heap_ids)?;
        let field_type: &str = if adopted { kind.sentinel().into() } else { "None" };
        Ok(write!(f, ",_field_type={field_type})")?)
    }

    /// Reads back what `field()` and adoption recorded. `metadata` raises, as
    /// `field()` refuses it.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        let this = self.get(vm.heap);
        let flags = this.flags;
        let value = match attr_str {
            // `None` until a decorator adopts the field, as in CPython.
            "name" => match this.name {
                Some(name) => {
                    let name = vm.interns.get_str(name).to_owned();
                    allocate_string(name, vm.heap)
                }
                None => Value::None,
            },
            "type" => match this.annotation.as_ref() {
                Some(annotation) => annotation.clone_with_heap(vm.heap),
                None => Value::None,
            },
            // An unset default or factory *is* `MISSING` in CPython.
            "default" => clone_or_missing(this.default.as_ref(), vm),
            "default_factory" => clone_or_missing(this.default_factory.as_ref(), vm),
            "init" => Value::Bool(flags.init),
            "repr" => Value::Bool(flags.repr),
            "compare" => Value::Bool(flags.compare),
            "hash" => flags.hash.map_or(Value::None, Value::Bool),
            "kw_only" => match (flags.kw_only, this.name) {
                (Some(kw_only), _) => Value::Bool(kw_only),
                (None, Some(_)) => Value::Bool(false),
                (None, None) => missing(),
            },
            "doc" => this
                .doc
                .as_ref()
                .map_or(Value::None, |doc| doc.clone_with_heap(vm.heap)),
            "_field_type" => match this.name {
                Some(_) => Value::Marker(Marker(this.kind.sentinel())),
                None => Value::None,
            },
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

/// Clones a stored slot for `py_getattr`, reporting `MISSING` when it is unset.
fn clone_or_missing(value: Option<&Value>, vm: &VM<'_>) -> Value {
    value.map_or_else(missing, |value| value.clone_with_heap(vm.heap))
}

impl HeapItem for DataclassField {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for value in [
            &mut self.annotation,
            &mut self.default,
            &mut self.default_factory,
            &mut self.doc,
        ]
        .into_iter()
        .flatten()
        {
            value.py_dec_ref_ids(stack);
        }
    }
}
