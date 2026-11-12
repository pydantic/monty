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
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, unhashable_type_message},
    hash::HashValue,
    heap::HeapData,
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
    match value.py_hash(vm)? {
        Some(hash) => Ok(hash),
        None => Err(ExcType::type_error_unhashable(&unhashable_type_name(value, vm)?)),
    }
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
    let reason = match value.py_hash(vm) {
        Ok(Some(hash)) => return Ok(hash.raw()),
        Ok(None) => unhashable_type_message(&unhashable_type_name(value, vm)?),
        Err(RunError::Exc(raise)) if raise.exc.exc_type() == ExcType::TypeError => raise.exc.py_str(),
        Err(err) => return Err(err),
    };
    Err(wrap(&value.py_type_name(vm), &reason))
}

/// Finds the nested value responsible for a tuple-like value being unhashable.
///
/// Monty's `py_hash` reports an unhashable value as `None` rather than raising,
/// so this recovers the innermost type CPython's raised `TypeError` would name.
pub(crate) fn unhashable_type_name(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let Value::Ref(id) = value else {
        return Ok(value.py_type_name(vm).into_owned());
    };
    let len = match vm.heap.get(*id) {
        HeapData::Tuple(tuple) => tuple.as_slice().len(),
        HeapData::NamedTuple(namedtuple) => namedtuple.as_vec().len(),
        _ => return Ok(value.py_type_name(vm).into_owned()),
    };

    for index in 0..len {
        let item = match vm.heap.get(*id) {
            HeapData::Tuple(tuple) => tuple.as_slice()[index].clone_with_heap(vm.heap),
            HeapData::NamedTuple(namedtuple) => namedtuple.as_vec()[index].clone_with_heap(vm.heap),
            _ => unreachable!("tuple-like heap value changed type"),
        };
        defer_drop!(item, vm);
        if item.py_hash(vm)?.is_none() {
            return unhashable_type_name(item, vm);
        }
    }

    Ok(value.py_type_name(vm).into_owned())
}
