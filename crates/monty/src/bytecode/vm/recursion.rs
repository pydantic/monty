//! Recursion-depth tracking for the [`VM`].
//!
//! The recursion counter lives on the `VM` (not the heap) because every site
//! that charges depth — function-call frames, container `repr`/`eq`/`cmp`/`hash`,
//! `isinstance`, json encoding — has a `&mut VM` in scope. Two primitives charge
//! a level:
//!
//! - [`VM::recursion_guard`] for lexically-scoped recursion: an RAII guard that
//!   derefs to the VM and releases the level on drop (every path, incl. `?`).
//! - [`VM::incr_recursion`] for reservations that must outlive a lexical scope —
//!   notably the container iterators, which store the [`RecursionToken`] so the
//!   bound is owned by the iterator and a caller cannot forget to charge it.
//!
//! A stored token can't be released through [`DropWithHeap`](crate::heap::DropWithHeap)
//! (the heap has no path back to the VM counter), so it uses the parallel
//! [`DropWithVM`] / [`ContainsVM`] machinery and the `defer_drop_vm!` macros.

use std::{
    marker::PhantomData,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
};

use super::VM;
use crate::{
    heap::ContainsHeap,
    resource::{ResourceError, ResourceTracker},
};

impl<'h, T: ResourceTracker> VM<'h, T> {
    /// Enters a lexically-scoped recursive operation, returning a guard that
    /// releases the depth level when dropped.
    ///
    /// The guard derefs to the VM, so recursive calls run through `&mut *guard`:
    ///
    /// ```ignore
    /// let mut guard = vm.recursion_guard()?;
    /// let vm = &mut *guard;
    /// // ... recurse through `vm`; the level is released when `guard` drops ...
    /// ```
    ///
    /// Returns `Err(ResourceError::Recursion)` if the limit would be exceeded.
    pub(crate) fn recursion_guard(&mut self) -> Result<RecursionGuard<'_, 'h, T>, ResourceError> {
        self.incr_recursion()?;
        Ok(RecursionGuard { vm: self })
    }

    /// Reserves one recursion level as a standalone [`RecursionToken`], released
    /// via [`DropWithVM`] rather than tied to a lexical scope.
    ///
    /// Unlike [`recursion_guard`](Self::recursion_guard), the token does not
    /// borrow the VM, so it can be stored (e.g. inside a container iterator) and
    /// released later with `defer_drop_vm!`.
    pub(crate) fn recursion_token(&mut self) -> Result<RecursionToken, ResourceError> {
        self.incr_recursion()?;
        Ok(RecursionToken(()))
    }

    /// Checks the recursion limit against the heap's tracker and increments the
    /// depth counter.
    #[inline]
    pub(crate) fn incr_recursion(&mut self) -> Result<(), ResourceError> {
        self.heap.tracker().check_recursion_depth(self.recursion_depth)?;
        self.recursion_depth += 1;
        Ok(())
    }

    /// Releases one recursion level. Paired with [`charge_recursion`](Self::charge_recursion);
    /// called by the guard/token cleanup and by `pop_frame`.
    #[inline]
    pub(crate) fn decr_recursion(&mut self) {
        debug_assert!(self.recursion_depth > 0, "decr_recursion called when depth is 0");
        self.recursion_depth -= 1;
    }
}

/// RAII guard for a lexically-scoped recursion level (see [`VM::recursion_guard`]).
///
/// Derefs to the [`VM`] so recursive operations run through the guard; the
/// reserved level is released when the guard is dropped on any code path.
pub(crate) struct RecursionGuard<'a, 'h, T: ResourceTracker> {
    vm: &'a mut VM<'h, T>,
}

impl<'h, T: ResourceTracker> Deref for RecursionGuard<'_, 'h, T> {
    type Target = VM<'h, T>;
    fn deref(&self) -> &Self::Target {
        self.vm
    }
}

impl<T: ResourceTracker> DerefMut for RecursionGuard<'_, '_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.vm
    }
}

impl<T: ResourceTracker> Drop for RecursionGuard<'_, '_, T> {
    fn drop(&mut self) {
        self.vm.decr_recursion();
    }
}

/// Hard cap on native Rust call-stack re-entry, enforced by
/// [`VM::enter_run_reentry`] and released by [`RunReentryGuard`]. Much smaller
/// than the 1000-frame Python recursion limit because each level costs a real
/// nested call to [`VM::run`], not a push onto the heap-allocated `frames` vec.
///
/// Tuned conservatively from the smallest native stack observed while fixing
/// recursive callback crashes. A debug monty-datatest worker with an ~2 MiB
/// stack crashed at depth 19 on macOS/arm64; keep this much lower and
/// revalidate on all supported host targets before raising it.
///
/// A hard safety constant, not a Python-visible setting: unlike ordinary
/// recursion depth, it does not go through the tracker and cannot be changed
/// by sandboxed code or test hooks.
// TODO set this value to custom values per-OS/arch
pub(crate) const MAX_RUN_REENTRY_DEPTH: u8 = 12;

impl<T: ResourceTracker> VM<'_, T> {
    /// Charges one native re-entry level, counting the remaining budget down
    /// from [`MAX_RUN_REENTRY_DEPTH`]; errors when it's exhausted. Pair with
    /// [`RunReentryGuard::new`] to release the level on every exit path.
    ///
    /// Split into a plain `Result<(), _>` check (unlike
    /// [`recursion_guard`](Self::recursion_guard)'s combined check-and-wrap) so
    /// `evaluate_function` can `if let Err(e) = ...` it and run its own cleanup
    /// (dropping owned arguments) without a `Drop` guard extending `self`'s
    /// borrow across the match. Bypasses the tracker: a fixed safety constant,
    /// not a user-configurable limit.
    #[inline]
    pub(crate) fn enter_run_reentry(&mut self) -> Result<(), ResourceError> {
        if let Some(new_value) = self.run_reentry_depth.checked_sub(1) {
            self.run_reentry_depth = new_value;
            Ok(())
        } else {
            Err(ResourceError::Recursion {
                limit: MAX_RUN_REENTRY_DEPTH as usize,
                depth: MAX_RUN_REENTRY_DEPTH as usize,
            })
        }
    }

    /// Releases one native re-entry level. Paired with
    /// [`enter_run_reentry`](Self::enter_run_reentry); called only by
    /// [`RunReentryGuard`]'s `Drop` impl.
    #[inline]
    pub(crate) fn release_run_reentry(&mut self) {
        debug_assert!(
            self.run_reentry_depth < MAX_RUN_REENTRY_DEPTH,
            "release_run_reentry called when depth is MAX_RUN_REENTRY_DEPTH"
        );
        self.run_reentry_depth += 1;
    }
}

/// RAII guard for one level of native `run()` re-entry, wrapping a level
/// already reserved via [`VM::enter_run_reentry`].
///
/// Derefs to the [`VM`] so the nested `call_function`/`run()` call runs
/// through the guard; the reserved level is released when the guard is
/// dropped on any code path (normal return, `?`, or early return).
pub(crate) struct RunReentryGuard<'a, 'h, T: ResourceTracker> {
    vm: &'a mut VM<'h, T>,
}

impl<'a, 'h, T: ResourceTracker> RunReentryGuard<'a, 'h, T> {
    /// Wraps a re-entry level already charged by a prior
    /// [`VM::enter_run_reentry`] call.
    pub(crate) fn new(vm: &'a mut VM<'h, T>) -> Self {
        Self { vm }
    }
}

impl<'h, T: ResourceTracker> Deref for RunReentryGuard<'_, 'h, T> {
    type Target = VM<'h, T>;
    fn deref(&self) -> &Self::Target {
        self.vm
    }
}

impl<T: ResourceTracker> DerefMut for RunReentryGuard<'_, '_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.vm
    }
}

impl<T: ResourceTracker> Drop for RunReentryGuard<'_, '_, T> {
    fn drop(&mut self) {
        self.vm.release_run_reentry();
    }
}

/// Zero-size reservation of one recursion level, returned by [`VM::incr_recursion`].
///
/// Released via [`DropWithVM`] (it cannot reach the VM counter through the heap).
/// Stored by container iterators so the depth bound is owned by the iterator and
/// released on every exit path via `defer_drop_vm!`.
pub(crate) struct RecursionToken(());

/// Types that can release a recursion level — the VM analogue of
/// [`ContainsHeap`](crate::heap::ContainsHeap), which it extends so that a
/// [`DropWithVM`] value can drop its plain heap fields through the same handle.
///
/// Implemented by the [`VM`] itself and by wrappers that own a `&mut VM` (the
/// json `Encoder`), so a token released through such a wrapper still reaches the
/// counter while the wrapper stays borrowable.
pub(crate) trait ContainsVM<'h>: ContainsHeap {
    // `+ 'h` because `VM<'h, T>` is only well-formed when its tracker outlives the
    // brand; making it part of the associated-type bound means callers of `vm()`
    // get `Self::Tracker: 'h` for free instead of having to prove it.
    type Tracker: ResourceTracker + 'h;
    fn vm(&mut self) -> &mut VM<'h, Self::Tracker>;
}

impl<'h, T: ResourceTracker> ContainsVM<'h> for VM<'h, T> {
    type Tracker = T;
    fn vm(&mut self) -> &mut VM<'h, Self::Tracker> {
        self
    }
}

/// Cleanup for values holding a VM-side reservation that can't be released
/// through the heap alone (a [`RecursionToken`], or an iterator holding one).
///
/// The VM analogue of [`DropWithHeap`](crate::heap::DropWithHeap); use
/// `defer_drop_vm!` / `defer_drop_vm_mut!` to guarantee cleanup on every path.
/// There is deliberately no blanket impl over `DropWithHeap` (it would overlap
/// the `RecursionToken` impl under coherence) — implementers drop their plain
/// heap fields via `drop_with_heap(container)` directly (`ContainsVM: ContainsHeap`).
pub(crate) trait DropWithVM<'h>: Sized {
    fn drop_with_vm(self, container: &mut impl ContainsVM<'h>);
}

impl<'h> DropWithVM<'h> for RecursionToken {
    fn drop_with_vm(self, container: &mut impl ContainsVM<'h>) {
        container.vm().decr_recursion();
    }
}

/// RAII guard ensuring a [`DropWithVM`] value is released on every code path —
/// the VM analogue of [`HeapGuard`](crate::heap::HeapGuard).
///
/// Prefer the `defer_drop_vm!` / `defer_drop_vm_mut!` macros; use the guard
/// directly only when you need to reclaim the value via [`as_parts_mut`](Self::as_parts_mut).
pub(crate) struct VmGuard<'a, 'h, C: ContainsVM<'h>, V: DropWithVM<'h>> {
    value: ManuallyDrop<V>,
    container: &'a mut C,
    phantom: PhantomData<&'a mut VM<'h, C::Tracker>>,
}

impl<'a, 'h, C: ContainsVM<'h>, V: DropWithVM<'h>> VmGuard<'a, 'h, C, V> {
    #[inline]
    pub(crate) fn new(value: V, container: &'a mut C) -> Self {
        Self {
            value: ManuallyDrop::new(value),
            container,
            phantom: PhantomData,
        }
    }

    /// Borrows the value (immutably) and container (mutably) — backs `defer_drop_vm!`.
    #[inline]
    pub(crate) fn as_parts(&mut self) -> (&V, &mut C) {
        (&self.value, self.container)
    }

    /// Borrows the value (mutably) and container (mutably) — backs `defer_drop_vm_mut!`.
    #[inline]
    pub(crate) fn as_parts_mut(&mut self) -> (&mut V, &mut C) {
        (&mut self.value, self.container)
    }
}

impl<'h, C: ContainsVM<'h>, V: DropWithVM<'h>> Drop for VmGuard<'_, 'h, C, V> {
    fn drop(&mut self) {
        // SAFETY: `value` is wrapped in `ManuallyDrop` and never otherwise taken
        // before this point, so this is the unique move-out.
        unsafe { ManuallyDrop::take(&mut self.value) }.drop_with_vm(self.container);
    }
}

/// Like [`defer_drop!`](crate::defer_drop), but for [`DropWithVM`] values released
/// through a [`ContainsVM`] (e.g. a [`RecursionToken`] held alongside a json
/// `Encoder`). Rebinds `$value` as `&V` and `$container` as `&mut C`.
#[macro_export]
macro_rules! defer_drop_vm {
    ($value:ident, $container:ident) => {
        let mut _vm_guard = $crate::bytecode::VmGuard::new($value, $container);
        #[allow(
            clippy::allow_attributes,
            reason = "the reborrowed parts may not both be used in every case, so allow unused vars to avoid warnings"
        )]
        #[allow(unused_variables)]
        let ($value, $container) = _vm_guard.as_parts();
    };
}

/// Like [`defer_drop_vm!`], but rebinds `$value` as `&mut V` — for iterators that
/// store a [`RecursionToken`] and are advanced in place (`next`/`next_with_index`).
#[macro_export]
macro_rules! defer_drop_vm_mut {
    ($value:ident, $container:ident) => {
        let mut _vm_guard = $crate::bytecode::VmGuard::new($value, $container);
        #[allow(
            clippy::allow_attributes,
            reason = "the reborrowed parts may not both be used in every case, so allow unused vars to avoid warnings"
        )]
        #[allow(unused_variables)]
        let ($value, $container) = _vm_guard.as_parts_mut();
    };
}
