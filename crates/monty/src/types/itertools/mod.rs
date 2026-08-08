//! Iterator types produced by the `itertools` module.
//!
//! Each callable gets its own struct in its own file, but the family shares one
//! `HeapData::Itertools(ItertoolsIter)` variant: nothing outside this module
//! dispatches on *which* adaptor an iterator is. Python-visible distinctions
//! (`type()` name, error messages) live in [`Type`] instead.
//!
//! - Missing GC wiring is a compile error, not a leak — [`ItertoolsIter`]'s
//!   walkers are exhaustive with no wildcard, unlike `heap.rs`'s `_ => {}`.
//! - `py_next` cannot hold the state borrow: adaptors re-enter the VM, so each
//!   per-type function takes the `HeapRead` and re-projects under short borrows.

pub mod chain;
pub mod compress;
pub mod count;
pub mod cycle;
pub mod islice;
pub mod pairwise;
pub mod repeat;

use std::fmt::Write;

pub(crate) use chain::Chain;
pub(crate) use compress::Compress;
pub(crate) use count::Count;
pub(crate) use cycle::Cycle;
pub(crate) use islice::Islice;
pub(crate) use pairwise::Pairwise;
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
/// Held inline: `HeapData` is 160 bytes and the widest adaptor needs 56. If one
/// ever exceeds that, box it in its variant here — never at the `HeapData`
/// boundary, where `heap_read_boxed` is only sound for reads.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ItertoolsIter {
    Count(Count),
    Repeat(Repeat),
    Pairwise(Pairwise),
    Compress(Compress),
    Islice(Islice),
    Chain(Chain),
    Cycle(Cycle),
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
    Pairwise,
    Compress,
    Islice,
    Chain,
    Cycle,
}

impl ItertoolsIter {
    /// The adaptor tag, for dispatch without holding a borrow.
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Count(_) => Kind::Count,
            Self::Repeat(_) => Kind::Repeat,
            Self::Pairwise(_) => Kind::Pairwise,
            Self::Compress(_) => Kind::Compress,
            Self::Islice(_) => Kind::Islice,
            Self::Chain(_) => Kind::Chain,
            Self::Cycle(_) => Kind::Cycle,
        }
    }

    /// The Python type this adaptor reports — the dotted CPython `tp_name`.
    pub(crate) fn py_type(&self) -> Type {
        match self {
            Self::Count(_) => Type::ItertoolsCount,
            Self::Repeat(_) => Type::ItertoolsRepeat,
            Self::Pairwise(_) => Type::ItertoolsPairwise,
            Self::Compress(_) => Type::ItertoolsCompress,
            Self::Islice(_) => Type::ItertoolsIslice,
            Self::Chain(_) => Type::ItertoolsChain,
            Self::Cycle(_) => Type::ItertoolsCycle,
        }
    }

    /// Whether this adaptor can take part in a reference cycle — answered per
    /// adaptor rather than for the family.
    pub(crate) fn is_gc_tracked(&self) -> bool {
        match self {
            // Only ever holds numbers, whose refs point at `LongInt` leaves.
            Self::Count(_) => false,
            // Both hold arbitrary objects, which may reach back to the iterator.
            Self::Repeat(_)
            | Self::Pairwise(_)
            | Self::Compress(_)
            | Self::Islice(_)
            | Self::Chain(_)
            | Self::Cycle(_) => true,
        }
    }

    /// Values not yet yielded, or `0` when unbounded or unknown.
    ///
    /// Drives preallocation only, so an infinite adaptor MUST report `0` rather
    /// than a guess (see `checked_preallocation_hint`).
    pub(crate) fn size_hint(&self) -> usize {
        match self {
            Self::Count(_)
            | Self::Pairwise(_)
            | Self::Compress(_)
            | Self::Islice(_)
            | Self::Chain(_)
            | Self::Cycle(_) => 0,
            Self::Repeat(repeat) => repeat.size_hint(),
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, on_child: impl FnMut(HeapId)) {
        match self {
            Self::Count(count) => count.for_each_child_id(on_child),
            Self::Repeat(repeat) => repeat.for_each_child_id(on_child),
            Self::Pairwise(pairwise) => pairwise.for_each_child_id(on_child),
            Self::Compress(compress) => compress.for_each_child_id(on_child),
            Self::Islice(islice) => islice.for_each_child_id(on_child),
            Self::Chain(chain) => chain.for_each_child_id(on_child),
            Self::Cycle(cycle) => cycle.for_each_child_id(on_child),
        }
    }
}

impl HeapItem for ItertoolsIter {
    /// Mirrors [`ItertoolsIter::for_each_child_id`] — the two must stay in sync.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Count(count) => count.py_dec_ref_ids(stack),
            Self::Repeat(repeat) => repeat.py_dec_ref_ids(stack),
            Self::Pairwise(pairwise) => pairwise.py_dec_ref_ids(stack),
            Self::Compress(compress) => compress.py_dec_ref_ids(stack),
            Self::Islice(islice) => islice.py_dec_ref_ids(stack),
            Self::Chain(chain) => chain.py_dec_ref_ids(stack),
            Self::Cycle(cycle) => cycle.py_dec_ref_ids(stack),
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
        let kind = self.get(vm.heap).kind();
        match kind {
            // Self-contained adaptors: neither drives a wrapped iterator.
            Kind::Count => count::next(self, vm),
            Kind::Repeat => repeat::next(self, vm),
            // Source-driving adaptors re-enter `py_next` on their wrapped
            // iterator, recursing on the native Rust stack; charge a recursion
            // level so deep nesting raises `RecursionError`, not a stack overflow.
            Kind::Pairwise | Kind::Compress | Kind::Islice | Kind::Chain | Kind::Cycle => {
                let mut guard = vm.recursion_guard()?;
                let vm = &mut *guard;
                match kind {
                    Kind::Pairwise => pairwise::next(self, vm),
                    Kind::Compress => compress::next(self, vm),
                    Kind::Islice => islice::next(self, vm),
                    Kind::Chain => chain::next(self, vm),
                    Kind::Cycle => cycle::next(self, vm),
                    Kind::Count | Kind::Repeat => unreachable!("handled above"),
                }
            }
        }
    }

    /// Only `count` and `repeat` carry a custom `repr`; every other adaptor
    /// uses CPython's default `<itertools.name object>` form, which is what the
    /// `PyTrait` default writes.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        match self.get(vm.heap).kind() {
            Kind::Count => count::repr_fmt(self, f, vm, heap_ids),
            Kind::Repeat => repeat::repr_fmt(self, f, vm, heap_ids),
            Kind::Pairwise | Kind::Compress | Kind::Islice | Kind::Chain | Kind::Cycle => {
                let type_name = self.py_type(vm).name(vm.heap, vm.interns);
                Ok(write!(f, "<{type_name} object>")?)
            }
        }
    }
}
