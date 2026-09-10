//! The per-item step the adaptors share.
//!
//! Every source-driving adaptor pulls from its wrapped iterator through
//! [`next_source`], the one place an adaptor re-enters the VM. `takewhile`,
//! `dropwhile` and `filterfalse` share the decision around it too: only what
//! they do with the item differs, so the guards that keep a raising test from
//! leaking it live here rather than in three copies.

use crate::{bytecode::VM, defer_drop, exception_private::RunResult, heap::DropGuard, value::Value};

/// Pulls one item from a wrapped `source`, charging one recursion level.
///
/// This is the only place an adaptor re-enters `py_next` on the native Rust
/// stack, so it is the only place the depth belongs: nesting stays bounded and
/// raises `RecursionError` instead of overflowing, while an adaptor answering
/// from its own state — a spent source, a latched predicate, an `accumulate`
/// still holding its `initial` — pays nothing and so cannot fail a level early.
pub(super) fn next_source(source: &Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    let mut read = source.read(vm);
    read.py_next(vm)
}

/// Pulls one item from `source`, releasing the caller's clone before returning.
///
/// `source` is taken by value because it must be a clone: `py_next` re-enters
/// the VM, and the adaptor's own reference is unreachable behind that borrow.
pub(super) fn next_item(source: Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    defer_drop!(source, vm);
    next_source(source, vm)
}

/// Pulls one item and applies `test` to it, returning both the item and the
/// answer.
///
/// The item is guarded across `test`, so a predicate that raises drops it
/// instead of leaking, and `predicate` is released on every path. Callers get
/// the item back owned and decide whether to yield it — which is the only part
/// that differs between the adaptors.
pub(super) fn next_tested<'h>(
    predicate: Value,
    source: Value,
    vm: &mut VM<'h>,
    test: impl FnOnce(&Value, &Value, &mut VM<'h>) -> RunResult<bool>,
) -> RunResult<Option<(Value, bool)>> {
    defer_drop!(predicate, vm);
    let Some(item) = next_item(source, vm)? else {
        return Ok(None);
    };
    let mut item_guard = DropGuard::new(item, vm);
    let (item, vm) = item_guard.as_parts_mut();
    let answer = test(predicate, item, vm)?;
    let (item, _) = item_guard.into_parts();
    Ok(Some((item, answer)))
}
