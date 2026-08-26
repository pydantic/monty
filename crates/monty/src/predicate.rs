//! Calling a user-supplied predicate and taking its truthiness.
//!
//! Shared by `filter` and the `itertools` adaptors `takewhile`, `dropwhile`
//! and `filterfalse`, which differ only in what they do with the answer.

use crate::{args::ArgValues, bytecode::VM, defer_drop, exception_private::RunResult, types::PyTrait, value::Value};

/// Applies `predicate` to `item` and returns its truthiness.
///
/// `item` is only borrowed — the call gets its own reference, so the caller can
/// still yield the item afterwards. Goes through `evaluate_function`, which
/// runs a defined function's frame to completion, so a predicate that suspends
/// (an external function, an `os` call) is rejected rather than paused; `ctx`
/// names the caller in that error.
pub(crate) fn call_predicate(predicate: &Value, item: &Value, ctx: &'static str, vm: &mut VM<'_>) -> RunResult<bool> {
    let arg = item.clone_with_heap(vm.heap);
    let result = vm.evaluate_function(ctx, predicate, ArgValues::One(arg))?;
    // Guarded because `py_bool` can raise through a user `__bool__`.
    defer_drop!(result, vm);
    result.py_bool(vm)
}
