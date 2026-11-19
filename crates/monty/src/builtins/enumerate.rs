//! Implementation of the enumerate() builtin function.

use monty_macros::py_signature;
use num_traits::ToPrimitive;
use smallvec::smallvec;

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, HeapData},
    types::{List, LongInt, allocate_tuple},
    value::Value,
};

/// Implementation of the enumerate() builtin function.
///
/// Returns a list of (index, value) tuples.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
/// Invalid calls raise `TypeError`. CPython’s custom parser can report a different
/// argument first, for example in `enumerate(start=1)`.
#[py_signature("enumerate(iterable, start: SupportsIndex=0)", at_most_total)]
pub fn builtin_enumerate(vm: &mut VM<'_>, EnumerateArgs { iterable, start }: EnumerateArgs) -> RunResult<Value> {
    let iter = iterable.into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    let small_start = start.inner().to_i64();
    let result: Vec<Value> = Vec::new();
    let mut result_guard = DropGuard::new(result, vm);
    let (result, vm) = result_guard.as_parts_mut();

    while let Some(item) = iter.py_next(vm)? {
        let offset = result.len();
        let index = match small_start.and_then(|n| n.checked_add_unsigned(offset as u64)) {
            Some(n) => Value::Int(n),
            None => LongInt::new(start.inner() + offset).into_value(vm.heap),
        };
        let tuple_val = allocate_tuple(smallvec![index, item], vm.heap);
        result.push(tuple_val);
    }

    let (result, vm) = result_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(result)));
    Ok(Value::Ref(heap_id))
}
