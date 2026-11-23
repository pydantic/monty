//! Implementation of the enumerate() builtin function.

use smallvec::smallvec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::DropGuard,
    types::{List, allocate_tuple},
    value::Value,
};

#[derive(FromArgs)]
#[from_args(name = "enumerate")]
struct EnumerateArgs {
    #[from_args(static_string = "IterableArg")]
    iterable: Value,
    #[from_args(default)]
    start: Option<Value>,
}

/// Implementation of the enumerate() builtin function.
///
/// Returns a list of (index, value) tuples.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_enumerate(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let EnumerateArgs { iterable, start } = EnumerateArgs::from_args(args, vm)?;
    // Guard `start` before building the iterator: a non-iterable `iterable`
    // errors out of `py_iter`, and a heap-backed start must not leak.
    defer_drop!(start, vm);
    let iter = iterable.into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    // Get start index (default 0)
    let mut index: i64 = match start {
        Some(Value::Int(n)) => *n,
        Some(Value::Bool(b)) => i64::from(*b),
        Some(v) => {
            let type_name = v.py_type_name(vm);
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("'{type_name}' object cannot be interpreted as an integer"),
            )
            .into());
        }
        None => 0,
    };

    let result: Vec<Value> = Vec::new();
    let mut result_guard = DropGuard::new(result, vm);
    let (result, vm) = result_guard.as_parts_mut();

    while let Some(item) = iter.py_next(vm)? {
        // Create tuple (index, item)
        let tuple_val = allocate_tuple(smallvec![Value::Int(index), item], vm.heap);
        result.push(tuple_val);
        index += 1;
    }

    let (result, vm) = result_guard.into_parts();
    Ok(Value::allocate(List::new(result), vm.heap))
}
