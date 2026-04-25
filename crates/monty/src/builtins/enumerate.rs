//! Implementation of the enumerate() builtin function.

use smallvec::smallvec;

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropWithHeap, HeapData},
    resource::ResourceTracker,
    types::{List, MontyIter, PyTrait, allocate_tuple},
    value::Value,
};

/// Implementation of the enumerate() builtin function.
///
/// Returns a list of (index, value) tuples.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
///
/// Matches CPython's `enumerate(iterable, start=0)` signature: `start` may be
/// passed either positionally or as the `start=` keyword argument.
pub fn builtin_enumerate(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (iterable, start) = parse_enumerate_args(args, vm)?;
    let iter = MontyIter::new(iterable, vm)?;
    defer_drop_mut!(iter, vm);
    defer_drop!(start, vm);

    // Get start index (default 0)
    let mut index: i64 = match start {
        Some(Value::Int(n)) => *n,
        Some(Value::Bool(b)) => i64::from(*b),
        Some(v) => {
            let type_name = v.py_type(vm);
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("'{type_name}' object cannot be interpreted as an integer"),
            )
            .into());
        }
        None => 0,
    };

    let mut result: Vec<Value> = Vec::new();

    while let Some(item) = iter.for_next(vm)? {
        // Create tuple (index, item)
        let tuple_val = allocate_tuple(smallvec![Value::Int(index), item], vm.heap)?;
        result.push(tuple_val);
        index += 1;
    }

    let heap_id = vm.heap.allocate(HeapData::List(List::new(result)))?;
    Ok(Value::Ref(heap_id))
}

/// Parses arguments for `enumerate(iterable, start=0)`.
///
/// `iterable` is required and positional. `start` is optional and may be passed
/// either positionally or as the `start=` keyword argument, but not both.
fn parse_enumerate_args(
    args: ArgValues,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<(Value, Option<Value>)> {
    let (mut positional, kwargs) = args.into_parts();
    let positional_count = positional.len();

    // Need at least one positional argument (the iterable).
    let Some(iterable) = positional.next() else {
        kwargs.drop_with_heap(vm.heap);
        return Err(ExcType::type_error_at_least("enumerate", 1, 0));
    };

    // Optional positional `start`; reject any extra positional args.
    let positional_start: Option<Value> = positional.next();
    if positional.len() > 0 {
        kwargs.drop_with_heap(vm.heap);
        iterable.drop_with_heap(vm.heap);
        positional_start.drop_with_heap(vm.heap);
        positional.drop_with_heap(vm.heap);
        return Err(ExcType::type_error_at_most("enumerate", 2, positional_count));
    }

    // Pull out the `start=` keyword argument if any.
    let saw_positional_start = positional_start.is_some();
    match extract_start_kwarg(kwargs, saw_positional_start, vm) {
        Ok(kw_start) => Ok((iterable, kw_start.or(positional_start))),
        Err(err) => {
            iterable.drop_with_heap(vm.heap);
            positional_start.drop_with_heap(vm.heap);
            Err(err)
        }
    }
}

/// Walks `kwargs` and returns the value passed for `start=`, if any.
///
/// All kwargs are fully consumed before returning, so reference counts stay correct
/// even when some kwargs come after a bad one. `saw_positional_start` indicates that
/// `start` was already supplied positionally, in which case any `start=` kwarg is
/// rejected as a duplicate.
fn extract_start_kwarg(
    kwargs: KwargsValues,
    saw_positional_start: bool,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> Result<Option<Value>, RunError> {
    let mut start: Option<Value> = None;
    let mut error: Option<RunError> = None;

    for (key, value) in kwargs {
        defer_drop!(key, vm);

        // If we already hit an error, drop remaining values and continue.
        if error.is_some() {
            value.drop_with_heap(vm.heap);
            continue;
        }

        let Some(keyword_name) = key.as_either_str(vm.heap) else {
            value.drop_with_heap(vm.heap);
            error = Some(ExcType::type_error_kwargs_nonstring_key());
            continue;
        };

        let key_str = keyword_name.as_str(vm.interns);
        if key_str == "start" {
            if saw_positional_start || start.is_some() {
                value.drop_with_heap(vm.heap);
                error = Some(ExcType::type_error_multiple_values("enumerate", "start"));
            } else {
                start = Some(value);
            }
        } else {
            // Build the error before touching `vm` mutably (the format borrows `key_str`).
            let err = ExcType::type_error_unexpected_keyword("enumerate", key_str);
            value.drop_with_heap(vm.heap);
            error = Some(err);
        }
    }

    if let Some(err) = error {
        start.drop_with_heap(vm.heap);
        Err(err)
    } else {
        Ok(start)
    }
}
