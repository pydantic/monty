//! Implementation of the all() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, defer_drop, exception_private::RunResult, types::PyTrait, value::Value};

/// Implementation of the all() builtin function.
///
/// Returns True if all elements of the iterable are true (or if the iterable is empty).
/// Short-circuits on the first falsy value.
#[py_signature("all(iterable, /)")]
pub fn builtin_all(vm: &mut VM<'_>, args: &AllArgs) -> RunResult<Value> {
    let AllArgs { iterable } = args;
    let iter = iterable.py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if !item.py_bool(vm)? {
            return Ok(Value::Bool(false));
        }
    }

    Ok(Value::Bool(true))
}
