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
//! A stored token can't be released through the heap alone (the heap has no path
//! back to the VM counter), so its [`DropWithContext`] impl is bound by
//! [`ContainsVM`] rather than [`ContainsHeap`], and it is cleaned up through the
//! same `defer_drop!` machinery as any other value.

use std::ops::{Deref, DerefMut};

use super::VM;
use crate::{
    heap::{ContainsHeap, DropWithContext},
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
    /// via [`DropWithContext`] rather than tied to a lexical scope.
    ///
    /// Unlike [`recursion_guard`](Self::recursion_guard), the token does not
    /// borrow the VM, so it can be stored (e.g. inside a container iterator) and
    /// released later with `defer_drop!`.
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

/// Zero-size reservation of one recursion level, returned by [`VM::incr_recursion`].
///
/// Released via [`DropWithContext`] (it cannot reach the VM counter through the heap).
/// Stored by container iterators so the depth bound is owned by the iterator and
/// released on every exit path via `defer_drop!`.
pub(crate) struct RecursionToken(());

/// Accessor for the [`VM`] behind a cleanup context — the VM-capable extension of
/// [`ContainsHeap`](crate::heap::ContainsHeap).
///
/// Implemented by the [`VM`] itself and by wrappers that own a `&mut VM` (the json
/// `Encoder`). A [`DropWithContext`] impl bounds its context `C` by `ContainsVM`
/// (rather than just `ContainsHeap`) when it must reach the VM-side recursion
/// counter — e.g. dropping a [`RecursionToken`] — while a wrapper like the encoder
/// stays borrowable through the same handle. Because `ContainsVM: ContainsHeap`,
/// such a context can still drop plain heap fields via `drop_with(ctx)`.
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

/// A [`RecursionToken`] releases its reserved level through any [`ContainsVM`]
/// context. The `C: ContainsVM<'h>` bound (rather than `ContainsHeap`) is what
/// confines token cleanup to a `VM`/`Encoder` — a bare heap cannot reach the
/// counter — and there is no overlap with the heap-only impls because those are
/// for different `Self` types.
impl<'h, C: ContainsVM<'h>> DropWithContext<C> for RecursionToken {
    fn drop_with(self, ctx: &mut C) {
        ctx.vm().decr_recursion();
    }
}
