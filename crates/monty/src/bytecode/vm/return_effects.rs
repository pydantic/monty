//! Work the VM performs on a returning frame's value before the caller sees it.
//!
//! Native code that calls back into Python sometimes needs the result of a call
//! it did not make: `Foo(...)` wants the instance rather than `__init__`'s
//! `None`, and a `functools.lru_cache` wrapper wants to store what its caller
//! receives. Both park the values they need on the caller's operand stack and
//! record a [`ReturnEffects`] on the frame they push; the
//! [`ReturnValue`](crate::bytecode::Opcode::ReturnValue) handler applies it on
//! the way out.
//!
//! Parking is what keeps this cheap and safe: the operands are ordinary stack
//! values, so an unwind, a task teardown or an abandoned snapshot releases them
//! through the drain it already performs, and a snapshot carries them with the
//! stack it already serializes. Nothing here owns a heap reference, so no path
//! needs bespoke cleanup.

use serde::{Deserialize, Serialize};

use super::VM;
use crate::{exception_private::RunResult, types::lru_cache::store_result, value::Value};

/// What the VM owes a returning frame's value, using operands parked on the
/// caller's operand stack below the frame's `stack_base`.
///
/// Plain `Copy` data, and deliberately not an enum: the effects compose —
/// `functools.cache(Foo)` sets both — and [`apply`](Self::apply) owns the order
/// they run in, which is the invariant a match arm per combination would leave
/// to its author.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct ReturnEffects {
    /// `Foo(...)`: `__init__` yields `None`, so the parked instance is the result.
    instance: bool,
    /// Parked `(cache, key)` pairs whose cache takes the result, one per stacked
    /// `lru_cache` wrapper. A chain deeper than `MAX_RUN_REENTRY_DEPTH` raises
    /// before it can park, so this cannot overflow.
    cache_stores: u8,
}

impl ReturnEffects {
    /// Whether the return path can hand the value straight to the caller.
    ///
    /// Checked per return on the hot path, so the ordinary case is one branch.
    pub(super) fn is_empty(self) -> bool {
        !self.instance && self.cache_stores == 0
    }

    /// Records that the constructed instance is parked for `Foo(...)`.
    pub(super) fn set_instance(&mut self) {
        self.instance = true;
    }

    /// Records one parked `(cache, key)` pair awaiting this frame's result.
    pub(super) fn push_cache_store(&mut self) {
        self.cache_stores += 1;
    }

    /// Resolves `value` into what the call that pushed this frame yields.
    ///
    /// The instance resolves first: what a cached call stores must be what its
    /// caller receives, so a cached `Foo(...)` stores the instance and not
    /// `__init__`'s `None`. Consumes `value`, releasing it if an effect fails.
    ///
    /// Operands left parked by a failing effect stay on the stack for the
    /// unwind to drain, which stores nothing — as a raising call should.
    pub(super) fn apply(self, value: Value, vm: &mut VM<'_>) -> RunResult<Value> {
        let value = if self.instance {
            vm.take_initializer_result(value)?
        } else {
            value
        };
        // Parked innermost pair on top, so the wrappers store in the reverse of
        // the order they were entered. They hold distinct caches keyed on the
        // same call, so the order is not observable.
        for _ in 0..self.cache_stores {
            let key = vm.pop();
            let cache = vm.pop();
            if let Err(err) = store_result(cache, key, &value, vm) {
                value.drop_with(vm);
                return Err(err);
            }
        }
        Ok(value)
    }
}
