//! Implementation of the reversed() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::HeapData,
    resource::ResourceTracker,
    types::{List, iter::collect_owned_iterable},
    value::Value,
};

/// Argument shape for `reversed(sequence, /)` — positional-only, so
/// `style = unpack` gives CPython's exact-arity wording
/// (`reversed expected 1 argument, got 2`) and the blanket
/// `reversed() takes no keyword arguments` rejection.
#[derive(FromArgs)]
#[from_args(name = "reversed", style = unpack)]
struct ReversedArgs {
    #[from_args(pos_only)]
    sequence: Value,
}

/// Implementation of the reversed() builtin function.
///
/// Returns a list with elements in reverse order.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_reversed(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let ReversedArgs { sequence } = ReversedArgs::from_args(args, vm)?;

    // Being iterable is not enough: CPython needs `__reversed__`, or
    // `__len__` + `__getitem__`. Check before iterating so unordered and
    // one-shot iterables are rejected rather than silently reversed.
    if !is_reversible(&sequence, vm) {
        let err = ExcType::type_error_not_reversible(&sequence.py_type_name(vm));
        sequence.drop_with(vm);
        return Err(err);
    }

    // Collect all items
    let mut items: Vec<_> = collect_owned_iterable(sequence, vm)?;

    // Reverse in place
    items.reverse();

    let heap_id = vm.heap.allocate(HeapData::List(List::new(items)))?;
    Ok(Value::Ref(heap_id))
}

/// Whether `reversed()` accepts this value, mirroring CPython's requirement of
/// `__reversed__` or `__len__` + `__getitem__`.
///
/// Deliberately a positive allowlist: a newly iterable type must opt in here
/// rather than becoming reversible by accident. Sets and iterators are excluded
/// (no ordering / one-shot), as are user instances — `__reversed__` is not
/// dispatched, see `limitations/classes.md`.
fn is_reversible(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match value {
        Value::InternString(_) | Value::InternBytes(_) => true,
        Value::Ref(id) => matches!(
            vm.heap.get(*id),
            HeapData::List(_)
                | HeapData::Tuple(_)
                | HeapData::NamedTuple(_)
                | HeapData::Str(_)
                | HeapData::Bytes(_)
                | HeapData::Range(_)
                | HeapData::Dict(_)
                | HeapData::DictKeysView(_)
                | HeapData::DictItemsView(_)
                | HeapData::DictValuesView(_)
        ),
        _ => false,
    }
}
