//! Iterator types produced by the `itertools` module.
//!
//! Each callable gets its own struct in its own file, but the family shares one
//! `HeapData::Itertools(Box<ItertoolsIter>)` variant: nothing outside this
//! module dispatches on *which* adaptor an iterator is. Python-visible
//! distinctions (`type()` name, error messages) live in [`Type`] instead.
//!
//! - Missing GC wiring is a compile error, not a leak — [`ItertoolsIter`]'s
//!   walkers are exhaustive with no wildcard, unlike `heap.rs`'s `_ => {}`.
//! - `py_next` cannot hold the state borrow: adaptors re-enter the VM, so each
//!   per-type function takes the `HeapRead` and re-projects under short borrows.

pub mod count;
pub mod repeat;

use std::{fmt::Write, mem};

pub(crate) use count::Count;
pub(crate) use repeat::Repeat;
use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    heap::{HeapId, HeapItem, HeapRead},
    types::{LazyHeapSet, PyTrait, Type},
    value::Value,
};

/// The state of one `itertools` iterator, whichever adaptor produced it.
///
/// Boxed inside `HeapData` so adding adaptors never grows the arena entry.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ItertoolsIter {
    Count(Count),
    Repeat(Repeat),
}

/// Which adaptor an [`ItertoolsIter`] is, without borrowing it.
///
/// `py_next` and friends need the variant to pick a per-type function, but they
/// then need `&mut` access — so they read this `Copy` tag under a short borrow
/// and let it end before dispatching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Count,
    Repeat,
}

impl ItertoolsIter {
    /// The adaptor tag, for dispatch without holding a borrow.
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Count(_) => Kind::Count,
            Self::Repeat(_) => Kind::Repeat,
        }
    }

    /// The Python type this adaptor reports — the dotted CPython `tp_name`.
    pub(crate) fn py_type(&self) -> Type {
        match self {
            Self::Count(_) => Type::ItertoolsCount,
            Self::Repeat(_) => Type::ItertoolsRepeat,
        }
    }

    /// Whether this adaptor can take part in a reference cycle.
    ///
    /// Answered per adaptor rather than for the family: `count` holds only
    /// numbers, so tracking it would cost GC work for a type that can never
    /// close a cycle.
    pub(crate) fn is_gc_tracked(&self) -> bool {
        match self {
            // Only ever holds numbers, whose refs point at `LongInt` leaves.
            Self::Count(_) => false,
            // Holds an arbitrary object, which may reach back to the iterator.
            Self::Repeat(_) => true,
        }
    }

    /// Values not yet yielded, or `0` when unbounded or unknown.
    ///
    /// Drives preallocation only, so an infinite adaptor MUST report `0` rather
    /// than a guess (see `checked_preallocation_hint`).
    pub(crate) fn size_hint(&self) -> usize {
        match self {
            Self::Count(_) => 0,
            Self::Repeat(repeat) => repeat.size_hint(),
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    ///
    /// Exhaustive with no wildcard: a new adaptor must state its edges.
    pub(crate) fn for_each_child_id(&self, on_child: impl FnMut(HeapId)) {
        match self {
            Self::Count(count) => count.for_each_child_id(on_child),
            Self::Repeat(repeat) => repeat.for_each_child_id(on_child),
        }
    }
}

impl HeapItem for ItertoolsIter {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    /// Exhaustive with no wildcard, for the same reason as `for_each_child_id`;
    /// the two must stay in sync.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Count(count) => count.py_dec_ref_ids(stack),
            Self::Repeat(repeat) => repeat.py_dec_ref_ids(stack),
        }
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, ItertoolsIter> {
    fn py_is_iterator(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).py_type()
    }

    /// `None` for every adaptor: CPython exposes remaining counts through
    /// `__length_hint__`, never `__len__`.
    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        let self_id = self_id.expect("heap values have an id");
        vm.heap.inc_ref(self_id);
        Ok(Value::Ref(self_id))
    }

    fn py_next(&mut self, _: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match self.get(vm.heap).kind() {
            Kind::Count => count::next(self, vm),
            Kind::Repeat => repeat::next(self, vm),
        }
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        match self.get(vm.heap).kind() {
            Kind::Count => count::repr_fmt(self, f, vm, heap_ids),
            Kind::Repeat => repeat::repr_fmt(self, f, vm, heap_ids),
        }
    }
}
