use std::{fmt::Write, sync::Arc};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    hash::{HashValue, identity_hash},
    heap::HeapObjectRead,
    types::{LazyHeapSet, PyTrait, Type},
    value::{EitherStr, Value},
};

/// A host-provided callable identified by its external lookup name.
///
/// The name allocation is shared with the heap's weak external-function cache,
/// which preserves identity while any function with this name remains live.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ExtFunction(Arc<str>);

impl ExtFunction {
    /// Creates an external function with an owned, shareable lookup name.
    pub(crate) fn new(name: &str) -> Self {
        Self(Arc::from(name))
    }

    /// Returns the function name as text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the shared name used as the weak-cache key.
    pub(crate) fn cache_key(&self) -> Arc<str> {
        Arc::clone(&self.0)
    }

    /// Clones the function name for an external call suspension.
    pub(crate) fn clone_name(&self) -> EitherStr {
        self.0.to_string().into()
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ExtFunction> {
    /// Indistinguishable from a sandbox `def` to Python code, which is the
    /// point: a host function is passed around like any other function.
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Function
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Identity only. Two lookups of the same name share one heap entry (the
    /// weak external-function cache), so `is` already answers this.
    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_bool(&self, _: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<function '{}' external>", self.get(vm.heap).as_str())?)
    }

    /// Suspends: only the host can run this, so the VM yields the name and the
    /// arguments and waits to be resumed with the result.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::External {
            name: self.get(vm.heap).clone_name(),
            args,
            is_async: false,
        })
    }
}

/// A host function whose calls start without waiting for the host's answer.
///
/// Created from a host value marked async. A call still yields an external
/// call, flagged `is_async`, and a worker that supports it registers the
/// future itself and streams the call to the host instead of suspending.
/// It is a separate heap type, not a flag on [`ExtFunction`], so dumps
/// written before it existed still load. Identity follows the same weak cache
/// rules as `ExtFunction`, keyed separately so a sync and an async function
/// with one name stay distinct.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct AsyncExtFunction(ExtFunction);

impl AsyncExtFunction {
    /// Creates an async external function with an owned, shareable lookup name.
    pub(crate) fn new(name: &str) -> Self {
        Self(ExtFunction::new(name))
    }

    /// Returns the function name as text.
    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Returns the shared name used as the weak-cache key.
    pub(crate) fn cache_key(&self) -> Arc<str> {
        self.0.cache_key()
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, AsyncExtFunction> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Function
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Identity only, as for [`ExtFunction`].
    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_bool(&self, _: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<function '{}' external>", self.get(vm.heap).as_str())?)
    }

    /// Yields an external call flagged async; the worker decides whether to
    /// dispatch it without waiting or to suspend as for a sync function.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::External {
            name: self.get(vm.heap).0.clone_name(),
            args,
            is_async: true,
        })
    }
}
