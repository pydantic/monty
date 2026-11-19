//! Implementation of the repr() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, types::PyTrait, value::Value};

/// Implementation of the repr() builtin function.
///
/// Returns a string containing a printable representation of an object.
/// `py_repr` already yields a heap `str` `Value`, so it is returned as-is
/// without an intermediate `String` round-trip.
#[py_signature("repr(obj, /)", style = one_arg)]
pub fn builtin_repr(vm: &mut VM<'_>, obj: &Value) -> RunResult<Value> {
    obj.py_repr(vm)
}
