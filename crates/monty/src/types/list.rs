use std::{cmp::Ordering, fmt::Write, mem, ptr};

mod bitmap;
mod packed;
mod storage;

use monty_types::ResourceError;
use num_traits::ToPrimitive;
pub(super) use packed::{BoolView, PackedView};
use packed::{NumericSliceSnapshot, PackedList, Packing};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use storage::ListStorage;

use super::{
    CmpOrder, PyTrait,
    iter::{collect_owned_iterable, collect_python_iterator},
};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, ContainsVM, RecursionToken, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{
        ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead,
        HeapReadMut, HeapReadOutput, HeapReader,
    },
    intern::StaticStrings,
    modules::copy::{Memo, PyDeepCopy, deep_copy},
    resource_checks::{check_repeat_size, repeated_len, try_vec_with_capacity},
    sorting::parse_and_sort,
    types::{
        LazyHeapSet, Type, TypeRef,
        sequence::SequenceItems,
        slice::{normalize_sequence_index, value_to_i64_bound},
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

const INITIAL_INT_CAPACITY: usize = 4;

/// A Python list keeps its heap identity when its storage changes.
#[derive(Debug, Default)]
pub(crate) struct List {
    storage: ListStorage,
    // Generic storage may contain heap references when this flag is set.
    contains_refs: bool,
}

impl List {
    /// Takes ownership of an existing buffer and its references.
    #[must_use]
    pub fn new(items: Vec<Value>) -> Self {
        if items.is_empty() {
            return Self::default();
        }
        let contains_refs = items.iter().any(|item| matches!(item, Value::Ref(_)));
        Self {
            storage: ListStorage::Generic(items),
            contains_refs,
        }
    }

    /// Packs compatible values when both buffers fit during conversion.
    pub(crate) fn from_values(items: Vec<Value>, heap: &Heap) -> Self {
        let tracker = &heap.tracker;
        if let Ok(packing) = Packing::for_values(&items, tracker)
            && packing != Packing::Generic
            && let Ok(mut packed) = PackedList::new(items.len(), packing, tracker)
        {
            for (index, item) in items.iter().enumerate() {
                if tracker.check_time_every(index).is_err() {
                    return Self::new(items);
                }
                packed.push(item);
            }
            return Self::from_packed(packed);
        }
        Self::new(items)
    }

    fn from_packed(items: PackedList) -> Self {
        if items.len() == 0 {
            return Self::default();
        }
        Self {
            storage: ListStorage::Packed(items),
            contains_refs: false,
        }
    }

    #[must_use]
    pub(crate) fn items(&self) -> SequenceItems<'_> {
        self.storage.view()
    }

    pub(crate) fn swap(&mut self, i: usize, j: usize) {
        match &mut self.storage {
            ListStorage::Empty => unreachable!("cannot swap items in an empty list"),
            ListStorage::Packed(items) => items.swap(i, j),
            ListStorage::Generic(items) => items.swap(i, j),
        }
    }

    /// Moves owned items into a generic list without a resource check, for heap fixtures.
    #[cfg(test)]
    pub(crate) fn extend_owned(&mut self, items: impl IntoIterator<Item = Value>) {
        let values = self.generic_mut();
        let start = values.len();
        values.extend(items);
        self.contains_refs |= values[start..].iter().any(|item| matches!(item, Value::Ref(_)));
    }

    fn generic_mut(&mut self) -> &mut Vec<Value> {
        if matches!(self.storage, ListStorage::Empty) {
            self.storage = ListStorage::Generic(Vec::new());
        }
        let ListStorage::Generic(items) = &mut self.storage else {
            unreachable!("list storage must be generic")
        };
        items
    }

    /// Reads the live index so callers can tolerate reentrant mutation.
    #[expect(
        clippy::inline_always,
        reason = "keeps storage dispatch inside the integer subscript read"
    )]
    #[inline(always)]
    fn get_cloned(&self, index: usize, heap: &Heap) -> Option<Value> {
        match &self.storage {
            ListStorage::Empty => None,
            ListStorage::Packed(items) => items.get(index),
            ListStorage::Generic(items) => items.get(index).map(|item| item.clone_with_heap(heap)),
        }
    }

    #[inline]
    pub(crate) fn getitem_int(&self, index: i64, heap: &Heap) -> RunResult<Value> {
        let index = if index < 0 {
            index + i64::try_from(self.len()).expect("list length exceeds i64::MAX")
        } else {
            index
        };
        usize::try_from(index)
            .ok()
            .and_then(|index| self.get_cloned(index, heap))
            .ok_or_else(ExcType::list_index_error)
    }

    fn try_set(&mut self, index: usize, value: &mut Value) -> bool {
        match &mut self.storage {
            ListStorage::Packed(items) => items.try_set(index, value),
            ListStorage::Generic(items) => {
                self.contains_refs |= matches!(value, Value::Ref(_));
                // Return the old item to the caller for refcount cleanup.
                mem::swap(&mut items[index], value);
                true
            }
            ListStorage::Empty => unreachable!("assignment index is in bounds"),
        }
    }

    fn remove_at(&mut self, index: usize) -> Value {
        match &mut self.storage {
            ListStorage::Empty => unreachable!("cannot remove an item from an empty list"),
            ListStorage::Packed(items) => items.remove(index),
            ListStorage::Generic(items) => items.remove(index),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match &self.storage {
            ListStorage::Empty => 0,
            ListStorage::Packed(items) => items.len(),
            ListStorage::Generic(items) => items.len(),
        }
    }

    /// Visits each owned heap reference once per occurrence, as in `py_dec_ref_ids`.
    pub(crate) fn for_each_child_id(&self, on_child: impl FnMut(HeapId)) {
        if self.contains_refs
            && let ListStorage::Generic(items) = &self.storage
        {
            items.iter().filter_map(Value::ref_id).for_each(on_child);
        }
    }
}

impl<'h> HeapReadMut<'h, List> {
    fn promote(&self, vm: &mut VM<'h>, additional: usize) -> RunResult<()> {
        if let ListStorage::Packed(items) = &self.get(vm.heap).storage {
            let values = ListStorage::expand_packed(items, additional, vm.heap)?;
            self.get_mut(vm.heap).storage = ListStorage::Generic(values);
        }
        Ok(())
    }

    fn reserve_bulk(&self, vm: &mut VM<'h>, additional: usize, incoming: Packing) -> RunResult<()> {
        let mut list = self.get(vm.heap);
        let len = list.len();
        let mut previous = list.storage.packing();
        if len == 0
            && incoming != Packing::Empty
            && previous != Packing::Empty
            && previous != incoming
            && (previous == Packing::Generic || previous.merge(incoming) != previous)
        {
            *self.get_mut(vm.heap) = List::default();
            list = self.get(vm.heap);
            previous = Packing::Empty;
        }
        let packing = previous.merge(incoming);
        if matches!(list.storage, ListStorage::Packed(_)) && packing == Packing::Generic {
            return self.promote(vm, additional);
        }

        let required = checked_list_len(len, additional)?;
        let transition = previous != packing;
        let needs_wider = matches!(&list.storage, ListStorage::Packed(items) if items.needs_wider_ints(packing));
        let current = if needs_wider || (previous == Packing::Nones && transition) {
            0
        } else {
            list.storage.capacity()
        };
        if required == 0 || (required <= current && !transition) {
            return Ok(());
        }

        let preferred = if transition && (previous != Packing::Empty || required > 1) {
            required.max(current)
        } else {
            required.max(current.saturating_mul(2)).max(INITIAL_INT_CAPACITY)
        };
        let growth_bytes = |capacity: usize| match &list.storage {
            ListStorage::Packed(items) => items.growth_bytes(capacity, packing),
            ListStorage::Empty if packing != Packing::Generic => PackedList::Empty.growth_bytes(capacity, packing),
            _ => (capacity - current).saturating_mul(VALUE_SIZE),
        };
        let tracker = &vm.heap.tracker;
        let capacity = if tracker.check_pending_allocation(growth_bytes(preferred)).is_ok() {
            preferred
        } else {
            let required = required.max(current);
            tracker.check_pending_allocation(growth_bytes(required))?;
            required
        };
        if needs_wider && let ListStorage::Packed(items) = &list.storage {
            let replacement = items.widen(capacity, packing, tracker)?;
            self.get_mut(vm.heap).storage = ListStorage::Packed(replacement);
            return Ok(());
        }

        let storage = &mut self.get_mut(vm.heap).storage;
        match storage {
            ListStorage::Empty if packing != Packing::Generic => {
                let mut items = PackedList::Empty;
                items.reserve(capacity, packing)?;
                *storage = ListStorage::Packed(items);
            }
            ListStorage::Empty => *storage = ListStorage::Generic(try_vec_with_capacity(capacity)?),
            ListStorage::Packed(items) => items.reserve(capacity, packing)?,
            ListStorage::Generic(items) => items
                .try_reserve_exact(capacity - len)
                .map_err(|_| ExcType::allocation_too_large())?,
        }
        Ok(())
    }

    fn try_extend_packed(&self, vm: &mut VM<'h>, source: &HeapRead<'h, List>) -> RunResult<bool> {
        let target = self.get(vm.heap);
        let source = source.get(vm.heap);
        if source.len() == 0 {
            return Ok(true);
        }
        let ListStorage::Packed(items) = &source.storage else {
            return Ok(false);
        };
        let packing = items.packing();
        if target.len() != 0 && target.storage.packing().merge(packing) == Packing::Generic {
            return Ok(false);
        }

        let additional = items.len();
        if ptr::eq(target, source) {
            self.reserve_bulk(vm, additional, packing)?;
            let ListStorage::Packed(target) = &mut self.get_mut(vm.heap).storage else {
                unreachable!("reserved packed storage");
            };
            target.extend_from_within(0..additional);
        } else {
            let tracker = &vm.heap.tracker;
            let mut snapshot = PackedList::new(additional, packing, tracker)?;
            snapshot.copy_range(items, 0..additional, tracker)?;
            // Finish fallible work before writing to the target.
            self.reserve_bulk(vm, additional, packing)?;
            let ListStorage::Packed(target) = &mut self.get_mut(vm.heap).storage else {
                unreachable!("reserved packed storage");
            };
            target.append_range(&snapshot, 0..additional);
        }
        Ok(true)
    }

    fn packing_for(&self, vm: &VM<'h>, items: &[Value]) -> RunResult<Packing> {
        if matches!(&self.get(vm.heap).storage, ListStorage::Generic(values) if !values.is_empty()) {
            Ok(Packing::Generic)
        } else {
            Packing::for_values(items, &vm.heap.tracker)
        }
    }

    pub(crate) fn extend_values<T>(&self, vm: &mut VM<'h>, items: T) -> RunResult<()>
    where
        T: AsRef<[Value]> + IntoIterator<Item = Value> + DropWithContext<VM<'h>>,
    {
        let mut items = DropGuard::new(items, vm);
        let (pending, vm) = items.as_parts();
        let values = pending.as_ref();
        if values.is_empty() {
            return Ok(());
        }
        let packing = self.packing_for(vm, values)?;
        self.reserve_bulk(vm, values.len(), packing)?;
        let (items, vm) = items.into_parts();
        let list = self.get_mut(vm.heap);
        match &mut list.storage {
            ListStorage::Packed(values) => items.into_iter().for_each(|item| values.push(&item)),
            ListStorage::Generic(values) => {
                let start = values.len();
                values.extend(items);
                list.contains_refs |= values[start..].iter().any(|item| matches!(item, Value::Ref(_)));
            }
            ListStorage::Empty => unreachable!("reserved space for a nonempty batch"),
        }
        Ok(())
    }

    fn try_set_numeric_slice(
        &self,
        vm: &mut VM<'h>,
        source: &HeapRead<'h, List>,
        (start, stop, step): (i64, i64, i64),
    ) -> RunResult<bool> {
        let ListStorage::Packed(source) = &source.get(vm.heap).storage else {
            return Ok(false);
        };
        let target = &self.get(vm.heap).storage;
        let compatible = match source {
            PackedList::ByteInts(_) => matches!(
                target,
                ListStorage::Empty | ListStorage::Packed(PackedList::ByteInts(_) | PackedList::Ints(_))
            ),
            PackedList::Ints(_) => matches!(target, ListStorage::Empty | ListStorage::Packed(PackedList::Ints(_))),
            PackedList::Floats(_) => matches!(target, ListStorage::Empty | ListStorage::Packed(PackedList::Floats(_))),
            _ => false,
        };
        if !compatible {
            return Ok(false);
        }

        let count = slice_item_count(start, stop, step);
        check_slice_replacement(step, count, source.len())?;
        if count == 0 && source.len() == 0 {
            return Ok(true);
        }
        let growth = source.len().saturating_sub(count);
        let packing = source.packing();
        // Snapshot before mutation so `a[...] = a` reads the original list.
        let snapshot = NumericSliceSnapshot::take(source, &vm.heap.tracker)?;
        if growth != 0 {
            self.reserve_bulk(vm, growth, packing)?;
        }
        let target = self.get_mut(vm.heap);
        let ListStorage::Packed(items) = &mut target.storage else {
            unreachable!("reserved numeric storage");
        };
        snapshot.write(items, start, step, count);
        Ok(true)
    }

    /// Collects replacements before assignment to support self-assignment and avoid
    /// partial writes on iterator failure. Bounds are normalized after collection
    /// because the iterator may have resized the target.
    // Keep slice collection and replacement out of the scalar assignment handler.
    #[inline(never)]
    fn setitem_slice(&self, slice: &super::Slice, value: &Value, vm: &mut VM<'h>) -> RunResult<()> {
        // Reject a zero step before executing the replacement iterator.
        let indices = slice.indices(self.get(vm.heap).len())?;

        let items = match value.read_heap(vm) {
            Some(HeapReadOutput::List(list)) => {
                if self.try_set_numeric_slice(vm, &list, indices)? {
                    return Ok(());
                }
                list.clone_all_items(vm)?
            }
            Some(HeapReadOutput::Tuple(tuple)) => tuple.cloned_items(vm)?,
            _ => {
                let iterator = value
                    .py_iter(vm)
                    .and_then(|call| call.require_synchronous(vm))
                    .map_err(|error| {
                        if matches!(&error, RunError::Exc(raise) if raise.exc.exc_type() == ExcType::TypeError) {
                            ExcType::type_error("must assign iterable to extended slice")
                        } else {
                            error
                        }
                    })?;
                collect_python_iterator(iterator, vm)?
            }
        };
        defer_drop_mut!(items, vm);

        // Iteration may have changed the target list's length.
        let (start, stop, step) = slice.indices(self.get(vm.heap).len())?;
        let count = slice_item_count(start, stop, step);

        check_slice_replacement(step, count, items.len())?;
        if count == 0 && items.is_empty() {
            return Ok(());
        }

        let packing = self.packing_for(vm, items)?;
        let growth = items.len().saturating_sub(count);
        self.reserve_bulk(vm, growth, packing)?;
        if let ListStorage::Packed(target) = &mut self.get_mut(vm.heap).storage {
            if step == 1 {
                let start = usize::try_from(start).expect("positive slice start is non-negative");
                target.splice(start..start + count, items);
            } else {
                let mut index = i128::from(start);
                for item in items.iter() {
                    target.set(usize::try_from(index).expect("slice index is within the list"), item);
                    index += i128::from(step);
                }
            }
        } else if items.len() == count {
            let list = self.get_mut(vm.heap);
            list.contains_refs |= items.iter().any(|item| matches!(item, Value::Ref(_)));
            let target = list.generic_mut();
            if step == 1 {
                let start = usize::try_from(start).expect("positive slice start is non-negative");
                target[start..start + count].swap_with_slice(items);
            } else {
                let mut index = i128::from(start);
                for item in items.iter_mut() {
                    mem::swap(
                        &mut target[usize::try_from(index).expect("slice index is within the list")],
                        item,
                    );
                    index += i128::from(step);
                }
            }
        } else {
            vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE))?;
            let list = self.get_mut(vm.heap);
            list.contains_refs |= items.iter().any(|item| matches!(item, Value::Ref(_)));
            let start = usize::try_from(start).expect("positive slice start is non-negative");
            let removed: Vec<Value> = list
                .generic_mut()
                .splice(start..start + count, items.drain(..))
                .collect();
            removed.drop_with(vm);
        }
        Ok(())
    }

    /// Takes ownership of the item, including on allocation failure.
    pub fn append(&self, vm: &mut VM<'h>, item: Value) -> RunResult<()> {
        self.insert(vm, usize::MAX, item)
    }

    /// Clamps the index to the list length and consumes the item on every path.
    #[inline]
    pub fn insert(&self, vm: &mut VM<'h>, index: usize, item: Value) -> RunResult<()> {
        let list = self.get_mut(vm.heap);
        match (&item, &mut list.storage) {
            (_, ListStorage::Generic(items))
                if items.len() < items.capacity() && (!items.is_empty() || Packing::of(&item) == Packing::Generic) =>
            {
                list.contains_refs |= matches!(item, Value::Ref(_));
                insert_slot(items, index, item);
                return Ok(());
            }
            (Value::Int(integer), ListStorage::Packed(PackedList::ByteInts(items)))
                if items.len() < items.capacity()
                    && let Ok(byte) = u8::try_from(*integer) =>
            {
                insert_slot(items, index, byte);
                return Ok(());
            }
            (Value::Int(integer), ListStorage::Packed(PackedList::Ints(items))) if items.len() < items.capacity() => {
                insert_slot(items, index, *integer);
                return Ok(());
            }
            (Value::Float(value), ListStorage::Packed(PackedList::Floats(items))) if items.len() < items.capacity() => {
                insert_slot(items, index, *value);
                return Ok(());
            }
            _ => {}
        }
        self.insert_with_reserve(vm, index, item)
    }

    fn insert_with_reserve(&self, vm: &mut VM<'h>, index: usize, item: Value) -> RunResult<()> {
        let mut item = DropGuard::new(item, vm);
        let (pending, vm) = item.as_parts();
        let packing = Packing::of(pending);
        self.reserve_bulk(vm, 1, packing)?;

        let (item, vm) = item.into_parts();
        let list = self.get_mut(vm.heap);
        let index = index.min(list.len());
        if let ListStorage::Packed(items) = &mut list.storage {
            items.insert(index, &item);
        } else {
            list.contains_refs |= matches!(item, Value::Ref(_));
            list.generic_mut().insert(index, item);
        }
        Ok(())
    }
}

impl List {
    /// Creates a list from the `list()` constructor call.
    ///
    /// - `list()` with no args returns an empty list
    /// - `list(iterable)` creates a list from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("list", vm.heap)?;
        match value {
            None => {
                let heap_id = vm.heap.allocate(HeapData::List(Self::new(Vec::new())));
                Ok(Value::Ref(heap_id))
            }
            Some(value) => {
                let mut value = DropGuard::new(value, vm);
                let (pending, vm) = value.as_parts();
                let packed = match pending.read_heap(vm) {
                    Some(HeapReadOutput::Bytes(bytes)) => {
                        Some(PackedList::from_bytes(bytes.get(vm.heap).as_slice(), &vm.heap.tracker)?)
                    }
                    Some(HeapReadOutput::Range(range)) => {
                        Some(PackedList::from_range(range.get(vm.heap), &vm.heap.tracker)?)
                    }
                    Some(HeapReadOutput::List(list)) => return list.copy_for_constructor(vm),
                    _ => None,
                };
                if let Some(items) = packed {
                    return Ok(Value::Ref(vm.heap.allocate(HeapData::List(Self::from_packed(items)))));
                }
                let (value, vm) = value.into_parts();
                let items = collect_owned_iterable(value, vm)?;
                let heap_id = vm.heap.allocate(HeapData::List(Self::from_values(items, vm.heap)));
                Ok(Value::Ref(heap_id))
            }
        }
    }
}

impl<'h> HeapRead<'h, List> {
    fn copy_for_constructor(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let list = self.get(vm.heap);
        let ListStorage::Generic(items) = &list.storage else {
            return list_copy(list, vm.heap);
        };
        let output = self.clone_generic_slice(0, 1, items.len(), vm)?;
        let copy = List::from_values(output, vm.heap);
        Ok(Value::Ref(vm.heap.allocate(HeapData::List(copy))))
    }

    fn clone_generic_slice(&self, start: i64, step: i64, count: usize, vm: &mut VM<'h>) -> RunResult<Vec<Value>> {
        vm.heap
            .tracker
            .check_pending_allocation(count.saturating_mul(VALUE_SIZE))?;
        let mut copy = DropGuard::new(try_vec_with_capacity(count)?, vm);
        let (output, vm) = copy.as_parts_mut();
        let ListStorage::Generic(items) = &self.get(vm.heap).storage else {
            unreachable!("caller matched generic storage")
        };
        if step == 1 {
            let start = usize::try_from(start).expect("forward slice starts within the list");
            SequenceItems::from_slice(&items[start..start + count]).extend_cloned(output, vm.heap)?;
        } else {
            for (offset, index) in packed::slice_indices(start, step, count).enumerate() {
                vm.heap.tracker.check_time_every(offset)?;
                output.push(items[index].clone_with_heap(vm.heap));
            }
        }
        Ok(copy.into_inner())
    }

    fn getitem_slice(&self, start: i64, step: i64, count: usize, vm: &mut VM<'h>) -> RunResult<Value> {
        let list = match &self.get(vm.heap).storage {
            ListStorage::Empty => List::default(),
            ListStorage::Packed(items) => List::from_packed(items.slice(start, step, count, &vm.heap.tracker)?),
            ListStorage::Generic(_) => List::new(self.clone_generic_slice(start, step, count, vm)?),
        };
        Ok(Value::Ref(vm.heap.allocate(HeapData::List(list))))
    }

    /// Clones the item at the given index with proper refcount management.
    ///
    /// Panics if `index` is out of bounds, so only use it where the length was
    /// read without a user callback running since — the comparison walks below
    /// use [`Self::try_clone_item`] instead.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h>) -> Value {
        self.get(vm.heap).items().clone_item(index, vm.heap)
    }

    /// Clones the item at `index`, or `None` once the list has shrunk past it.
    ///
    /// For walks that run Python between steps and so cannot trust a length
    /// read earlier: the comparison walks index the *other* list directly
    /// while a user `__eq__` may be shrinking it, and `copy.deepcopy` runs a
    /// `__deepcopy__` hook between items.
    pub(crate) fn try_clone_item(&self, index: usize, vm: &mut VM<'h>) -> Option<Value> {
        self.get(vm.heap).get_cloned(index, vm.heap)
    }

    /// Lexicographic comparison for lists — the ordering behind `<`/`<=`/`>`/`>=`.
    ///
    /// Element-by-element left-to-right; the first non-equal pair decides. If all
    /// compared elements are equal, the shorter list is less (`[1] < [1, 2]`).
    /// The first differing pair's [`CmpOrder`] propagates: a `NaN` element makes
    /// the list [`CmpOrder::Unordered`] (`[nan] < [1]` is `False`), a
    /// type-mismatched element makes it [`CmpOrder::Incomparable`] (`[1] < ['a']`
    /// raises `TypeError`). Mirrors [`Tuple::py_cmp`](super::Tuple) — the
    /// `ListIter` holds the recursion token that bounds nested-container depth.
    pub(crate) fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        let a_len = self.get(vm.heap).len();
        let b_len = other.get(vm.heap).len();
        let min_len = a_len.min(b_len);
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, av)) = iter.next_with_index(vm)? {
            if i >= min_len {
                break;
            }
            // `other` may have been shrunk past `i` by a user comparison on a
            // previous iteration; CPython stops at whichever list ran out.
            let Some(bv) = other.try_clone_item(i, vm) else {
                break;
            };
            defer_drop!(bv, vm);
            match av.py_cmp(bv, vm)? {
                CmpOrder::Ordered(Ordering::Equal) => {}
                CmpOrder::Ordered(ord) => return Ok(CmpOrder::Ordered(ord)),
                // A `NaN` element is never `==`-equal, so it is the first
                // differing pair and the list is unordered (yields `False`).
                CmpOrder::Unordered => return Ok(CmpOrder::Unordered),
                // CPython checks `__eq__` first and only orders non-equal pairs, so
                // equal-but-unorderable elements (e.g. `None == None`) don't block.
                CmpOrder::Incomparable => {
                    if !av.py_eq(bv, vm)? {
                        return Ok(CmpOrder::Incomparable);
                    }
                }
            }
        }
        // Re-read: a user comparison may have resized either list, and CPython
        // settles an all-equal prefix on the lengths as they are now.
        let a_len = self.get(vm.heap).len();
        let b_len = other.get(vm.heap).len();
        Ok(CmpOrder::Ordered(a_len.cmp(&b_len)))
    }

    /// Clones all items from this list with proper refcount management.
    ///
    /// Preflights the slot bytes so an over-budget clone raises a graceful
    /// `MemoryError` instead of bursting past the allocator's hard limit.
    fn clone_all_items(&self, vm: &mut VM<'h>) -> RunResult<Vec<Value>> {
        self.get(vm.heap).items().to_vec(vm.heap)
    }

    /// Returns a stack-borrowed lending iterator over the list's items,
    /// holding a recursion-depth token for its entire lifetime.
    ///
    /// Named `iter` despite returning a non-stdlib lending iterator (see
    /// [`ListIter`]) because that's the obvious entry point for "iterate
    /// this container" — the lending shape is documented on the returned
    /// type, and exposing `next(vm)` makes it self-evident the result is
    /// not a [`Iterator`](core::iter::Iterator).
    #[expect(clippy::iter_not_returning_iterator)]
    pub(crate) fn iter(&self, vm: &mut VM<'h>) -> RunResult<ListIter<'_, 'h>> {
        ListIter::new(self, vm)
    }
}

/// Stack-borrowed lending iterator over a [`List`]'s items.
///
/// Borrows a [`HeapRead`] for its lifetime, retaining its object guard or
/// protected source — no extra refcount on the container is needed.
///
/// **Lending shape.** [`next`](Self::next) returns `Option<&Value>` rather
/// than `Option<Value>`. The iterator itself owns the most-recently-yielded
/// item in its `current` slot (using [`Value::Undefined`] as the empty
/// sentinel) and drops the previous item at the start of each `next` call.
/// The held item is also dropped when the iterator itself is released via
/// [`DropWithContext`]. This means call sites do **not** need a per-item
/// `defer_drop!`; the iter manages every item it hands out.
///
/// **Recursion guard.** Acquires a [`RecursionToken`] at construction and
/// releases it via [`DropWithContext`]. The iterator MUST be wrapped in
/// [`defer_drop_mut!`] so the token (and any in-flight item) is released on
/// every exit path (success, early `return`, error via `?`). The token is
/// intentionally non-optional — every iteration of a Python container can
/// transitively trigger `py_eq` / `py_hash` / `py_repr` / `py_cmp` /
/// dict-or-set membership, all of which recurse on cyclic structures.
///
/// **Mutation safety.** The list length is re-read on every call to `next`,
/// so items appended during iteration may be visited and shrinking past the
/// current index halts iteration cleanly rather than panicking on an
/// out-of-bounds index.
pub(crate) struct ListIter<'a, 'h> {
    list: &'a HeapRead<'h, List>,
    index: usize,
    token: RecursionToken,
    /// Most-recently-yielded item. `Value::Undefined` when nothing is held —
    /// drops on that variant are no-ops, so `next` can unconditionally drop
    /// the previous slot before fetching the new one.
    current: Value,
}

impl<'a, 'h> ListIter<'a, 'h> {
    fn new(list: &'a HeapRead<'h, List>, vm: &mut VM<'h>) -> RunResult<Self> {
        let token = vm.recursion_token()?;
        Ok(Self {
            list,
            index: 0,
            token,
            current: Value::Undefined,
        })
    }

    /// Advances the iterator and returns a borrow of the next item, or
    /// `Ok(None)` when the list is exhausted (or has shrunk below the
    /// current index).
    ///
    /// The returned reference is valid until the next call to `next` (or
    /// until the iterator itself is dropped), at which point the held item
    /// is released.
    ///
    /// Performs an amortized time-limit check (a clock read every 64th
    /// call) so long Rust-side loops cannot bypass the configured timeout.
    pub(crate) fn next<'i>(&'i mut self, vm: &mut VM<'h>) -> RunResult<Option<&'i Value>> {
        // Drop the previously-yielded item (no-op when `current` is `Undefined`).
        mem::replace(&mut self.current, Value::Undefined).drop_with(vm.heap);
        vm.heap.tracker.check_time_every(self.index)?;
        let Some(item) = self.list.get(vm.heap).get_cloned(self.index, vm.heap) else {
            return Ok(None);
        };
        self.current = item;
        self.index += 1;
        Ok(Some(&self.current))
    }

    /// Like [`next`](Self::next), but also returns the 0-based position of
    /// the yielded item — useful for `zip`-style sibling-container access,
    /// returning the matching position from search methods, or applying
    /// per-position range checks.
    ///
    /// The position is the index of the item being returned (not the next
    /// one to read), so the first yielded item has position 0.
    pub(crate) fn next_with_index<'i>(&'i mut self, vm: &mut VM<'h>) -> RunResult<Option<(usize, &'i Value)>> {
        // Capture before `next` increments `self.index`.
        let position = self.index;
        Ok(self.next(vm)?.map(|item| (position, item)))
    }
}

impl<'h, C: ContainsVM<'h>> DropWithContext<C> for ListIter<'_, 'h> {
    fn drop_with(self, container: &mut C) {
        self.current.drop_with(container);
        self.token.drop_with(container);
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, List> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// Linear search by equality, stored element on the left of `==` as CPython's
    /// `list_contains` does. `ListIter` re-reads the length on every step, so a
    /// user `__eq__` re-entering the VM and shrinking the list halts the walk
    /// cleanly instead of indexing out of bounds.
    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(el) = iter.next(vm)? {
            if el.py_eq(item, vm)? {
                return Ok(Some(true));
            }
        }
        Ok(Some(false))
    }

    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        Type::List.into()
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        HeapRead::py_cmp(self, other, vm)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            let (start, stop, step) = slice.indices(self.get(vm.heap).len())?;
            return self.getitem_slice(start, step, slice_item_count(start, stop, step), vm);
        }

        let index = key.as_index(vm, Type::List)?;
        self.get(vm.heap).getitem_int(index, vm.heap)
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop_mut!(value, vm);

        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            let slice = slice.clone();
            return self.setitem_slice(&slice, value, vm);
        }

        let index = match *key {
            Value::Int(index) => index,
            Value::Bool(value) => i64::from(value),
            _ if let Some(index) = key.as_long_int(vm) => {
                index.to_i64().ok_or_else(ExcType::index_error_int_too_large)?
            }
            _ => {
                let key_type = key.py_type_name(vm);
                return Err(ExcType::type_error_indices(Type::List, &key_type));
            }
        };

        // Normalize negative indices (Python-style: -1 = last element)
        let list = self.get_mut(vm.heap);
        let len = i64::try_from(list.len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_assignment_index_error());
        }

        let idx = usize::try_from(normalized_index).expect("index validated non-negative");

        if !list.try_set(idx, value) {
            self.reserve_bulk(vm, 0, Packing::of(value))?;
            assert!(self.get_mut(vm.heap).try_set(idx, value), "reserved list storage");
        }

        Ok(())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::List(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        if self.get(vm.heap).len() != other.get(vm.heap).len() {
            return Ok(Some(false));
        }
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, a)) = iter.next_with_index(vm)? {
            // `ListIter` re-reads `self`'s length each step, but `other` is
            // indexed directly and a user `__eq__` may have shrunk it.
            let Some(b) = other.try_clone_item(i, vm) else {
                break;
            };
            defer_drop!(b, vm);
            if !a.py_eq(b, vm)? {
                return Ok(Some(false));
            }
        }
        // Equal element-wise as far as both lists still reach; a mutating
        // `__eq__` can leave them different lengths, which CPython reports as
        // not equal.
        Ok(Some(self.get(vm.heap).len() == other.get(vm.heap).len()))
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(self.get(vm.heap).len() != 0)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        repr_sequence_fmt('[', ']', |heap, i| self.get(heap).get_cloned(i, heap), f, vm, heap_ids)
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(HeapReadOutput::List(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let left = self.get(vm.heap);
        let right = other.get(vm.heap);
        if left.len() == 0 || right.len() == 0 {
            return list_copy(if left.len() == 0 { right } else { left }, vm.heap).map(Some);
        }
        if let (ListStorage::Packed(a), ListStorage::Packed(b)) = (&left.storage, &right.storage) {
            let total = checked_list_len(a.len(), b.len())?;
            let tracker = &vm.heap.tracker;
            let packing = a.packing().merge(b.packing());
            let list = if packing == Packing::Generic {
                tracker.check_allocation(total.saturating_mul(VALUE_SIZE))?;
                let mut values = try_vec_with_capacity(total)?;
                for (index, value) in a.view().iter().chain(b.view().iter()).enumerate() {
                    tracker.check_time_every(index)?;
                    values.push(value);
                }
                List::new(values)
            } else {
                let mut items = PackedList::new(total, packing, tracker)?;
                items.copy_range(a, 0..a.len(), tracker)?;
                items.copy_range(b, 0..b.len(), tracker)?;
                List::from_packed(items)
            };
            return Ok(Some(Value::Ref(vm.heap.allocate(HeapData::List(list)))));
        }
        // The guard releases our clones if cloning `other` hits the memory limit.
        let mut items = DropGuard::new(self.clone_all_items(vm)?, vm);
        let (left, vm) = items.as_parts_mut();
        left.extend(other.clone_all_items(vm)?);
        let (items, vm) = items.into_parts();
        let id = vm.heap.allocate(HeapData::List(List::new(items)));
        Ok(Some(Value::Ref(id)))
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(count) = other.py_value(vm).repeat_count(vm)? else {
            return Ok(None);
        };
        let len = self.get(vm.heap).len();
        // An empty operand yields `[]` however large `count` is; without this the
        // loop below would spin `count` times over nothing.
        if count == 0 || len == 0 {
            return Ok(Some(Value::Ref(
                vm.heap.allocate(HeapData::List(List::new(Vec::new()))),
            )));
        }
        if let ListStorage::Packed(items) = &self.get(vm.heap).storage {
            let packing = items.packing();
            let total = repeated_len(len, count, list_element_size(packing))?;
            let tracker = &vm.heap.tracker;
            let mut result = PackedList::new(total, packing, tracker)?;
            result.copy_range(items, 0..len, tracker)?;
            result.repeat_prefix(total, tracker)?;
            return Ok(Some(Value::Ref(
                vm.heap.allocate(HeapData::List(List::from_packed(result))),
            )));
        }
        let total = repeated_len(len, count, VALUE_SIZE)?;
        check_repeat_size(len.saturating_mul(VALUE_SIZE), count, &vm.heap.tracker)?;
        // The guard releases the clones built so far if the time poll trips.
        let mut result = DropGuard::new(try_vec_with_capacity(total)?, vm);
        let (items, vm) = result.as_parts_mut();
        let ListStorage::Generic(source) = &self.get(vm.heap).storage else {
            unreachable!("only nonempty generic lists reach this branch");
        };
        for rep in 0..count {
            items.extend(source.iter().map(|item| item.clone_with_heap(vm.heap)));
            vm.heap.tracker.check_time_every(rep)?;
        }
        let (items, vm) = result.into_parts();
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))))))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    /// Extends in place from a list or synchronous iterable, preserving aliases.
    fn py_iadd_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        // Lists use bulk copying; other operands follow the iterator protocol, including its errors.
        let other_id = other
            .ref_id()
            .filter(|id| matches!(vm.heap.get(*id), HeapData::List(_)));
        let Some(other_id) = other_id else {
            // Append as the source yields so an iterator error keeps earlier additions.
            let iterator = other.py_iter(vm)?.require_synchronous(vm)?;
            defer_drop!(iterator, vm);
            let mut iterator = iterator.read(vm);

            // Preflight the size hint; append still checks growth if the iterator yields more.
            let hint = iterator.iter_size_hint(vm);
            vm.heap.tracker.check_allocation(hint.saturating_mul(VALUE_SIZE))?;

            while let Some(item) = iterator.py_next(vm)? {
                self.append(vm, item)?;
            }
            return Ok(true);
        };

        let HeapReadOutput::List(source) = vm.heap.read(other_id) else {
            unreachable!("checked the source list above");
        };
        if !self.try_extend_packed(vm, &source)? {
            vm.heap
                .tracker
                .check_allocation(source.get(vm.heap).len().saturating_mul(2 * VALUE_SIZE))?;
            let items = source.clone_all_items(vm)?;
            self.extend_values(vm, items)?;
        }

        Ok(true)
    }

    /// Repeats for integer or `__index__` counts, retaining identity and shared elements.
    fn py_imul_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let Some(count) = other.py_value(vm).repeat_count(vm)? else {
            return Ok(false);
        };

        // `__index__` can resize this list; read its length only after conversion.
        let len = self.get(vm.heap).len();
        // These no-op repetitions need neither allocation nor reference-count changes.
        if count == 1 || len == 0 {
            return Ok(true);
        }

        // Clear the existing list rather than replacing it, so aliases also see an empty list.
        // Detach the items before releasing their references, which may include the list itself.
        if count == 0 {
            list_clear(self, vm);
            return Ok(true);
        }

        let packing = self.get(vm.heap).storage.packing();
        let item_size = list_element_size(packing);
        let total = repeated_len(len, count, item_size)?;
        if packing == Packing::Generic {
            check_repeat_size(len.saturating_mul(item_size), count, &vm.heap.tracker)?;
        }
        self.reserve_bulk(vm, total - len, packing)?;

        if packing == Packing::Generic {
            for index in len..total {
                vm.heap.tracker.check_time_every(index)?;
                let item = self.get(vm.heap).items().clone_item(index % len, vm.heap);
                self.get_mut(vm.heap).generic_mut().push(item);
            }
        } else {
            let mut filled = len;
            while filled < total {
                vm.heap.tracker.check_time()?;
                let ListStorage::Packed(items) = &mut self.get_mut(vm.heap).storage else {
                    unreachable!("reserved packed storage");
                };
                filled = items.repeat_chunk(len, total);
            }
        }
        Ok(true)
    }

    /// Delegates methods to `call_list_method`.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        if attr.static_string(vm.interns) == Some(StaticStrings::Sort) {
            do_list_sort(self, args, vm)?;
            return Ok(CallResult::Value(Value::None));
        }

        let Some(method) = attr.static_string(vm.interns) else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::List, attr.as_str(vm.interns)));
        };

        call_list_method(self, method, args, vm).map(CallResult::Value)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let list_id = self.id();
        let iterator = vm.heap.allocate(HeapData::ListIterator(ListIterator::new(list_id)));
        vm.heap.inc_ref(list_id);
        Ok(CallResult::Value(Value::Ref(iterator)))
    }
}

impl<C: ContainsHeap> DropWithContext<C> for List {
    fn drop_with(self, ctx: &mut C) {
        if let ListStorage::Generic(items) = self.storage {
            items.drop_with(ctx);
        }
    }
}

impl HeapItem for List {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if !self.contains_refs {
            return;
        }
        let ListStorage::Generic(items) = &mut self.storage else {
            return;
        };
        for obj in items {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                obj.dec_ref_forget();
            }
        }
    }
}

/// Dispatches a method call on a list value.
///
/// This is the unified entry point for list method calls.
///
/// # Arguments
/// * `list` - The list to call the method on
/// * `method` - The method to call (e.g., `StaticStrings::Append`)
/// * `args` - The method arguments
/// * `heap` - The heap for allocation and reference counting
fn call_list_method<'h>(
    list: &HeapReadMut<'h, List>,
    method: StaticStrings,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let heap = &mut *vm.heap;
    match method {
        StaticStrings::Append => {
            let item = args.get_one_arg("list.append", heap)?;
            list.append(vm, item)?;
            Ok(Value::None)
        }
        StaticStrings::Insert => list_insert(list, args, vm),
        StaticStrings::Pop => list_pop(list, args, vm),
        StaticStrings::Remove => list_remove(list, args, vm),
        StaticStrings::Clear => {
            args.check_zero_args("list.clear", heap)?;
            list_clear(list, vm);
            Ok(Value::None)
        }
        StaticStrings::Copy => {
            args.check_zero_args("list.copy", heap)?;
            list_copy(list.get(heap), heap)
        }
        StaticStrings::Extend => list_extend(list, args, vm),
        StaticStrings::Index => list_index(list, args, vm),
        StaticStrings::Count => list_count(list, args, vm),
        StaticStrings::Reverse => {
            args.check_zero_args("list.reverse", heap)?;
            match &mut list.get_mut(vm.heap).storage {
                ListStorage::Empty => {}
                ListStorage::Packed(items) => items.reverse(),
                ListStorage::Generic(items) => items.reverse(),
            }
            Ok(Value::None)
        }
        // Note: list.sort is handled by py_call_attr which intercepts it before reaching here
        _ => {
            args.drop_with(heap);
            Err(ExcType::attribute_error(Type::List, method.into()))
        }
    }
}

/// Implements Python's `list.insert(index, item)` method.
fn list_insert<'h>(list: &HeapReadMut<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let (index_obj, item) = args.get_two_args("insert", vm.heap)?;
    defer_drop!(index_obj, vm);
    let mut item_guard = DropGuard::new(item, vm);
    let vm = item_guard.ctx();
    // Python's insert() handles negative indices by adding len
    // If still negative after adding len, clamps to 0
    // If >= len, appends to end
    let index_i64 = index_obj.as_int(vm)?;
    let len = list.get(vm.heap).len();
    let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
    let index = if index_i64 < 0 {
        // Negative index: add length, clamp to 0 if still negative
        let adjusted = index_i64 + len_i64;
        usize::try_from(adjusted).unwrap_or(0)
    } else {
        // Positive index: clamp to len if too large
        usize::try_from(index_i64).unwrap_or(len)
    };
    let (item, heap) = item_guard.into_parts();
    list.insert(heap, index, item)?;
    Ok(Value::None)
}

/// Implements Python's `list.pop([index])` method.
///
/// Removes the item at the given index (default: -1) and returns it.
/// Raises IndexError if the list is empty or the index is out of range.
fn list_pop<'h>(list: &HeapReadMut<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let Some(value) = args.get_zero_one_arg("list.pop", vm.heap)? else {
        let this = list.get_mut(vm.heap);
        let value = match &mut this.storage {
            ListStorage::Packed(PackedList::ByteInts(items)) => items.pop().map(|v| Value::Int(i64::from(v))),
            ListStorage::Packed(PackedList::Ints(items)) => items.pop().map(Value::Int),
            ListStorage::Packed(PackedList::Floats(items)) => items.pop().map(Value::Float),
            _ => {
                let last = this
                    .len()
                    .checked_sub(1)
                    .ok_or_else(ExcType::index_error_pop_empty_list)?;
                return Ok(this.remove_at(last));
            }
        };
        return value.ok_or_else(ExcType::index_error_pop_empty_list);
    };

    // Python checks the index type even on an empty list.
    let index = value.as_int(vm);
    value.drop_with(vm);
    let index_i64 = index?;

    // THEN check empty list
    if list.get(vm.heap).len() == 0 {
        return Err(ExcType::index_error_pop_empty_list());
    }

    // Normalize index
    let len = list.get(vm.heap).len();
    let len_i64 = i64::try_from(len).expect("list length exceeds i64::MAX");
    let normalized = if index_i64 < 0 { index_i64 + len_i64 } else { index_i64 };

    // Bounds check
    if normalized < 0 || normalized >= len_i64 {
        return Err(ExcType::index_error_pop_out_of_range());
    }

    // Remove and return the item
    let idx = usize::try_from(normalized).expect("index validated non-negative");
    Ok(list.get_mut(vm.heap).remove_at(idx))
}

/// Implements Python's `list.remove(value)` method.
///
/// Removes the first occurrence of value. Raises ValueError if not found.
fn list_remove<'h>(list: &HeapReadMut<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let value = args.get_one_arg("list.remove", vm.heap)?;
    defer_drop!(value, vm);

    let mut found_idx = None;
    {
        let iter = list.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, item)) = iter.next_with_index(vm)? {
            if value.py_eq(item, vm)? {
                found_idx = Some(i);
                break;
            }
        }
    }

    match found_idx {
        Some(idx) => {
            // A matching user `__eq__` may itself have shrunk the list, leaving
            // `idx` stale; CPython clamps the deletion (`list_ass_slice`), so a
            // gone index removes nothing rather than panicking.
            let this = list.get_mut(vm.heap);
            if idx < this.len() {
                let removed = this.remove_at(idx);
                removed.drop_with(vm.heap);
            }
            Ok(Value::None)
        }
        None => Err(ExcType::value_error_remove_not_in_list()),
    }
}

/// Implements Python's `list.clear()` method.
///
/// Removes all items from the list.
fn list_clear<'h>(list: &HeapReadMut<'h, List>, vm: &mut VM<'h>) {
    let this = list.get_mut(vm.heap);
    this.contains_refs = false;
    match &mut this.storage {
        ListStorage::Empty => {}
        ListStorage::Packed(items) => items.splice(0..items.len(), &[]),
        ListStorage::Generic(items) => {
            let mut detached = mem::take(items);
            for item in detached.drain(..) {
                item.drop_with(vm);
            }
            list.get_mut(vm.heap).storage = ListStorage::Generic(detached);
        }
    }
}

/// Implements Python's `list.copy()` method.
///
/// Returns a shallow copy of the list, preflighting the slot bytes like
/// `clone_all_items` so a huge copy fails with a graceful `MemoryError`.
fn list_copy(list: &List, heap: &Heap) -> RunResult<Value> {
    let copy = match &list.storage {
        ListStorage::Empty => List::default(),
        ListStorage::Packed(items) => {
            let tracker = &heap.tracker;
            let mut result = PackedList::new(items.len(), items.packing(), tracker)?;
            result.copy_range(items, 0..items.len(), tracker)?;
            List::from_packed(result)
        }
        ListStorage::Generic(_) => List::new(list.items().to_vec(heap)?),
    };
    let heap_id = heap.allocate(HeapData::List(copy));
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `list.extend(iterable)` method.
///
/// Extends the list by appending all items from the iterable.
fn list_extend<'h>(list: &HeapReadMut<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let iterable = args.get_one_arg("list.extend", vm.heap)?;
    let mut iterable = DropGuard::new(iterable, vm);
    let (value, vm) = iterable.as_parts();
    if let Some(HeapReadOutput::List(source)) = value.read_heap(vm)
        && list.try_extend_packed(vm, &source)?
    {
        return Ok(Value::None);
    }
    let (iterable, vm) = iterable.into_parts();
    let items: SmallVec<[_; 2]> = collect_owned_iterable(iterable, vm)?;

    list.extend_values(vm, items)?;

    Ok(Value::None)
}

/// Implements Python's `list.index(value[, start[, end]])` method.
///
/// Returns the index of the first occurrence of value.
/// Raises ValueError if the value is not found.
fn list_index<'h>(list: &HeapRead<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let pos_args = args.into_pos_only("list.index", vm.heap)?;
    defer_drop!(pos_args, vm);

    // Bounds are coerced before the length is read: the coercion may dispatch a
    // user `__index__` that mutates this list, and CPython normalizes against the
    // length *after* argument parsing. Reading it first would resolve a negative
    // bound against a stale length and search the wrong window.
    let (value, start_arg, end_arg) = match pos_args.as_slice() {
        [] => return Err(ExcType::type_error_at_least("list.index", 1, 0)),
        [value] => (value, None, None),
        [value, start_arg] => (value, Some(value_to_i64_bound(start_arg, vm)?), None),
        [value, start_arg, end_arg] => (
            value,
            Some(value_to_i64_bound(start_arg, vm)?),
            Some(value_to_i64_bound(end_arg, vm)?),
        ),
        other => return Err(ExcType::type_error_at_most("list.index", 3, other.len())),
    };

    let len = list.get(vm.heap).len();
    let start = start_arg.map_or(0, |i| normalize_sequence_index(i, len));
    let end = end_arg.map_or(len, |i| normalize_sequence_index(i, len)).max(start);

    // Nested short offset searches may never reach an iterator checkpoint.
    vm.heap.tracker.check_time()?;

    let mut iter = list.iter(vm)?;
    // Excluded prefix entries need neither equality calls nor reference-count traffic.
    iter.index = start;
    defer_drop_mut!(iter, vm);
    while let Some((idx, item)) = iter.next_with_index(vm)? {
        if idx >= end {
            // No further matches possible inside [start, end).
            break;
        }
        if idx >= start && value.py_eq(item, vm)? {
            let i64_idx = i64::try_from(idx).expect("index exceeds i64::MAX");
            return Ok(Value::Int(i64_idx));
        }
    }
    Err(ExcType::value_error_not_in_list())
}

/// Implements Python's `list.count(value)` method.
///
/// Returns the number of occurrences of value in the list.
fn list_count<'h>(list: &HeapRead<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let value = args.get_one_arg("list.count", vm.heap)?;
    defer_drop!(value, vm);

    let mut count: usize = 0;
    let iter = list.iter(vm)?;
    defer_drop_mut!(iter, vm);
    while let Some(item) = iter.next(vm)? {
        if value.py_eq(item, vm)? {
            count += 1;
        }
    }

    let count_i64 = i64::try_from(count).expect("count exceeds i64::MAX");
    Ok(Value::Int(count_i64))
}

/// Sorts detached items so reentrant Python initially sees an empty list.
/// Restores the items on error and rejects additions left by reentrant calls.
fn do_list_sort<'h>(list: &HeapReadMut<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> Result<(), RunError> {
    let mut args = DropGuard::new(args, vm);
    let vm = args.ctx();
    let repack = matches!(&list.get(vm.heap).storage, ListStorage::Packed(items) if items.len() != 0);
    if repack {
        list.promote(vm, 0)?;
    }
    let (args, vm) = args.into_parts();

    let mut sorted = mem::take(list.get_mut(vm.heap));
    let sort_result = match &mut sorted.storage {
        ListStorage::Generic(items) => parse_and_sort(items, args, vm),
        _ => parse_and_sort(&mut [], args, vm),
    };
    if repack {
        let ListStorage::Generic(items) = sorted.storage else {
            unreachable!("sort materialized the packed items")
        };
        sorted = List::from_values(items, vm.heap);
    }
    let during_sort = mem::replace(list.get_mut(vm.heap), sorted);
    let modified = during_sort.len() != 0;
    during_sort.drop_with(vm);

    sort_result?;
    if modified {
        Err(SimpleException::new_msg(ExcType::ValueError, "list modified during sort").into())
    } else {
        Ok(())
    }
}

fn checked_list_len(first: usize, second: usize) -> RunResult<usize> {
    first
        .checked_add(second)
        .filter(|len| isize::try_from(*len).is_ok())
        .ok_or_else(ExcType::allocation_too_large)
}

fn list_element_size(packing: Packing) -> usize {
    match packing {
        Packing::Generic => VALUE_SIZE,
        Packing::Int { width, .. } => width.size(),
        Packing::Float { .. } => size_of::<f64>(),
        // Bound logical length here and preflight bitmap bytes separately.
        _ => 1,
    }
}

fn insert_slot<T>(items: &mut Vec<T>, index: usize, value: T) {
    if index >= items.len() {
        items.push(value);
    } else {
        items.insert(index, value);
    }
}

fn check_slice_replacement(step: i64, count: usize, replacements: usize) -> RunResult<()> {
    if step != 1 && replacements != count {
        return Err(ExcType::value_error(format!(
            "attempt to assign sequence of size {replacements} to extended slice of size {count}"
        )));
    }
    Ok(())
}

fn slice_item_count(start: i64, stop: i64, step: i64) -> usize {
    let count = if step > 0 && start < stop {
        (stop - start - 1).unsigned_abs() / step.unsigned_abs() + 1
    } else if step < 0 && start > stop {
        (start - stop - 1).unsigned_abs() / step.unsigned_abs() + 1
    } else {
        0
    };
    usize::try_from(count).expect("slice length fits usize")
}

/// Writes a formatted sequence of values to a formatter.
///
/// This helper function is used to implement `__repr__` for sequence types like
/// lists and tuples. It writes items as comma-separated repr interns.
///
/// There is no length parameter: `get_item` is asked for indices from 0 until
/// it returns `None`. For an immutable sequence (tuple) that is simply its
/// fixed end; for a mutable one (list) it means a user `__repr__` growing or
/// shrinking the sequence mid-format is observed rather than panicking on a
/// stale bound — CPython's list repr re-checks the live length the same way.
///
/// # Arguments
/// * `start` - The opening character (e.g., '[' for lists, '(' for tuples)
/// * `end` - The closing character (e.g., ']' for lists, ')' for tuples)
/// * `get_item` - Returns an owned clone of the i-th value via brief heap access,
///   or `None` once `i` is out of range. The clone keeps the item alive across
///   user `__repr__` calls that may remove it from the container.
/// * `f` - The formatter to write to
/// * `vm` - The VM for resolving value references and looking up interned strings
/// * `heap_ids` - Set of heap IDs being repr'd (for cycle detection)
pub(crate) fn repr_sequence_fmt<'h>(
    start: char,
    end: char,
    get_item: impl Fn(&HeapReader<'h>, usize) -> Option<Value>,
    f: &mut impl Write,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    // Check depth limit before recursing
    let Ok(mut guard) = vm.recursion_guard() else {
        return Ok(f.write_str("...")?);
    };
    let vm = &mut *guard;

    f.write_char(start)?;
    // The item is fetched before the separator is written so nothing is emitted
    // for an index that turned out to be out of range. `repr_should_truncate` bounds
    // reprs that keep growing the sequence from inside a user `__repr__`.
    for i in 0.. {
        let Some(item) = get_item(vm.heap, i) else { break };
        defer_drop!(item, vm);
        if i > 0 {
            if repr_should_truncate(i, vm)? {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        item.py_repr_fmt(f, vm, heap_ids)?;
    }
    f.write_char(end)?;

    Ok(())
}

/// Writes the comma-separated reprs of an already-snapshotted item slice.
///
/// The slice is a refcount-bumped snapshot owned by the caller (deque, set),
/// so items are formatted by reference with no per-item clone; the caller owns
/// the recursion guard and any surrounding brackets. Live containers must go
/// through [`repr_sequence_fmt`] instead, which re-fetches (and clones) each
/// item so a mutating user `__repr__` is observed safely.
pub(crate) fn repr_items_fmt(
    items: &[Value],
    f: &mut impl Write,
    vm: &mut VM<'_>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            if repr_should_truncate(i, vm)? {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        item.py_repr_fmt(f, vm, heap_ids)?;
    }
    Ok(())
}

/// Polls the time and memory soft limits every 64th item of a container repr.
///
/// `Ok(true)` means the time limit ran out, so the caller writes `", ...[timeout]"`
/// and stops (tested in `resource_limits.rs`). A memory error propagates instead,
/// since a truncated repr would let the feed carry on past `max_memory`.
pub(crate) fn repr_should_truncate(i: usize, vm: &VM<'_>) -> RunResult<bool> {
    match vm.heap.tracker.check_memory_time_every(i) {
        Ok(()) => Ok(false),
        Err(ResourceError::Time { .. }) => Ok(true),
        Err(err) => Err(err.into()),
    }
}

/// Iterates over a list while observing changes to its current contents.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ListIterator {
    /// Owned reference to the list under iteration.
    list: HeapId,
    /// Index of the next item to yield.
    index: usize,
}

impl ListIterator {
    /// Creates an iterator which takes ownership of one reference to `list`.
    pub(crate) fn new(list: HeapId) -> Self {
        Self { list, index: 0 }
    }

    /// Returns the list kept alive by this iterator.
    pub(crate) fn list_id(&self) -> HeapId {
        self.list
    }

    /// Returns the number of items remaining in the list's current contents.
    pub(crate) fn size_hint(&self, heap: &Heap) -> usize {
        let HeapData::List(list) = heap.get(self.list) else {
            unreachable!("list iterator must reference a list")
        };
        list.len().saturating_sub(self.index)
    }
}

impl HeapItem for ListIterator {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.list);
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ListIterator> {
    fn py_is_iterator(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, _: &VM<'h>) -> TypeRef {
        Type::ListIterator.into()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let (list_id, index) = {
            let iterator = self.get(vm.heap);
            (iterator.list, iterator.index)
        };
        let item = match vm.heap.get(list_id) {
            HeapData::List(list) => list.get_cloned(index, vm.heap),
            _ => unreachable!("list iterator must reference a list"),
        };
        if item.is_some() {
            self.get_mut(vm.heap).index += 1;
        }
        Ok(item)
    }
}

impl<'h> PyDeepCopy<'h> for HeapRead<'h, List> {
    /// Copies into a shell memoized before its items are, so a list holding
    /// itself resolves to the shell rather than recursing forever.
    #[inline(never)]
    fn py_deep_copy(&self, source: &Value, memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value> {
        let copy_id = vm.heap.allocate(HeapData::List(List::new(Vec::new())));
        let mut guard = DropGuard::new(Value::Ref(copy_id), vm);
        let (copy, vm) = guard.as_parts_mut();
        memo.insert(source, copy, vm)?;
        let len = self.get(vm.heap).len();
        vm.heap.tracker.check_allocation(len.saturating_mul(VALUE_SIZE))?;
        // The length is never cached across a step: copying an item runs Python
        // (a `__deepcopy__` hook, or a `__hash__` reached while filling a nested
        // dict), which can shrink the source. Reading it live also visits items
        // appended mid-walk, which is what CPython's `for a in x` does.
        for index in 0.. {
            let (_, vm) = guard.as_parts_mut();
            vm.heap.tracker.check_time_every(index)?;
            let Some(item) = self.try_clone_item(index, vm) else {
                break;
            };
            let copied = deep_copy(&item, memo, vm);
            item.drop_with(vm);
            let copied = copied?;
            let HeapReadOutput::List(dest) = vm.heap.read(copy_id) else {
                unreachable!("copy was allocated as a list")
            };
            dest.append(vm, copied)?;
        }
        let (copy, _) = guard.into_parts();
        Ok(copy)
    }
}

#[cfg(test)]
mod tests {
    use monty_types::{PrintWriter, ResourceTracker};
    use num_bigint::BigInt;

    use super::*;
    use crate::{
        heap::{Heap, HeapReader},
        run::{Program, SessionTables},
        types::LongInt,
    };

    #[test]
    fn packed_float_reads_preserve_bits() {
        let heap = Heap::new(16, ResourceTracker::default());
        let bits = [0x8000_0000_0000_0000, 0x7ff8_0000_0000_0123, 0xfff8_0000_0000_0456];
        for nullable in [false, true] {
            let mut values: Vec<_> = bits.iter().map(|&bits| Value::Float(f64::from_bits(bits))).collect();
            if nullable {
                values.push(Value::None);
            }
            let list = List::from_values(values, &heap);
            assert_eq!(list.storage.packing(), Packing::Float { nullable });
            assert!(matches!(
                (nullable, list.items()),
                (true, SequenceItems::Packed(_)) | (false, SequenceItems::Floats(_))
            ));
            for (index, expected) in bits.into_iter().enumerate() {
                let Some(Value::Float(value)) = list.get_cloned(index, &heap) else {
                    panic!("expected a float at {index}");
                };
                assert_eq!(value.to_bits(), expected);
            }
        }
    }

    #[test]
    fn drained_lists_reuse_compatible_storage() {
        let mut heap = Heap::new(16, ResourceTracker::default());
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        for (seed, refill) in [
            (Value::Int(255), Value::Int(0)),
            (Value::Int(-1), Value::Int(1)),
            (Value::Bool(true), Value::Bool(false)),
            (Value::None, Value::None),
            (Value::Float(0.5), Value::Float(1.5)),
            (Value::Ellipsis, Value::Ellipsis),
        ] {
            let values = (0..80).map(|_| seed.clone_with_heap(&heap)).collect();
            let initial = List::from_values(values, &heap);
            let packing = initial.storage.packing();
            let capacity = initial.storage.capacity();
            let id = heap.allocate(HeapData::List(initial));
            HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
                let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
                let HeapReadOutput::List(list) = vm.heap.read(id) else {
                    panic!("expected list");
                };
                for _ in 0..80 {
                    list_pop(&list, ArgValues::Empty, &mut vm).unwrap().drop_with(&mut vm);
                }
                list.append(&mut vm, refill).unwrap();
                list_clear(&list, &mut vm);
                do_list_sort(&list, ArgValues::Empty, &mut vm).unwrap();
                let storage = &list.get(vm.heap).storage;
                assert_eq!(storage.packing(), packing);
                assert_eq!(storage.capacity(), capacity);
            });
            Value::Ref(id).drop_with(&mut heap);
        }
    }

    #[test]
    fn drained_generic_list_can_switch_to_packed_storage() {
        let mut heap = Heap::new(16, ResourceTracker::default());
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        let id = heap.allocate(HeapData::List(List::new(vec![Value::Ellipsis])));
        HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let HeapReadOutput::List(list) = vm.heap.read(id) else {
                panic!("expected list");
            };
            list_clear(&list, &mut vm);
            list.append(&mut vm, Value::Float(0.5)).unwrap();
            assert_eq!(list.get(vm.heap).storage.packing(), Packing::Float { nullable: false });
        });
        Value::Ref(id).drop_with(&mut heap);
    }

    /// Creates a heap with a list and a LongInt index, bypassing into_value() demotion.
    ///
    /// This allows testing the defensive code path where a LongInt contains an i64-fitting value.
    fn create_heap_with_list_and_longint(list_items: Vec<Value>, index_value: BigInt) -> (Heap, HeapId, HeapId) {
        let heap = Heap::new(16, ResourceTracker::default());
        let list = List::new(list_items);
        let list_id = heap.allocate(HeapData::List(list));
        let long_int = LongInt::new(index_value);
        let index_id = heap.allocate(HeapData::LongInt(long_int));
        (heap, list_id, index_id)
    }

    /// Tests py_setitem with a LongInt index that fits in i64.
    ///
    /// This is a defensive code path - normally unreachable because LongInt::into_value()
    /// demotes i64-fitting values to Value::Int. However, it could be reached via
    /// deserialization of crafted snapshot data.
    #[test]
    fn py_setitem_longint_fits_in_i64() {
        let (mut heap, list_id, index_id) =
            create_heap_with_list_and_longint(vec![Value::Int(10), Value::Int(20), Value::Int(30)], BigInt::from(1));
        let mut tables = SessionTables::default();
        let program = Program::for_tests();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_ok());

        // Verify the list was updated by checking it matches expected Int value
        let HeapData::List(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(list.get_cloned(1, &heap), Some(Value::Int(99))));

        // Clean up
        Value::Ref(list_id).drop_with(&mut heap);
    }

    /// Tests py_setitem with a negative LongInt index that fits in i64.
    #[test]
    fn py_setitem_longint_negative_fits_in_i64() {
        let (mut heap, list_id, index_id) = create_heap_with_list_and_longint(
            vec![Value::Int(10), Value::Int(20), Value::Int(30)],
            BigInt::from(-1), // Last element
        );
        let mut tables = SessionTables::default();
        let program = Program::for_tests();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_ok());

        // Verify the last element was updated
        let HeapData::List(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(list.get_cloned(2, &heap), Some(Value::Int(99))));

        Value::Ref(list_id).drop_with(&mut heap);
    }

    /// Tests py_setitem with i64::MAX as a LongInt index.
    #[test]
    fn py_setitem_longint_at_i64_max() {
        let (mut heap, list_id, index_id) =
            create_heap_with_list_and_longint(vec![Value::Int(10)], BigInt::from(i64::MAX));
        let mut tables = SessionTables::default();
        let program = Program::for_tests();

        let key = Value::Ref(index_id);
        let new_value = Value::Int(99);
        heap.inc_ref(index_id);

        let result = HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_err());

        Value::Ref(list_id).drop_with(&mut heap);
    }
}
