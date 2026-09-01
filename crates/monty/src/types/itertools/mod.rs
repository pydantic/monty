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

pub mod accumulate;
pub mod batched;
pub mod chain;
pub mod compress;
pub mod count;
pub mod cycle;
pub mod dropwhile;
pub mod filterfalse;
pub mod islice;
pub mod pairwise;
pub mod repeat;
pub mod starmap;
mod step;
pub mod takewhile;
pub mod zip_longest;

use std::fmt::Write;

pub(crate) use accumulate::Accumulate;
pub(crate) use batched::Batched;
pub(crate) use chain::Chain;
pub(crate) use compress::Compress;
pub(crate) use count::Count;
pub(crate) use cycle::Cycle;
pub(crate) use dropwhile::DropWhile;
pub(crate) use filterfalse::FilterFalse;
pub(crate) use islice::Islice;
pub(crate) use pairwise::Pairwise;
pub(crate) use repeat::Repeat;
use serde::{Deserialize, Serialize};
pub(crate) use starmap::StarMap;
pub(crate) use takewhile::TakeWhile;
pub(crate) use zip_longest::ZipLongest;

// Only the 64-bit size budget below needs it.
#[cfg(target_pointer_width = "64")]
use crate::types::Dict;
use crate::{
    bytecode::VM,
    exception_private::RunResult,
    heap::{HeapId, HeapItem, HeapObjectRead},
    types::{LazyHeapSet, PyTrait, Type},
    value::Value,
};

/// The state of one `itertools` iterator, whichever adaptor produced it.
///
/// Held inline, so this width is memcpy'd on every heap allocate and free along
/// with the rest of `HeapData` — which #636 shrank to 80 bytes, asserted in
/// `heap_data.rs`. The budget below keeps the family from becoming what sets
/// that size.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ItertoolsIter {
    Count(Count),
    Repeat(Repeat),
    Pairwise(Pairwise),
    Compress(Compress),
    Islice(Islice),
    Chain(Chain),
    Cycle(Cycle),
    TakeWhile(TakeWhile),
    DropWhile(DropWhile),
    FilterFalse(FilterFalse),
    StarMap(StarMap),
    /// Boxed: three `Value`s make it 56 bytes, wide enough to set `HeapData`'s
    /// size on 32-bit. See the budget below.
    Accumulate(Box<Accumulate>),
    Batched(Batched),
    ZipLongest(ZipLongest),
}

// `Dict` is the widest `HeapData` payload on 64-bit hosts, so it — not a
// literal — is the budget: staying under it keeps this family from setting
// `HeapData`'s size. Only there: on 32-bit (the wasm worker) `Dict` halves
// while the adaptors' `i64` fields do not, and other variants set the size.
// TODO: when this fails, box the offending variant (`GroupBy(Box<GroupBy>)`),
// not the enum and not at the `HeapData` boundary.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<ItertoolsIter>() <= size_of::<Dict>());

// On 32-bit `Dict` is 36 bytes while these adaptors, built from `Value`s that
// stay 16 bytes either way, do not shrink — so this family *is* what sets
// `HeapData`'s size and no other variant can serve as the budget. 48 is the
// width already paid; every byte past it costs `PAGE_SIZE` more per heap page.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(size_of::<ItertoolsIter>() <= 48);

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
    TakeWhile,
    DropWhile,
    FilterFalse,
    StarMap,
    Accumulate,
    Batched,
    ZipLongest,
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
            Self::TakeWhile(_) => Kind::TakeWhile,
            Self::DropWhile(_) => Kind::DropWhile,
            Self::FilterFalse(_) => Kind::FilterFalse,
            Self::StarMap(_) => Kind::StarMap,
            Self::Accumulate(_) => Kind::Accumulate,
            Self::Batched(_) => Kind::Batched,
            Self::ZipLongest(_) => Kind::ZipLongest,
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
            Self::TakeWhile(_) => Type::ItertoolsTakeWhile,
            Self::DropWhile(_) => Type::ItertoolsDropWhile,
            Self::FilterFalse(_) => Type::ItertoolsFilterFalse,
            Self::StarMap(_) => Type::ItertoolsStarMap,
            Self::Accumulate(_) => Type::ItertoolsAccumulate,
            Self::Batched(_) => Type::ItertoolsBatched,
            Self::ZipLongest(_) => Type::ItertoolsZipLongest,
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
            | Self::Cycle(_)
            | Self::TakeWhile(_)
            | Self::DropWhile(_)
            | Self::FilterFalse(_)
            | Self::StarMap(_)
            | Self::Accumulate(_)
            | Self::Batched(_)
            | Self::ZipLongest(_) => true,
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
            | Self::Cycle(_)
            | Self::TakeWhile(_)
            | Self::DropWhile(_)
            | Self::FilterFalse(_)
            | Self::StarMap(_)
            | Self::Accumulate(_)
            | Self::Batched(_)
            | Self::ZipLongest(_) => 0,
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
            Self::TakeWhile(take) => take.for_each_child_id(on_child),
            Self::DropWhile(drop_while) => drop_while.for_each_child_id(on_child),
            Self::FilterFalse(filter) => filter.for_each_child_id(on_child),
            Self::StarMap(starmap) => starmap.for_each_child_id(on_child),
            Self::Accumulate(accumulate) => accumulate.for_each_child_id(on_child),
            Self::Batched(batched) => batched.for_each_child_id(on_child),
            Self::ZipLongest(zip) => zip.for_each_child_id(on_child),
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
            Self::TakeWhile(take) => take.py_dec_ref_ids(stack),
            Self::DropWhile(drop_while) => drop_while.py_dec_ref_ids(stack),
            Self::FilterFalse(filter) => filter.py_dec_ref_ids(stack),
            Self::StarMap(starmap) => starmap.py_dec_ref_ids(stack),
            Self::Accumulate(accumulate) => accumulate.py_dec_ref_ids(stack),
            Self::Batched(batched) => batched.py_dec_ref_ids(stack),
            Self::ZipLongest(zip) => zip.py_dec_ref_ids(stack),
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ItertoolsIter> {
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

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(self.clone_value(vm.heap))
    }

    /// Recursion is charged by [`step::next_source`], not here: the level is
    /// owed by re-entering a wrapped iterator on the native Rust stack, and an
    /// adaptor that answers from its own state never does. Charging it up front
    /// made a spent `batched`, a latched `takewhile` or an `accumulate` yielding
    /// its `initial` cost a level it never spent.
    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match self.get(vm.heap).kind() {
            Kind::Count => count::next(self, vm),
            Kind::Repeat => repeat::next(self, vm),
            Kind::Pairwise => pairwise::next(self, vm),
            Kind::Compress => compress::next(self, vm),
            Kind::Islice => islice::next(self, vm),
            Kind::Chain => chain::next(self, vm),
            Kind::Cycle => cycle::next(self, vm),
            Kind::TakeWhile => takewhile::next(self, vm),
            Kind::DropWhile => dropwhile::next(self, vm),
            Kind::FilterFalse => filterfalse::next(self, vm),
            Kind::StarMap => starmap::next(self, vm),
            Kind::Accumulate => accumulate::next(self, vm),
            Kind::Batched => batched::next(self, vm),
            Kind::ZipLongest => zip_longest::next(self, vm),
        }
    }

    /// Only `count` and `repeat` carry a custom `repr`; every other adaptor
    /// uses Python's identity-bearing default object representation.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        match self.get(vm.heap).kind() {
            Kind::Count => count::repr_fmt(self, f, vm, heap_ids),
            Kind::Repeat => repeat::repr_fmt(self, f, vm, heap_ids),
            Kind::Pairwise
            | Kind::Compress
            | Kind::Islice
            | Kind::Chain
            | Kind::Cycle
            | Kind::TakeWhile
            | Kind::DropWhile
            | Kind::FilterFalse
            | Kind::StarMap
            | Kind::Accumulate
            | Kind::Batched
            | Kind::ZipLongest => self.py_default_repr_fmt(f, vm),
        }
    }
}
