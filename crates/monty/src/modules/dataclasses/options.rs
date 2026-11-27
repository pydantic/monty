//! The `@dataclass(...)` options, and the `__dataclass_params__` object a
//! decorated class keeps them in.

use std::fmt::Write;

use monty_macros::pyattrs;

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    hash::{HashValue, identity_hash},
    heap::{HeapId, HeapItem, HeapObjectRead},
    py_attrs::PyAttrs,
    types::{LazyHeapSet, PyTrait, Type, TypeRef},
    value::Value,
};

/// The `@dataclass(...)` options Monty implements.
///
/// Small and `Copy`, so it doubles as the payload of the *configured decorator*
/// (`dataclass(frozen=True)`) without a heap allocation. Every other CPython
/// flag is rejected at the call, so each is either stored here or known to hold
/// its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassOptions {
    /// Synthesize a field-wise `__eq__` (CPython's `eq`, default `True`).
    pub eq: bool,
    /// Reject attribute assignment, and hash by field values when `eq` is also
    /// set (CPython's `frozen`, default `False`).
    pub frozen: bool,
}

impl Default for DataclassOptions {
    /// CPython's defaults: `eq=True, frozen=False`.
    fn default() -> Self {
        Self {
            eq: true,
            frozen: false,
        }
    }
}

/// The `__dataclass_params__` object `@dataclass` writes into a class
/// namespace: CPython's `dataclasses._DataclassParams`.
///
/// A record of the decoration only: the generated methods were built from the
/// options, so rebinding `__dataclass_params__` changes nothing, as in CPython.
/// Holds no heap references.
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

    /// The options the class was decorated with, which `legacy` rebuilds the
    /// generated methods from.
    #[must_use]
    pub(crate) fn options(&self) -> DataclassOptions {
        self.options
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, DataclassParams> {
    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        Type::DataclassParams.into()
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

#[pyattrs]
impl<'heap> PyAttrs<'heap> for DataclassParams {
    type Receiver = HeapObjectRead<'heap, DataclassParams>;

    #[pyget(Eq, member)]
    fn eq<'h>(this: &HeapObjectRead<'h, DataclassParams>, vm: &mut VM<'h>) -> Value {
        Value::Bool(this.get(vm.heap).options.eq)
    }

    #[pyget(Frozen, member)]
    fn frozen<'h>(this: &HeapObjectRead<'h, DataclassParams>, vm: &mut VM<'h>) -> Value {
        Value::Bool(this.get(vm.heap).options.frozen)
    }

    #[pyget(Init, member)]
    fn init_flag<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(true)
    }

    #[pyget(Repr, member)]
    fn repr_flag<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(true)
    }

    #[pyget(MatchArgs, member)]
    fn match_args<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(true)
    }

    #[pyget(Order, member)]
    fn order<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(false)
    }

    #[pyget(UnsafeHash, member)]
    fn unsafe_hash<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(false)
    }

    #[pyget(KwOnly, member)]
    fn kw_only<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(false)
    }

    #[pyget(Slots, member)]
    fn slots<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(false)
    }

    #[pyget(WeakrefSlot, member)]
    fn weakref_slot<'h>(_this: &HeapObjectRead<'h, DataclassParams>, _vm: &mut VM<'h>) -> Value {
        Value::Bool(false)
    }
}
