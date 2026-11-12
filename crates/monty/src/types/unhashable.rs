//! Hashing for set elements and dict keys, with CPython's
//! `cannot use 'T' as a set element (...)` / `... as a dict key (...)` errors.
//!
//! CPython builds these in `set_unhashable_type` (`Objects/setobject.c`) and
//! `dict_unhashable_type` (`Objects/dictobject.c`): a hash that raises an exact
//! `TypeError` is re-raised wrapped, naming the element or key's own type, while
//! any other exception passes through unchanged. Paths that call `PyObject_Hash`
//! directly (`hash()`, `set.intersection(iterable)`) raise the bare error instead.

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, unhashable_type_message},
    hash::HashValue,
    py_value::PyValue,
    types::PyTrait,
    value::Value,
};

/// Hashes a value about to be stored in, or looked up in, a set.
pub(crate) fn set_element_hash(value: &Value, vm: &mut VM<'_>) -> RunResult<u64> {
    hash_as(value, HashRole::SetElement, vm)
}

/// Hashes a value about to be stored in, or looked up in, a dict.
pub(crate) fn dict_key_hash(value: &Value, vm: &mut VM<'_>) -> RunResult<u64> {
    hash_as(value, HashRole::DictKey, vm)
}

/// Hashes a value as `hash()` does: an unhashable value raises
/// `unhashable type: 'T'`, naming the innermost culprit of a tuple.
pub(crate) fn plain_hash(value: &Value, vm: &mut VM<'_>) -> RunResult<HashValue> {
    hash_or_culprit(value, vm)?.map_err(|culprit| ExcType::type_error_unhashable(&culprit))
}

/// What a value is hashed for, which decides how a hashing error is reported.
#[derive(Clone, Copy)]
pub(crate) enum HashRole {
    /// Wrapped as `cannot use 'T' as a set element (...)`.
    SetElement,
    /// Wrapped as `cannot use 'T' as a dict key (...)`.
    DictKey,
    /// Unwrapped, as [`plain_hash`] reports it.
    Plain,
}

/// Hashes `value` for `role`.
pub(crate) fn hash_as(value: &Value, role: HashRole, vm: &mut VM<'_>) -> RunResult<u64> {
    match role {
        HashRole::SetElement => wrapped_hash(value, ExcType::type_error_unhashable_set_element, vm),
        HashRole::DictKey => wrapped_hash(value, ExcType::type_error_unhashable_dict_key, vm),
        HashRole::Plain => plain_hash(value, vm).map(HashValue::raw),
    }
}

/// Hashes `value`, passing an unhashable value or a raised exact `TypeError` to
/// `wrap` as `(type name, inner message)`. Other errors, uncatchable ones included,
/// pass through: CPython does not wrap a `TypeError` subclass either.
fn wrapped_hash(value: &Value, wrap: fn(&str, &str) -> RunError, vm: &mut VM<'_>) -> RunResult<u64> {
    let reason = match hash_or_culprit(value, vm) {
        Ok(Ok(hash)) => return Ok(hash.raw()),
        Ok(Err(culprit)) => unhashable_type_message(&culprit),
        Err(RunError::Exc(raise)) if raise.exc.exc_type() == ExcType::TypeError => raise.exc.py_str(),
        Err(err) => return Err(err),
    };
    Err(wrap(&value.py_type_name(vm), &reason))
}

/// Hashes `value`, or returns the type name of the innermost value that will
/// not hash: a tuple's culprit item rather than `tuple`, as CPython names it.
///
/// Monty's `py_hash` reports an unhashable value as `None` without saying
/// which item refused, so tuple-likes are walked here in the same hashing pass.
pub(crate) fn hash_or_culprit(value: &Value, vm: &mut VM<'_>) -> RunResult<Result<HashValue, String>> {
    let hash = match value {
        Value::Ref(id) => match PyValue::from_heap(*id, vm.heap) {
            PyValue::Tuple(tuple) => return tuple.hash_or_culprit(vm),
            PyValue::NamedTuple(namedtuple) => return namedtuple.hash_or_culprit(vm),
            other => other.py_hash(vm)?,
        },
        _ => value.py_hash(vm)?,
    };
    Ok(hash.ok_or_else(|| value.py_type_name(vm).into_owned()))
}
