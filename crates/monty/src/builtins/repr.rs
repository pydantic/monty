//! Implementation of the repr() builtin function.

use crate::{
    args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, heap::RuntimeRef, types::PyTrait,
    value::Value,
};

/// Implementation of the repr() builtin function.
///
/// Returns a string containing a printable representation of an object.
/// `py_repr` already yields a heap `str` `Value`, so it is returned as-is
/// without an intermediate `String` round-trip.
pub fn builtin_repr<H: RuntimeRef>(vm: &mut VM<'_, H>, args: ArgValues<H>) -> RunResult<Value<H>> {
    let value = args.get_one_arg("repr", vm.heap)?;
    defer_drop!(value, vm);
    value.py_repr(vm)
}
