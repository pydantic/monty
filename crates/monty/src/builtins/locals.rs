//! Implementation of the locals() builtin function.

use crate::{args::ArgValues, bytecode::VM, exception_private::RunResult, heap::RuntimeRef, value::Value};

/// Implementation of the `locals()` builtin function.
///
/// Inside a function this is a snapshot of the frame's local variables
/// (PEP 667: writes to the dict never reach the frame), at module scope a
/// snapshot of the bound globals, and inside an `exec()` / `eval()` snippet
/// the namespace dict the snippet runs in. See `limitations/eval_exec.md`.
pub fn builtin_locals<H: RuntimeRef>(vm: &mut VM<'_, H>, args: ArgValues<H>) -> RunResult<Value<H>> {
    args.check_zero_args("locals", vm.heap)?;
    vm.locals_dict()
}
