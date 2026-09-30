//! Implementation of the locals() builtin function.

use crate::{args::ArgValues, bytecode::VM, exception_private::RunResult, value::Value};

/// Implementation of the `locals()` builtin function.
///
/// Inside a function this is a snapshot of the frame's local variables
/// (PEP 667: writes to the dict never reach the frame), at module scope a
/// snapshot of the bound globals, and inside an `exec()` / `eval()` snippet
/// the namespace dict the snippet runs in. See `limitations/eval_exec.md`.
pub fn builtin_locals(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("locals", vm.heap)?;
    vm.locals_dict()
}
