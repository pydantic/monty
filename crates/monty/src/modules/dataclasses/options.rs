//! The `__dataclass_params__` object a decorated class reports its
//! `@dataclass(...)` options in.
//!
//! The options themselves are [`DataclassOptions`], which lives in
//! [`crate::types::class`] because a `Class` stores them.

use std::fmt::Write;

use crate::{
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{HeapId, HeapItem, HeapObjectRead},
    types::{DataclassOptions, LazyHeapSet, PyTrait, Type},
    value::{EitherStr, Value},
};

/// The `__dataclass_params__` object `@dataclass` writes into a class
/// namespace: CPython's `dataclasses._DataclassParams`.
///
/// A report of what the class was decorated with, never a control — the options
/// the class acts on live on the `Class` itself. Holds no heap references, so it
/// is the cheap half of a class's metadata; `__dataclass_fields__` owns the
/// captured defaults.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassParams {
    options: DataclassOptions,
}

impl DataclassParams {
    /// Wraps the options the decorator was called with.
    #[must_use]
    pub fn new(options: DataclassOptions) -> Self {
        Self { options }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, DataclassParams> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DataclassParams
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // `_DataclassParams` defines no `__eq__`, so it compares by identity,
        // which `Value::py_eq_impl` resolves before ever reaching here.
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// CPython's `_DataclassParams.__repr__`, flag for flag. The eight
    /// unimplemented options print as constants, which cannot misreport a class
    /// because any other value is refused at decoration.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let options = self.get(vm.heap).options;
        Ok(write!(
            f,
            "_DataclassParams(init=True,repr=True,eq={},order=False,unsafe_hash=False,frozen={},\
             match_args=True,kw_only=False,slots=False,weakref_slot=False)",
            python_bool(options.eq),
            python_bool(options.frozen),
        )?)
    }

    /// Every flag CPython exposes; the unimplemented ones read as the defaults
    /// the decorator insists on.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        let options = self.get(vm.heap).options;
        let value = match attr_str {
            "eq" => Value::Bool(options.eq),
            "frozen" => Value::Bool(options.frozen),
            "init" | "repr" | "match_args" => Value::Bool(true),
            "order" | "unsafe_hash" | "kw_only" | "slots" | "weakref_slot" => Value::Bool(false),
            _ => return Err(ExcType::attribute_error("_DataclassParams", attr_str)),
        };
        Ok(Some(CallResult::Value(value)))
    }
}

/// Python's spelling of a bool, for the constant flags in the repr above.
fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

impl HeapItem for DataclassParams {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Two bools, no heap references.
    }
}
