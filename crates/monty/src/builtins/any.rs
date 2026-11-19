//! Implementation of the any() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, defer_drop, exception_private::RunResult, types::PyTrait, value::Value};

/// Implementation of the any() builtin function.
///
/// Returns True if any element of the iterable is true.
/// Returns False for an empty iterable. Short-circuits on the first truthy value.
#[py_signature("any(iterable, /)")]
pub fn builtin_any(vm: &mut VM<'_>, args: &AnyArgs) -> RunResult<Value> {
    let iter = args.iterable.py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if item.py_bool(vm)? {
            return Ok(Value::Bool(true));
        }
    }

    Ok(Value::Bool(false))
}
