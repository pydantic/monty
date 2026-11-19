//! Implementation of the enumerate() builtin function.

use monty_macros::py_signature;
use monty_types::ResourceError;
use num_bigint::BigInt;
use smallvec::smallvec;

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::DropGuard,
    types::{List, LongInt, allocate_tuple},
    value::Value,
};

/// Implementation of the enumerate() builtin function.
///
/// Returns a list of (index, value) tuples.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
/// Invalid calls raise `TypeError`. CPython’s custom parser can report a different
/// argument first, for example in `enumerate(start=1)`.
#[py_signature("enumerate(iterable, start: SupportsIndex=0)")]
pub fn builtin_enumerate(vm: &mut VM<'_>, params: &mut EnumerateArgs) -> RunResult<Value> {
    let iter = params.iterable.take().into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    let small_start = params.start.view(vm).to_i64(vm);
    let result: Vec<Value> = Vec::new();
    let mut result_guard = DropGuard::new(result, vm);
    let (result, vm) = result_guard.as_parts_mut();

    while let Some(item) = iter.py_next(vm)? {
        let offset = result.len();
        let (index, item) = if let Some(n) = small_start.and_then(|n| n.checked_add_unsigned(offset as u64)) {
            (Value::Int(n), item)
        } else {
            let mut item_guard = DropGuard::new(item, vm);
            let (_, vm) = item_guard.as_parts_mut();
            let index = params.start.view(vm).with_value(
                vm,
                |n| Ok(BigInt::from(n) + offset),
                |n| {
                    let bytes = usize::try_from(n.bits().div_ceil(64))
                        .unwrap_or(usize::MAX)
                        .saturating_mul(8);
                    // A carry can grow the cloned magnitude while its old buffer is still live.
                    vm.heap.tracker.check_allocation(bytes.saturating_mul(3))?;
                    Ok::<_, ResourceError>(n + offset)
                },
            )?;
            let (item, vm) = item_guard.into_parts();
            (LongInt::new(index).into_value(vm.heap), item)
        };
        let tuple_val = allocate_tuple(smallvec![index, item], vm.heap);
        result.push(tuple_val);
    }

    let (result, vm) = result_guard.into_parts();
    Ok(Value::allocate(List::new(result), vm.heap))
}
