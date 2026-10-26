//! The `@dataclass(...)` options, and the `__dataclass_params__` object a
//! decorated class keeps them in.

use std::fmt::Write;

use crate::{
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{HeapId, HeapItem, HeapObjectRead},
    types::{LazyHeapSet, PyTrait, Type},
    value::{EitherStr, Value},
};

/// The `@dataclass(...)` options Monty implements.
///
/// Small and `Copy`, so it doubles as the payload of the *configured decorator*
/// (`dataclass(frozen=True)`) without a heap allocation. `slots` and
/// `weakref_slot` are rejected at the call, so each CPython flag is either
/// stored here or known to hold its default.
///
/// Dumps written before a field existed load it at CPython's default, which is
/// what those builds required it to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per CPython option, which keeps the mapping checkable"
)]
pub(crate) struct DataclassOptions {
    /// Generate `__init__` (CPython's `init`, default `True`).
    pub init: bool,
    /// Generate a field-wise `__repr__` (CPython's `repr`, default `True`).
    pub repr: bool,
    /// Generate a field-wise `__eq__` (CPython's `eq`, default `True`).
    pub eq: bool,
    /// Generate `__lt__`/`__le__`/`__gt__`/`__ge__` (CPython's `order`,
    /// default `False`).
    pub order: bool,
    /// Generate `__hash__` whatever `eq` and `frozen` imply (CPython's
    /// `unsafe_hash`, default `False`).
    pub unsafe_hash: bool,
    /// Reject attribute assignment, and hash by field values when `eq` is also
    /// set (CPython's `frozen`, default `False`).
    pub frozen: bool,
    /// Write `__match_args__` (CPython's `match_args`, default `True`).
    pub match_args: bool,
    /// Make every field that does not say otherwise keyword-only in `__init__`
    /// (CPython's `kw_only`, default `False`).
    pub kw_only: bool,
}

impl Default for DataclassOptions {
    /// CPython's defaults.
    fn default() -> Self {
        Self {
            init: true,
            repr: true,
            eq: true,
            order: false,
            unsafe_hash: false,
            frozen: false,
            match_args: true,
            kw_only: false,
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
    pub(super) fn options(&self) -> DataclassOptions {
        self.options
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

    /// CPython's `_DataclassParams.__repr__`, flag for flag. `slots` and
    /// `weakref_slot` print as constants, which cannot misreport a class because
    /// any other value is refused at decoration.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let o = self.get(vm.heap).options;
        Ok(write!(
            f,
            "_DataclassParams(init={},repr={},eq={},order={},unsafe_hash={},frozen={},\
             match_args={},kw_only={},slots=False,weakref_slot=False)",
            python_bool(o.init),
            python_bool(o.repr),
            python_bool(o.eq),
            python_bool(o.order),
            python_bool(o.unsafe_hash),
            python_bool(o.frozen),
            python_bool(o.match_args),
            python_bool(o.kw_only),
        )?)
    }

    /// Every flag CPython exposes; the two Monty refuses read as their defaults.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        let o = self.get(vm.heap).options;
        let value = match attr_str {
            "init" => o.init,
            "repr" => o.repr,
            "eq" => o.eq,
            "order" => o.order,
            "unsafe_hash" => o.unsafe_hash,
            "frozen" => o.frozen,
            "match_args" => o.match_args,
            "kw_only" => o.kw_only,
            "slots" | "weakref_slot" => false,
            _ => return Err(ExcType::attribute_error("_DataclassParams", attr_str)),
        };
        Ok(Some(CallResult::Value(Value::Bool(value))))
    }
}

/// Python's spelling of a bool, for the reprs of `_DataclassParams` and `Field`.
pub(super) fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

impl HeapItem for DataclassParams {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Bools only, no heap references.
    }
}
