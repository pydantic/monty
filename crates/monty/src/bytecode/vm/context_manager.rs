//! Context-manager opcode helpers (`BeforeWith`, `WithExit`, `WithExceptStart`).
//!
//! These three opcodes dispatch directly to `PyTrait::py_enter` / `PyTrait::py_exit`
//! rather than the generic `CallAttr` machinery — explicit invocation is cheaper
//! (no attribute lookup) and matches CPython's `BEFORE_WITH` / `WITH_EXCEPT_START`
//! shape. Each helper returns a `CallResult` so the host can suspend during the
//! call (e.g. `OpenFile.__exit__` issues an `OsCall` to close the file); the
//! caller routes the result through `handle_call_result!`.

use super::{CallResult, VM};
use crate::{
    defer_drop,
    exception_private::{ExcType, ExceptionRaise, RunError, RunResult, SimpleException},
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

impl<T: ResourceTracker> VM<'_, T> {
    /// `BeforeWith`: peek the context manager at TOS, call `__enter__`, and push
    /// the result. The context manager stays on the stack across the body so the
    /// matching `WithExit` / `WithExceptStart` can find it.
    ///
    /// When the value cannot act as a context manager, CPython raises a
    /// `TypeError` with a specific message — not the generic `AttributeError`
    /// that `__enter__()` would yield on direct invocation. We translate the
    /// underlying `AttributeError` (raised by `PyTrait::py_enter`'s default
    /// impl for types that don't implement the protocol) into the expected
    /// `TypeError` here.
    pub(super) fn exec_before_with(&mut self) -> RunResult<CallResult> {
        // Pattern-matching `*self.peek()` is a place expression so it doesn't
        // move the whole Value — Rust only copies the HeapId out.
        let Value::Ref(ctx_id) = *self.peek() else {
            return Err(not_a_context_manager(self));
        };
        match self.heap.read(ctx_id).py_enter(ctx_id, self) {
            Err(err) if is_missing_attr(&err, "__enter__") => Err(not_a_context_manager(self)),
            other => other,
        }
    }

    /// `WithExit`: pop the context manager, call `__exit__(None, None, None)`,
    /// and push the result. The compiler emits a trailing `Pop` to discard the
    /// result; splitting "call + discard" lets the call yield to the host while
    /// the discard happens once the host has resumed with the return value.
    pub(super) fn exec_with_exit(&mut self) -> RunResult<CallResult> {
        let this = self;
        let ctx = this.pop();
        let Value::Ref(ctx_id) = ctx else {
            // Unreachable in well-formed bytecode (BeforeWith would have rejected
            // a non-Ref ctx), but guard rather than panic.
            let ty = ctx.py_type(this);
            ctx.drop_with_heap(this);
            return Err(ExcType::attribute_error(ty, "__exit__"));
        };
        // Drop the ctx reference on every exit path of this function — whether
        // py_exit returns a value, yields, or errors. This matches the ref-count
        // balance from BeforeWith's push.
        defer_drop!(ctx, this);
        this.heap.read(ctx_id).py_exit(ctx_id, this, None)
    }

    /// `WithExceptStart`: peek at `[..., ctx, exc]`, call
    /// `__exit__(type(exc), exc, None)`, and push the raw return value. The
    /// compiler-emitted `JumpIfTrue` then branches on its truthiness to either
    /// suppress (Pop ctx, Pop exc, ClearException) or re-raise (Pop ctx, Pop exc,
    /// Reraise).
    pub(super) fn exec_with_except_start(&mut self) -> RunResult<CallResult> {
        let len = self.stack.len();
        // Pattern-match via place expressions so neither stack slot is moved.
        let Value::Ref(exc_id) = self.stack[len - 1] else {
            // The exception value pushed by `handle_exception` is always a heap
            // ref; reaching this branch means the VM is in a corrupted state.
            return Err(RunError::internal("WithExceptStart: expected exception ref on stack"));
        };
        let Value::Ref(ctx_id) = self.stack[len - 2] else {
            // BeforeWith already validated ctx as Value::Ref before pushing it
            // onto the stack, so a non-Ref here means the VM is corrupted.
            return Err(RunError::internal(
                "WithExceptStart: expected context-manager ref on stack",
            ));
        };
        self.heap.read(ctx_id).py_exit(ctx_id, self, Some(exc_id))
    }
}

/// Builds the CPython-equivalent `TypeError` raised when a value used in a
/// `with` statement does not implement the context-manager protocol.
///
/// CPython's message names the missing dunder (`__exit__` is what it checks
/// for first); Monty checks `__enter__` first internally but uses the same
/// user-visible text so traceback-equivalence tests pass.
fn not_a_context_manager<T: ResourceTracker>(vm: &VM<'_, T>) -> RunError {
    let ty = vm.peek().py_type(vm);
    SimpleException::new_msg(
        ExcType::TypeError,
        format!("'{ty}' object does not support the context manager protocol (missed __exit__ method)"),
    )
    .into()
}

/// Returns true when the error is the `AttributeError` raised by `py_enter`
/// / `py_exit`'s default impl — i.e. the type does not implement the
/// context-manager protocol at all.
fn is_missing_attr(err: &RunError, attr: &'static str) -> bool {
    let RunError::Exc(ExceptionRaise { exc, .. }) = err else {
        return false;
    };
    if exc.exc_type() != ExcType::AttributeError {
        return false;
    }
    exc.arg().is_some_and(|m| m.contains(attr))
}
