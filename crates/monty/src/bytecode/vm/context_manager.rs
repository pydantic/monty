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
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

impl<T: ResourceTracker> VM<'_, T> {
    /// `BeforeWith`: peek the context manager at TOS, call `__enter__`, and push
    /// the result. The context manager stays on the stack across the body so the
    /// matching `WithExit` / `WithExceptStart` can find it.
    ///
    /// The CPython "object does not support the context manager protocol"
    /// `TypeError` is gated on [`PyTrait::py_is_context_manager`] so we never
    /// have to sniff exception messages — a real context manager whose
    /// `__enter__` itself raises `AttributeError` propagates unchanged.
    pub(super) fn exec_before_with(&mut self) -> RunResult<CallResult> {
        // Pattern-matching `*self.peek()` is a place expression so it doesn't
        // move the whole Value — Rust only copies the HeapId out. Non-Ref
        // values (Int, Bool, None, …) never implement the protocol.
        let Value::Ref(ctx_id) = *self.peek() else {
            return Err(not_a_context_manager(self));
        };
        let mut ctx = self.heap.read(ctx_id);
        if ctx.py_is_context_manager() {
            ctx.py_enter(ctx_id, self)
        } else {
            Err(not_a_context_manager(self))
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
            // a non-Ref ctx), but guard rather than panic so a corrupt VM
            // surfaces a clear internal error instead of an uncontrolled drop.
            ctx.drop_with_heap(this);
            return Err(RunError::internal("WithExit: expected context-manager ref on stack"));
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
/// for first); Monty's [`PyTrait::py_is_context_manager`] gate is per-type
/// rather than per-dunder, but the user-visible text matches CPython so
/// traceback-equivalence tests pass.
fn not_a_context_manager<T: ResourceTracker>(vm: &VM<'_, T>) -> RunError {
    let ty = vm.peek().py_type(vm);
    SimpleException::new_msg(
        ExcType::TypeError,
        format!("'{ty}' object does not support the context manager protocol (missed __exit__ method)"),
    )
    .into()
}
