//! Synthetic context manager used by tests to exercise `with` statement code
//! paths that no production type currently reaches.
//!
//! **REMOVE THIS FILE** (along with the `_test_cm` builtin, the `HeapData` /
//! `HeapReadOutput` variants, and `with_test_cm.rs`) once a real context
//! manager covers the same branches — specifically:
//!
//! - `__exit__` returning truthy to suppress an in-flight exception
//!   (the swallow path emitted by `compile_with`).
//! - `__exit__` itself raising during the exception path (replacing the
//!   original exception).
//! - `__enter__` returning a value other than `self`.
//!
//! Today only [`crate::types::OpenFile`] implements the protocol, and its
//! `__exit__` always returns `None`, so those branches in
//! `crates/monty/src/bytecode/compiler.rs` (`compile_with`) and
//! `crates/monty/src/bytecode/vm/context_manager.rs` would be untested
//! without this synthetic helper.
//!
//! The type is exposed to Python through the `_test_cm()` builtin defined
//! in [`crate::builtins::test_cm`]. Both are gated behind the `test-hooks`
//! cargo feature so a production sandbox can never construct one.

use std::{fmt::Write, mem};

use ahash::AHashSet;

use super::{PyTrait, Type};
use crate::{
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{HeapId, HeapItem, HeapRead},
    resource::ResourceTracker,
    value::Value,
};

/// Configuration for a synthetic context manager.
///
/// All fields default to "passthrough" — the manager returns itself from
/// `__enter__`, returns `None` from `__exit__`, and never raises. Setting a
/// field flips one specific branch on, so a single test can pin one path at
/// a time without touching production code.
///
/// `enter_value` is intentionally restricted to an integer rather than an
/// arbitrary `Value`: a heap-allocated enter value would need refcount
/// plumbing here (Value does not impl Clone, intentionally), and an int is
/// sufficient to verify that `__enter__`'s return value flows to the `as`
/// target. If a future branch needs the test manager to return a heap
/// value, store a `HeapId` and bump its refcount on each `py_enter` call.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TestContextManager {
    /// Integer to return from `__enter__`. `None` means "return self".
    pub enter_value: Option<i64>,
    /// When `true`, `__exit__` returns `Value::Bool(true)` on the exception
    /// path so the in-flight exception is swallowed. Has no effect on the
    /// normal-exit path (CPython only checks the return value when an
    /// exception is propagating).
    pub suppress: bool,
    /// When `Some`, `__enter__` raises `ValueError(msg)` instead of
    /// returning `enter_value`. The body never runs.
    pub raise_on_enter: Option<String>,
    /// When `Some`, `__exit__` raises `ValueError(msg)`. On the normal-exit
    /// path the new exception propagates out of the `with`; on the
    /// exception path it replaces the in-flight exception.
    pub raise_on_exit: Option<String>,
}

impl TestContextManager {
    /// Builds a passthrough manager. Tests mutate the result to flip the
    /// branches they want to exercise — keeping the constructor zero-arg
    /// means the `_test_cm()` builtin doesn't need to know which fields
    /// exist, only which kwargs to forward.
    #[must_use]
    pub fn new() -> Self {
        Self {
            enter_value: None,
            suppress: false,
            raise_on_enter: None,
            raise_on_exit: None,
        }
    }
}

impl Default for TestContextManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HeapItem for TestContextManager {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
            + self.raise_on_enter.as_ref().map_or(0, String::len)
            + self.raise_on_exit.as_ref().map_or(0, String::len)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // The manager holds no heap references — `enter_value` is a plain
        // i64 and the strings are owned Rust data — so cycle collection
        // and ref-count drops have nothing to do here.
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, TestContextManager> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::TestContextManager
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, _other: &Self, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<bool> {
        Ok(false)
    }

    fn py_bool(&self, _vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        f.write_str("<_test_cm>")?;
        Ok(())
    }

    fn py_is_context_manager(&self) -> bool {
        true
    }

    fn py_enter(&mut self, self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<CallResult> {
        let cfg = self.get(vm.heap);
        if let Some(msg) = cfg.raise_on_enter.clone() {
            return Err(SimpleException::new_msg(ExcType::ValueError, msg).into());
        }
        let value = if let Some(n) = cfg.enter_value {
            Value::Int(n)
        } else {
            vm.heap.inc_ref(self_id);
            Value::Ref(self_id)
        };
        Ok(CallResult::Value(value))
    }

    fn py_exit(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        exc: Option<HeapId>,
    ) -> RunResult<CallResult> {
        let cfg = self.get(vm.heap);
        if let Some(msg) = cfg.raise_on_exit.clone() {
            return Err(SimpleException::new_msg(ExcType::ValueError, msg).into());
        }
        // Only the exception path consults the suppress flag — on the
        // normal-exit path CPython ignores the return value entirely, but
        // we still return `Bool(false)` there to keep the implementation
        // uniform and the user-visible value testable.
        let value = if exc.is_some() && cfg.suppress {
            Value::Bool(true)
        } else {
            Value::None
        };
        Ok(CallResult::Value(value))
    }
}
