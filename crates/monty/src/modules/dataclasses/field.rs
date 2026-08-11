//! The `dataclasses.Field` objects making up a class's `__dataclass_fields__`.

use std::fmt::Write;

use crate::{
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropWithContext, HeapId, HeapItem, HeapRead},
    intern::StringId,
    types::{LazyHeapSet, PyTrait, Type, str::allocate_string},
    value::{EitherStr, Value},
};

/// One entry of a class's `__dataclass_fields__`: CPython's `dataclasses.Field`.
///
/// **Owns heap references** (`annotation`, `default`), reported in
/// `py_dec_ref_ids` below and `heap::for_each_child_id` — a default can reach
/// back to its class, closing a cycle. Only the attributes Monty can model are
/// stored; the rest are constants or refused (see `py_getattr` below).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassField {
    /// The interned field name (from an `__annotations__` key, always interned).
    name: StringId,
    /// `Field.type`: the annotation as source text, never evaluated.
    annotation: Value,
    /// The default **captured when `@dataclass` ran**, or `None` for a required
    /// field — rebinding the class attribute afterwards must not change it.
    default: Option<Value>,
}

impl DataclassField {
    /// Builds a field, taking ownership of `annotation` and `default`.
    #[must_use]
    pub fn new(name: StringId, annotation: Value, default: Option<Value>) -> Self {
        Self {
            name,
            annotation,
            default,
        }
    }

    /// The interned field name, as the synthesized `__init__` binds it.
    #[must_use]
    pub fn name(&self) -> StringId {
        self.name
    }

    /// `Field.type`, borrowed — the annotation the class was defined with.
    #[must_use]
    pub fn annotation(&self) -> &Value {
        &self.annotation
    }

    /// The captured default, or `None` for a required field.
    #[must_use]
    pub fn default(&self) -> Option<&Value> {
        self.default.as_ref()
    }
}

/// Releases a field that never reached the heap — one the decorator collected
/// before rejecting the class.
impl<C: ContainsHeap> DropWithContext<C> for DataclassField {
    fn drop_with(self, ctx: &mut C) {
        self.annotation.drop_with(ctx);
        self.default.drop_with(ctx);
    }
}

/// `Field` attributes CPython has but Monty has no object for, paired with what
/// is missing. Refused rather than faked.
const UNMODELLED_ATTRS: [(&str, &str); 3] = [
    ("default_factory", "dataclasses.MISSING"),
    ("metadata", "types.MappingProxyType"),
    ("_field_type", "dataclasses._FIELD"),
];

impl<'h> PyTrait<'h> for HeapRead<'h, DataclassField> {
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

    fn py_hash(&self, self_id: HeapId, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self_id)))
    }

    /// CPython's `Field.__repr__`, attribute for attribute. The two spellings
    /// Monty cannot reproduce are documented divergences: `type` is annotation
    /// text, and `MISSING` has no repr of its own.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;
        // Cloned out first: recursing into `py_repr_fmt` needs the heap mutably.
        let (name, annotation, default) = {
            let this = self.get(vm.heap);
            (
                Value::InternString(this.name),
                this.annotation.clone_with_heap(vm.heap),
                this.default.as_ref().map(|v| v.clone_with_heap(vm.heap)),
            )
        };
        defer_drop!(annotation, vm);
        defer_drop!(default, vm);
        f.write_str("Field(name=")?;
        name.py_repr_fmt(f, vm, heap_ids)?;
        f.write_str(",type=")?;
        annotation.py_repr_fmt(f, vm, heap_ids)?;
        f.write_str(",default=")?;
        match default {
            Some(default) => default.py_repr_fmt(f, vm, heap_ids)?,
            None => f.write_str("MISSING")?,
        }
        Ok(f.write_str(
            ",default_factory=MISSING,init=True,repr=True,hash=None,compare=True,\
             metadata=mappingproxy({}),kw_only=False,doc=None,_field_type=_FIELD)",
        )?)
    }

    /// `init`/`repr`/`compare`/`kw_only`/`hash`/`doc` are constants because the
    /// forms that vary them (`field()`, `@dataclass(...)`) are unsupported, so
    /// every field Monty builds carries CPython's defaults.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        let value = match attr_str {
            "name" => {
                let name = vm.interns.get_str(self.get(vm.heap).name).to_owned();
                allocate_string(name, vm.heap)
            }
            "type" => self.get(vm.heap).annotation.clone_with_heap(vm.heap),
            // A required field's default *is* `MISSING` in CPython.
            "default" => match self.get(vm.heap).default.as_ref() {
                Some(default) => default.clone_with_heap(vm.heap),
                None => return Err(unmodelled_attr_error("default", "dataclasses.MISSING")),
            },
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

impl HeapItem for DataclassField {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.annotation.py_dec_ref_ids(stack);
        if let Some(default) = &mut self.default {
            default.py_dec_ref_ids(stack);
        }
    }
}
