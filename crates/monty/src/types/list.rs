use std::{cmp::Ordering, fmt::Write, mem, ops::Deref};

use monty_types::{ResourceError, ResourceTracker};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

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
        HeapReadOutput, HeapReader,
    },
    intern::StaticStrings,
    modules::copy::{Memo, PyDeepCopy, deep_copy},
    resource_checks::check_repeat_size,
    sorting::parse_and_sort,
    types::{
        LazyHeapSet, Type,
        long_int::{repeat_count, wide_i128_into_value},
        slice::{normalize_sequence_index, value_to_i64_bound},
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// The legacy list payload. Its fields and serde names retain version-13 dump compatibility.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct List {
    #[serde(rename = "I")]
    items: Vec<Value>,
    #[serde(rename = "C")]
    contains_refs: bool,
}

impl List {
    pub fn new(items: Vec<Value>) -> Self {
        Self {
            contains_refs: items.iter().any(|v| matches!(v, Value::Ref(_))),
            items,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }
    pub fn get(&self, index: usize) -> Option<Item<'_>> {
        self.items.get(index).map(Item::Borrowed)
    }
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Item<'_>> + DoubleEndedIterator + Clone {
        self.items.iter().map(Item::Borrowed)
    }
    pub fn extend_owned(&mut self, values: Vec<Value>) {
        self.contains_refs |= values.iter().any(|v| matches!(v, Value::Ref(_)));
        self.items.extend(values);
    }
    pub fn swap(&mut self, a: usize, b: usize) {
        self.items.swap(a, b);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for List {
    fn drop_with(self, ctx: &mut C) {
        self.items.drop_with(ctx);
    }
}

impl HeapItem for List {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if !self.contains_refs {
            return;
        }
        for value in &mut self.items {
            if let Value::Ref(id) = value {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                value.dec_ref_forget();
            }
        }
    }
}

/// Shares Python list operations while each heap payload keeps its concrete type for its lifetime.
pub(crate) trait ListStorage: Default {
    fn from_values(values: Vec<Value>) -> Self;
    fn specialized(&self) -> Option<&SpecializedList> {
        None
    }

    fn specialized_mut(&mut self) -> Option<&mut SpecializedList> {
        None
    }

    fn generic(&self) -> &List;
    fn generic_mut(&mut self, additional: usize) -> &mut List;
    fn into_values(self) -> Vec<Value>;
    fn discard<C: ContainsHeap>(self, ctx: &mut C);

    fn len(&self) -> usize {
        self.specialized()
            .map_or_else(|| self.generic().len(), SpecializedList::len)
    }

    fn is_empty(&self) -> bool {
        self.specialized()
            .map_or_else(|| self.generic().is_empty(), SpecializedList::is_empty)
    }

    fn contains_refs(&self) -> bool {
        self.specialized()
            .map_or_else(|| self.generic().contains_refs(), SpecializedList::contains_refs)
    }

    fn get(&self, index: usize) -> Option<Item<'_>> {
        self.specialized()
            .map_or_else(|| self.generic().get(index), |list| list.get(index))
    }

    fn iter(&self) -> impl ExactSizeIterator<Item = Item<'_>> + DoubleEndedIterator + Clone {
        (0..self.len()).map(|i| self.get(i).expect("index within list"))
    }

    fn element_size(&self) -> usize {
        self.specialized().map_or(VALUE_SIZE, SpecializedList::element_size)
    }

    fn check_insert(&self, value: &Value, tracker: &ResourceTracker) -> Result<(), ResourceError> {
        if let Some(list) = self.specialized() {
            return list.check_insert(value, tracker);
        }
        tracker.check_growth(self.len(), self.generic().items.capacity(), VALUE_SIZE)
    }

    fn check_replace(&self, value: &Value, tracker: &ResourceTracker) -> Result<(), ResourceError> {
        self.specialized()
            .map_or(Ok(()), |list| list.check_replace(value, tracker))
    }

    fn check_extend(&self, values: &[Value], tracker: &ResourceTracker) -> Result<(), ResourceError> {
        if let Some(list) = self.specialized() {
            return list.check_extend(values, tracker);
        }
        let additional = self
            .len()
            .saturating_add(values.len())
            .saturating_sub(self.generic().items.capacity());
        tracker.check_pending_allocation(additional.saturating_mul(VALUE_SIZE))
    }

    fn push(&mut self, value: Value) {
        if let Some(list) = self.specialized_mut() {
            list.push(value);
            return;
        }
        let list = self.generic_mut(0);
        list.contains_refs |= matches!(value, Value::Ref(_));
        list.items.push(value);
    }

    fn insert(&mut self, index: usize, value: Value) {
        if let Some(list) = self.specialized_mut() {
            list.insert(index, value);
            return;
        }
        let list = self.generic_mut(0);
        list.contains_refs |= matches!(value, Value::Ref(_));
        list.items.insert(index.min(list.items.len()), value);
    }

    fn replace(&mut self, index: usize, value: &mut Value) {
        if let Some(list) = self.specialized_mut() {
            list.replace(index, value);
            return;
        }
        let list = self.generic_mut(0);
        list.contains_refs |= matches!(value, Value::Ref(_));
        mem::swap(&mut list.items[index], value);
    }

    fn extend_owned(&mut self, values: Vec<Value>) {
        if let Some(list) = self.specialized_mut() {
            list.extend_owned(values);
            return;
        }
        self.generic_mut(0).extend_owned(values);
    }

    fn remove(&mut self, index: usize) -> Value {
        if let Some(list) = self.specialized_mut() {
            return list.remove(index);
        }
        self.generic_mut(0).items.remove(index)
    }

    fn reverse(&mut self) {
        if let Some(list) = self.specialized_mut() {
            list.reverse();
            return;
        }
        self.generic_mut(0).items.reverse();
    }

    fn copy(&self, heap: &Heap) -> SpecializedList {
        if let Some(list) = self.specialized() {
            return list.copy(heap);
        }
        SpecializedList::Generic(List {
            items: self.iter().map(|v| v.clone_with_heap(heap)).collect(),
            contains_refs: self.contains_refs(),
        })
    }

    fn concat_element_size<U: ListStorage>(&self, other: &U) -> usize {
        if let (Some(a), Some(b)) = (self.specialized(), other.specialized()) {
            return a.concat_element_size(b);
        }
        VALUE_SIZE
    }

    fn concat<U: ListStorage>(&self, other: &U, heap: &Heap) -> SpecializedList {
        if let (Some(a), Some(b)) = (self.specialized(), other.specialized()) {
            return a.concat(b, heap);
        }
        SpecializedList::Generic(List {
            items: self
                .iter()
                .chain(other.iter())
                .map(|v| v.clone_with_heap(heap))
                .collect(),
            contains_refs: self.contains_refs() || other.contains_refs(),
        })
    }
}

impl ListStorage for List {
    fn from_values(values: Vec<Value>) -> Self {
        Self::new(values)
    }

    fn generic(&self) -> &List {
        self
    }

    fn generic_mut(&mut self, _: usize) -> &mut List {
        self
    }

    fn into_values(self) -> Vec<Value> {
        self.items
    }

    fn discard<C: ContainsHeap>(self, ctx: &mut C) {
        self.drop_with(ctx);
    }
}

impl ListStorage for SpecializedList {
    fn from_values(values: Vec<Value>) -> Self {
        Self::new(values)
    }

    fn specialized(&self) -> Option<&SpecializedList> {
        Some(self)
    }

    fn specialized_mut(&mut self) -> Option<&mut SpecializedList> {
        Some(self)
    }

    fn generic(&self) -> &List {
        let Self::Generic(list) = self else {
            unreachable!("generic storage required")
        };
        list
    }

    fn generic_mut(&mut self, additional: usize) -> &mut List {
        self.promote(additional);
        let Self::Generic(list) = self else {
            unreachable!("promoted to generic storage")
        };
        list
    }

    fn into_values(self) -> Vec<Value> {
        self.into_values()
    }

    fn discard<C: ContainsHeap>(self, ctx: &mut C) {
        self.drop_with(ctx);
    }
}

/// Python list with compact numeric buffers and generic reference-owning storage.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) enum SpecializedList {
    #[default]
    #[serde(rename = "E")]
    Empty,
    #[serde(rename = "I")]
    Int(Vec<i64>),
    #[serde(rename = "F")]
    Float(Vec<f64>),
    #[serde(rename = "G")]
    Generic(List),
}

impl SpecializedList {
    /// False for numeric buffers and generic buffers that have never held references.
    #[inline]
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        matches!(
            self,
            Self::Generic(List {
                contains_refs: true,
                ..
            })
        )
    }
}

impl<'h, T: ListStorage> HeapRead<'h, T> {
    fn eq_list<U: ListStorage>(&self, other: &HeapRead<'h, U>, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
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

    fn concat_list<U: ListStorage>(&self, other: &HeapRead<'h, U>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        let size = a.concat_element_size(b);
        vm.heap
            .tracker
            .check_allocation(a.len().saturating_add(b.len()).saturating_mul(size))?;
        let result = a.concat(b, vm.heap);
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::SpecializedList(result)))))
    }

    /// Collects replacements before assignment to support self-assignment and avoid
    /// partial writes on iterator failure. Bounds are normalized after collection
    /// because the iterator may have resized the target.
    fn setitem_slice(&mut self, slice: &super::Slice, value: &Value, vm: &mut VM<'h>) -> RunResult<()> {
        // Reject a zero step before executing the replacement iterator.
        slice.indices(self.get(vm.heap).len())?;

        // Collect replacements before mutation to support self-assignment.
        let items = match value.read_heap(vm) {
            Some(HeapReadOutput::List(list)) => list.clone_all_items(vm)?,
            Some(HeapReadOutput::SpecializedList(list)) => list.clone_all_items(vm)?,
            Some(HeapReadOutput::Tuple(tuple)) => tuple.cloned_items(vm)?,
            _ => {
                let iterator = value.py_iter(vm).map_err(|error| {
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
        let count = if step > 0 && start < stop {
            usize::try_from((stop - start - 1) / step + 1).expect("slice length fits usize")
        } else if step < 0 && start > stop {
            usize::try_from((start - stop - 1).unsigned_abs() / step.unsigned_abs() + 1)
                .expect("slice length fits usize")
        } else {
            0
        };

        // Only step 1 permits resizing; other steps require one value per selected position.
        if step != 1 && items.len() != count {
            return Err(ExcType::value_error(format!(
                "attempt to assign sequence of size {} to extended slice of size {count}",
                items.len()
            )));
        }

        let growth = items.len().saturating_sub(count);
        if !self
            .get(vm.heap)
            .specialized()
            .is_none_or(|s| matches!(s, SpecializedList::Generic(_)))
        {
            // Slice assignment uses generic storage; preflight conversion while the numeric buffer is live.
            vm.heap.tracker.check_allocation(
                self.get(vm.heap)
                    .len()
                    .saturating_add(growth)
                    .max(4)
                    .saturating_mul(VALUE_SIZE),
            )?;
        }

        if items.len() == count {
            let List {
                items: target,
                contains_refs,
            } = self.get_mut(vm.heap).generic_mut(0);
            *contains_refs |= items.iter().any(|item| matches!(item, Value::Ref(_)));
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
            let growth = items.len().saturating_sub(count);
            vm.heap
                .tracker
                .check_allocation(growth.saturating_add(count).saturating_mul(VALUE_SIZE))?;

            let List {
                items: target,
                contains_refs,
            } = self.get_mut(vm.heap).generic_mut(growth);
            *contains_refs |= items.iter().any(|item| matches!(item, Value::Ref(_)));
            let start = usize::try_from(start).expect("positive slice start is non-negative");
            let removed: Vec<Value> = target.splice(start..start + count, items.drain(..)).collect();
            removed.drop_with(vm);
        }

        Ok(())
    }
}

/// A numeric element materialized on the stack, or a borrowed generic element.
pub(crate) enum Item<'a> {
    Immediate(Value),
    Borrowed(&'a Value),
}

impl Deref for Item<'_> {
    type Target = Value;

    fn deref(&self) -> &Value {
        match self {
            Self::Immediate(value) => value,
            Self::Borrowed(value) => value,
        }
    }
}

impl SpecializedList {
    /// Takes ownership of values without incrementing their reference counts.
    pub fn new(values: Vec<Value>) -> Self {
        if values.is_empty() {
            Self::Empty
        } else if values.iter().all(|v| matches!(v, Value::Int(_))) {
            let mut ints: Vec<i64> = values
                .into_iter()
                .map(|v| match v {
                    Value::Int(i) => i,
                    _ => unreachable!(),
                })
                .collect();
            ints.shrink_to_fit();
            Self::Int(ints)
        } else if values.iter().all(|v| matches!(v, Value::Float(_))) {
            let mut floats: Vec<f64> = values
                .into_iter()
                .map(|v| match v {
                    Value::Float(f) => f,
                    _ => unreachable!(),
                })
                .collect();
            floats.shrink_to_fit();
            Self::Float(floats)
        } else {
            Self::Generic(List {
                contains_refs: values.iter().any(|v| matches!(v, Value::Ref(_))),
                items: values,
            })
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Int(v) => v.len(),
            Self::Float(v) => v.len(),
            Self::Generic(List { items: v, .. }) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<Item<'_>> {
        match self {
            Self::Empty => None,
            Self::Int(v) => v.get(index).map(|&i| Item::Immediate(Value::Int(i))),
            Self::Float(v) => v.get(index).map(|&f| Item::Immediate(Value::Float(f))),
            Self::Generic(List { items: v, .. }) => v.get(index).map(Item::Borrowed),
        }
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = Item<'_>> + DoubleEndedIterator + Clone {
        (0..self.len()).map(|i| self.get(i).expect("index within storage"))
    }

    fn accepts(&self, value: &Value) -> bool {
        match self {
            Self::Empty | Self::Generic(_) => true,
            Self::Int(_) => matches!(value, Value::Int(_)),
            Self::Float(_) => matches!(value, Value::Float(_)),
        }
    }

    fn element_size(&self) -> usize {
        match self {
            Self::Empty | Self::Int(_) | Self::Float(_) => 8,
            Self::Generic(_) => VALUE_SIZE,
        }
    }

    fn capacity(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Int(v) => v.capacity(),
            Self::Float(v) => v.capacity(),
            Self::Generic(List { items: v, .. }) => v.capacity(),
        }
    }

    /// Promotion allocates the entire generic buffer while the numeric buffer is live.
    fn check_insert(&self, value: &Value, tracker: &ResourceTracker) -> Result<(), ResourceError> {
        if !self.accepts(value) {
            return tracker.check_allocation(self.len().saturating_add(1).max(4).saturating_mul(VALUE_SIZE));
        }
        let size = if matches!(self, Self::Empty) && !matches!(value, Value::Int(_) | Value::Float(_)) {
            VALUE_SIZE
        } else {
            self.element_size()
        };
        tracker.check_growth(self.len(), self.capacity(), size)
    }

    fn check_replace(&self, value: &Value, tracker: &ResourceTracker) -> Result<(), ResourceError> {
        if self.accepts(value) {
            Ok(())
        } else {
            tracker.check_allocation(self.len().max(4).saturating_mul(VALUE_SIZE))
        }
    }

    fn check_extend(&self, values: &[Value], tracker: &ResourceTracker) -> Result<(), ResourceError> {
        if values.is_empty() || matches!(self, Self::Empty) {
            // Empty storage takes the admitted vector, compacting numeric values in place.
            return Ok(());
        }
        if values.iter().any(|v| !self.accepts(v)) {
            tracker.check_allocation(
                self.len()
                    .saturating_add(values.len())
                    .max(4)
                    .saturating_mul(VALUE_SIZE),
            )
        } else {
            let additional = self.len().saturating_add(values.len()).saturating_sub(self.capacity());
            tracker.check_pending_allocation(additional.saturating_mul(self.element_size()))
        }
    }

    /// Callers preflight before changing representation; no Python runs during conversion.
    fn promote(&mut self, additional: usize) {
        if matches!(self, Self::Generic(_)) {
            return;
        }
        let mut values = Vec::with_capacity(self.len().saturating_add(additional).max(4));
        match mem::take(self) {
            Self::Int(v) => values.extend(v.into_iter().map(Value::Int)),
            Self::Float(v) => values.extend(v.into_iter().map(Value::Float)),
            Self::Empty => {}
            Self::Generic(_) => unreachable!(),
        }
        *self = Self::Generic(List {
            items: values,
            contains_refs: false,
        });
    }

    fn push(&mut self, value: Value) {
        if matches!(self, Self::Empty) {
            *self = match &value {
                Value::Int(_) => Self::Int(Vec::new()),
                Value::Float(_) => Self::Float(Vec::new()),
                _ => Self::Generic(List {
                    items: Vec::new(),
                    contains_refs: false,
                }),
            };
        }
        if !self.accepts(&value) {
            self.promote(1);
        }
        match (self, value) {
            (Self::Int(v), Value::Int(i)) => v.push(i),
            (Self::Float(v), Value::Float(f)) => v.push(f),
            (
                Self::Generic(List {
                    items: v,
                    contains_refs,
                }),
                value,
            ) => {
                *contains_refs |= matches!(value, Value::Ref(_));
                v.push(value);
            }
            _ => unreachable!("storage accepts value"),
        }
    }

    fn insert(&mut self, index: usize, value: Value) {
        if index >= self.len() {
            self.push(value);
            return;
        }
        if !self.accepts(&value) {
            self.promote(1);
        }
        match (self, value) {
            (Self::Int(v), Value::Int(i)) => v.insert(index, i),
            (Self::Float(v), Value::Float(f)) => v.insert(index, f),
            (
                Self::Generic(List {
                    items: v,
                    contains_refs,
                }),
                value,
            ) => {
                *contains_refs |= matches!(value, Value::Ref(_));
                v.insert(index, value);
            }
            _ => unreachable!("storage accepts value"),
        }
    }

    fn replace(&mut self, index: usize, value: &mut Value) {
        if !self.accepts(value) {
            self.promote(0);
        }
        match self {
            Self::Int(v) => {
                let Value::Int(i) = value else { unreachable!() };
                mem::swap(&mut v[index], i);
            }
            Self::Float(v) => {
                let Value::Float(f) = value else { unreachable!() };
                mem::swap(&mut v[index], f);
            }
            Self::Generic(List {
                items: v,
                contains_refs,
            }) => {
                *contains_refs |= matches!(*value, Value::Ref(_));
                mem::swap(&mut v[index], value);
            }
            Self::Empty => unreachable!("index validated"),
        }
    }

    /// Takes ownership of preflighted values, preserving their reference counts.
    pub fn extend_owned(&mut self, values: Vec<Value>) {
        if values.is_empty() {
            return;
        }
        if matches!(self, Self::Empty) {
            *self = Self::new(values);
            return;
        }
        if values.iter().any(|v| !self.accepts(v)) {
            self.promote(values.len());
        }
        match self {
            Self::Int(v) => {
                v.reserve_exact(values.len());
                v.extend(values.into_iter().map(|v| match v {
                    Value::Int(i) => i,
                    _ => unreachable!(),
                }));
            }
            Self::Float(v) => {
                v.reserve_exact(values.len());
                v.extend(values.into_iter().map(|v| match v {
                    Value::Float(f) => f,
                    _ => unreachable!(),
                }));
            }
            Self::Generic(List {
                items: v,
                contains_refs,
            }) => {
                *contains_refs |= values.iter().any(|v| matches!(v, Value::Ref(_)));
                v.reserve_exact(values.len());
                v.extend(values);
            }
            Self::Empty => unreachable!(),
        }
    }

    fn remove(&mut self, index: usize) -> Value {
        match self {
            Self::Int(v) => Value::Int(v.remove(index)),
            Self::Float(v) => Value::Float(v.remove(index)),
            Self::Generic(List { items: v, .. }) => v.remove(index),
            Self::Empty => unreachable!("index validated"),
        }
    }

    fn reverse(&mut self) {
        match self {
            Self::Empty => {}
            Self::Int(v) => v.reverse(),
            Self::Float(v) => v.reverse(),
            Self::Generic(List { items: v, .. }) => v.reverse(),
        }
    }

    pub fn swap(&mut self, a: usize, b: usize) {
        match self {
            Self::Empty => unreachable!("indices validated"),
            Self::Int(v) => v.swap(a, b),
            Self::Float(v) => v.swap(a, b),
            Self::Generic(List { items: v, .. }) => v.swap(a, b),
        }
    }

    fn copy(&self, heap: &Heap) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Int(v) => Self::Int(v.clone()),
            Self::Float(v) => Self::Float(v.clone()),
            Self::Generic(List {
                items: v,
                contains_refs,
            }) => Self::Generic(List {
                items: v.iter().map(|v| v.clone_with_heap(heap)).collect(),
                contains_refs: *contains_refs,
            }),
        }
    }

    fn concat_element_size(&self, other: &Self) -> usize {
        if self.is_empty() {
            other.element_size()
        } else if other.is_empty() {
            self.element_size()
        } else {
            match (self, other) {
                (Self::Int(_), Self::Int(_)) | (Self::Float(_), Self::Float(_)) => 8,
                _ => VALUE_SIZE,
            }
        }
    }

    fn concat(&self, other: &Self, heap: &Heap) -> Self {
        if self.is_empty() {
            return other.copy(heap);
        }
        if other.is_empty() {
            return self.copy(heap);
        }
        let len = self.len() + other.len();
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => {
                let mut values = Vec::with_capacity(len);
                values.extend_from_slice(a);
                values.extend_from_slice(b);
                Self::Int(values)
            }
            (Self::Float(a), Self::Float(b)) => {
                let mut values = Vec::with_capacity(len);
                values.extend_from_slice(a);
                values.extend_from_slice(b);
                Self::Float(values)
            }
            _ => {
                let mut values = Vec::with_capacity(len);
                values.extend(self.iter().chain(other.iter()).map(|v| v.clone_with_heap(heap)));
                Self::Generic(List {
                    items: values,
                    contains_refs: self.contains_refs() || other.contains_refs(),
                })
            }
        }
    }

    fn into_values(self) -> Vec<Value> {
        match self {
            Self::Empty => Vec::new(),
            Self::Int(v) => v.into_iter().map(Value::Int).collect(),
            Self::Float(v) => v.into_iter().map(Value::Float).collect(),
            Self::Generic(List { items: v, .. }) => v,
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for SpecializedList {
    fn drop_with(self, ctx: &mut C) {
        if let Self::Generic(List { items: values, .. }) = self {
            values.drop_with(ctx);
        }
    }
}

impl<'h, T: ListStorage> HeapRead<'h, T> {
    /// Returns None when storage or the start value requires generic iteration.
    pub(crate) fn sum(&self, start: &Value, vm: &VM<'h>) -> RunResult<Option<Value>> {
        match self.get(vm.heap).specialized() {
            Some(SpecializedList::Int(items)) => {
                let mut total = match start {
                    Value::Int(i) => i128::from(*i),
                    Value::Bool(b) if !items.is_empty() => i128::from(*b),
                    _ => return Ok(None),
                };
                // Even usize::MAX i64 elements plus an i64 start fit in i128.
                for chunk in items.chunks(64) {
                    vm.heap.tracker.check_time()?;
                    total += chunk.iter().map(|&i| i128::from(i)).sum::<i128>();
                }
                let result = match i64::try_from(total) {
                    Ok(i) => Value::Int(i),
                    Err(_) => wide_i128_into_value(total, vm.heap),
                };
                Ok(Some(result))
            }
            Some(SpecializedList::Float(items)) if !items.is_empty() => {
                let mut total = match start {
                    Value::Float(f) => *f,
                    Value::Int(i) => *i as f64,
                    Value::Bool(b) => f64::from(*b),
                    _ => return Ok(None),
                };
                // Preserve the generic reduction's left-to-right additions.
                for chunk in items.chunks(64) {
                    vm.heap.tracker.check_time()?;
                    for &value in chunk {
                        total += value;
                    }
                }
                Ok(Some(Value::Float(total)))
            }
            _ => Ok(None),
        }
    }

    /// Appends an element to the end of the list.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    ///
    /// A push that would grow the buffer past the memory limit fails with
    /// `MemoryError` and drops `item`, so the ownership transfer holds either way.
    pub fn append(&mut self, vm: &mut VM<'h>, item: Value) -> RunResult<()> {
        let mut guard = DropGuard::new(item, vm);
        let (item, vm) = guard.as_parts_mut();
        self.get(vm.heap).check_insert(item, &vm.heap.tracker)?;
        let (item, vm) = guard.into_parts();
        let list = self.get_mut(vm.heap);
        list.push(item);
        Ok(())
    }

    pub(crate) fn extend_items(&mut self, vm: &mut VM<'h>, items: Vec<Value>) -> RunResult<()> {
        let mut guard = DropGuard::new(items, vm);
        let (items, vm) = guard.as_parts_mut();
        self.get(vm.heap).check_extend(items, &vm.heap.tracker)?;
        let (items, vm) = guard.into_parts();
        self.get_mut(vm.heap).extend_owned(items);
        Ok(())
    }

    /// Inserts an element at the specified index.
    ///
    /// The caller transfers ownership of `item` to the list. The item's refcount
    /// is NOT incremented here - the caller is responsible for ensuring the refcount
    /// was already incremented.
    ///
    /// Like [`append`](Self::append), an insert that would grow the buffer past the
    /// memory limit fails with `MemoryError` and drops `item`.
    ///
    /// # Arguments
    /// * `index` - The position to insert at (0-based). If index >= len(),
    ///   the item is appended to the end (matching Python semantics).
    pub fn insert(&mut self, vm: &mut VM<'h>, index: usize, item: Value) -> RunResult<()> {
        let mut guard = DropGuard::new(item, vm);
        let (item, vm) = guard.as_parts_mut();
        self.get(vm.heap).check_insert(item, &vm.heap.tracker)?;
        let (item, vm) = guard.into_parts();
        let list = self.get_mut(vm.heap);
        list.insert(index, item);
        Ok(())
    }
}

impl SpecializedList {
    /// Creates a list from the `list()` constructor call.
    ///
    /// - `list()` with no args returns an empty list
    /// - `list(iterable)` creates a list from any iterable (list, tuple, range, str, bytes, dict)
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_zero_one_arg("list", vm.heap)?;
        match value {
            None => {
                let heap_id = vm.heap.allocate(HeapData::SpecializedList(Self::new(Vec::new())));
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let items = collect_owned_iterable(v, vm)?;
                let heap_id = vm.heap.allocate(HeapData::SpecializedList(Self::new(items)));
                Ok(Value::Ref(heap_id))
            }
        }
    }
}

impl<'h, T: ListStorage> HeapRead<'h, T> {
    /// Handles slice-based indexing for lists.
    ///
    /// Returns a new list containing the selected elements.
    fn getitem_slice(&self, slice: &super::Slice, vm: &mut VM<'h>) -> RunResult<Value> {
        let storage = self.get(vm.heap);
        let (mut index, stop, step) = slice.indices(storage.len())?;
        let distance = if step > 0 {
            stop.saturating_sub(index)
        } else {
            index.saturating_sub(stop)
        };
        let count = usize::try_from(
            u64::try_from(distance.max(0))
                .expect("nonnegative distance")
                .div_ceil(step.unsigned_abs()),
        )
        .expect("slice length fits usize");
        vm.heap
            .tracker
            .check_allocation(count.saturating_mul(storage.element_size()))?;
        let numeric = match storage.specialized() {
            Some(SpecializedList::Int(items)) => {
                let mut result = Vec::with_capacity(count);
                for i in 0..count {
                    vm.heap.tracker.check_time_every(i)?;
                    result.push(items[usize::try_from(index).expect("slice index nonnegative")]);
                    index = index.saturating_add(step);
                }
                Some(SpecializedList::Int(result))
            }
            Some(SpecializedList::Float(items)) => {
                let mut result = Vec::with_capacity(count);
                for i in 0..count {
                    vm.heap.tracker.check_time_every(i)?;
                    result.push(items[usize::try_from(index).expect("slice index nonnegative")]);
                    index = index.saturating_add(step);
                }
                Some(SpecializedList::Float(result))
            }
            Some(SpecializedList::Empty) => Some(SpecializedList::Empty),
            _ => None,
        };
        if let Some(items) = numeric {
            return Ok(Value::Ref(vm.heap.allocate(HeapData::SpecializedList(items))));
        }
        let mut guard = DropGuard::new(Vec::with_capacity(count), vm);
        let (items, vm) = guard.as_parts_mut();
        while if step > 0 { index < stop } else { index > stop } {
            vm.heap.tracker.check_time_every(items.len())?;
            items.push(self.clone_item(usize::try_from(index).expect("slice index nonnegative"), vm));
            index = index.saturating_add(step);
        }
        let (items, vm) = guard.into_parts();
        let result = SpecializedList::Generic(List {
            items,
            contains_refs: self.get(vm.heap).contains_refs(),
        });
        Ok(Value::Ref(vm.heap.allocate(HeapData::SpecializedList(result))))
    }

    /// Clones the item at the given index with proper refcount management.
    ///
    /// Panics if `index` is out of bounds, so only use it where the length was
    /// read without a user callback running since — the comparison walks below
    /// use [`Self::try_clone_item`] instead.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h>) -> Value {
        self.get(vm.heap)
            .get(index)
            .expect("index validated")
            .clone_with_heap(vm.heap)
    }

    /// Clones the item at `index`, or `None` once the list has shrunk past it.
    ///
    /// For walks that run Python between steps and so cannot trust a length
    /// read earlier: the comparison walks index the *other* list directly
    /// while a user `__eq__` may be shrinking it, and `copy.deepcopy` runs a
    /// `__deepcopy__` hook between items.
    pub(crate) fn try_clone_item(&self, index: usize, vm: &mut VM<'h>) -> Option<Value> {
        self.get(vm.heap).get(index).map(|item| item.clone_with_heap(vm.heap))
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
    pub(crate) fn py_cmp<U: ListStorage>(&self, other: &HeapRead<'h, U>, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
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
        let len = self.get(vm.heap).len();
        vm.heap.tracker.check_allocation(len.saturating_mul(VALUE_SIZE))?;
        let mut result = Vec::with_capacity(len);
        for i in 0..len {
            result.push(self.clone_item(i, vm));
        }
        Ok(result)
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
    pub(crate) fn iter(&self, vm: &mut VM<'h>) -> RunResult<ListIter<'_, 'h, T>> {
        ListIter::new(self, vm)
    }
}

/// Stack-borrowed lending iterator over a [`List`]'s items.
///
/// Borrows a [`HeapRead`] for its lifetime, so the heap entry is pinned by the
/// reader count for the duration of iteration — no extra refcount on the
/// container is needed.
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
pub(crate) struct ListIter<'a, 'h, T: ListStorage> {
    list: &'a HeapRead<'h, T>,
    index: usize,
    token: RecursionToken,
    /// Most-recently-yielded item. `Value::Undefined` when nothing is held —
    /// drops on that variant are no-ops, so `next` can unconditionally drop
    /// the previous slot before fetching the new one.
    current: Value,
}

impl<'a, 'h, T: ListStorage> ListIter<'a, 'h, T> {
    fn new(list: &'a HeapRead<'h, T>, vm: &mut VM<'h>) -> RunResult<Self> {
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
        if self.index >= self.list.get(vm.heap).len() {
            return Ok(None);
        }
        self.current = self
            .list
            .get(vm.heap)
            .get(self.index)
            .expect("index validated")
            .clone_with_heap(vm.heap);
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

impl<'h, T: ListStorage, C: ContainsVM<'h>> DropWithContext<C> for ListIter<'_, 'h, T> {
    fn drop_with(self, container: &mut C) {
        self.current.drop_with(container);
        self.token.drop_with(container);
    }
}

impl<'h, T: ListStorage> PyTrait<'h> for HeapObjectRead<'h, T> {
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

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::List
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
            let slice = slice.clone();
            return self.getitem_slice(&slice, vm);
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(vm, Type::List)?;

        // Convert to usize, handling negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.get(vm.heap).len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_index_error());
        }

        // Return clone of the item with proper refcount increment
        // Safety: normalized_index is validated to be in [0, len) above
        let idx = usize::try_from(normalized_index).expect("list index validated non-negative");
        Ok(self
            .get(vm.heap)
            .get(idx)
            .expect("index validated")
            .clone_with_heap(vm.heap))
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

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt.
        // Note: The LongInt-to-i64 conversion is defensive code. In normal execution,
        // heap-allocated LongInt values always exceed i64 range because into_value()
        // demotes i64-fitting values to Value::Int. However, this could be reached via
        // deserialization of crafted snapshot data.
        let index = match *key {
            Value::Int(i) => i,
            Value::Bool(b) => i64::from(b),
            Value::Ref(heap_id) => {
                if let HeapData::LongInt(li) = vm.heap.get(heap_id) {
                    if let Some(i) = li.to_i64() {
                        i
                    } else {
                        return Err(ExcType::index_error_int_too_large());
                    }
                } else {
                    let key_type = key.py_type_name(vm);
                    return Err(ExcType::type_error_indices(Type::List, &key_type));
                }
            }
            _ => {
                let key_type = key.py_type_name(vm);
                return Err(ExcType::type_error_indices(Type::List, &key_type));
            }
        };

        // Normalize negative indices (Python-style: -1 = last element)
        let len = i64::try_from(self.get(vm.heap).len()).expect("list length exceeds i64::MAX");
        let normalized_index = if index < 0 { index + len } else { index };

        // Bounds check
        if normalized_index < 0 || normalized_index >= len {
            return Err(ExcType::list_assignment_index_error());
        }

        let idx = usize::try_from(normalized_index).expect("index validated non-negative");

        // Replace value (old one dropped by defer_drop_mut guard)
        self.get(vm.heap).check_replace(value, &vm.heap.tracker)?;
        self.get_mut(vm.heap).replace(idx, value);

        Ok(())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        match other.read_heap(vm) {
            Some(HeapReadOutput::List(other)) => self.eq_list(&other, vm),
            Some(HeapReadOutput::SpecializedList(other)) => self.eq_list(&other, vm),
            _ => Ok(None),
        }
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(!self.get(vm.heap).is_empty())
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        repr_sequence_fmt(
            '[',
            ']',
            |heap, i| self.get(heap).get(i).map(|item| item.clone_with_heap(heap)),
            f,
            vm,
            heap_ids,
        )
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match other.read_heap(vm) {
            Some(HeapReadOutput::List(other)) => self.concat_list(&other, vm),
            Some(HeapReadOutput::SpecializedList(other)) => self.concat_list(&other, vm),
            _ => Ok(None),
        }
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(count) = repeat_count(other, vm)? else {
            return Ok(None);
        };
        let value = self.get(vm.heap);
        if count == 0 || value.is_empty() {
            return Ok(Some(Value::Ref(
                vm.heap.allocate(HeapData::SpecializedList(SpecializedList::default())),
            )));
        }
        check_repeat_size(
            value.len().saturating_mul(value.element_size()),
            count,
            &vm.heap.tracker,
        )?;
        let numeric = match value.specialized() {
            Some(SpecializedList::Int(items)) => {
                let mut result = Vec::with_capacity(items.len() * count);
                for rep in 0..count {
                    vm.heap.tracker.check_time_every(rep)?;
                    result.extend_from_slice(items);
                }
                Some(SpecializedList::Int(result))
            }
            Some(SpecializedList::Float(items)) => {
                let mut result = Vec::with_capacity(items.len() * count);
                for rep in 0..count {
                    vm.heap.tracker.check_time_every(rep)?;
                    result.extend_from_slice(items);
                }
                Some(SpecializedList::Float(result))
            }
            _ => None,
        };
        if let Some(items) = numeric {
            return Ok(Some(Value::Ref(vm.heap.allocate(HeapData::SpecializedList(items)))));
        }
        let len = value.len();
        let mut guard = DropGuard::new(Vec::with_capacity(len * count), vm);
        let (result, vm) = guard.as_parts_mut();
        for rep in 0..count {
            vm.heap.tracker.check_time_every(rep)?;
            result.extend(self.get(vm.heap).iter().map(|value| value.clone_with_heap(vm.heap)));
        }
        let (result, vm) = guard.into_parts();
        let result = SpecializedList::Generic(List {
            items: result,
            contains_refs: self.get(vm.heap).contains_refs(),
        });
        Ok(Some(Value::Ref(vm.heap.allocate(HeapData::SpecializedList(result)))))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    fn py_iadd_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let Value::Ref(other_id) = other else {
            return Ok(false);
        };

        if *other_id == self.id() {
            // Self-extend: clone our own items with proper refcounting. Checked
            // at 2× — the temporary clone plus the equal-sized target growth —
            // up front, while no owned values need releasing on failure.
            let len = self.get(vm.heap).len();
            vm.heap.tracker.check_allocation(len.saturating_mul(2 * VALUE_SIZE))?;
            let items = self.clone_all_items(vm)?;
            self.extend_items(vm, items)?;
        } else {
            // Read source list via HeapRead, clone items into a temporary Vec.
            // Checked at 2× — the clone plus the target growth — see above.
            let source = vm.heap.read(*other_id);
            let source_items = match source {
                HeapReadOutput::List(source) => {
                    vm.heap
                        .tracker
                        .check_allocation(source.get(vm.heap).len().saturating_mul(2 * VALUE_SIZE))?;
                    source.clone_all_items(vm)?
                }
                HeapReadOutput::SpecializedList(source) => {
                    vm.heap
                        .tracker
                        .check_allocation(source.get(vm.heap).len().saturating_mul(2 * VALUE_SIZE))?;
                    source.clone_all_items(vm)?
                }
                _ => return Ok(false),
            };
            self.extend_items(vm, source_items)?;
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

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let list_id = self.id();
        let iterator = vm.heap.allocate(HeapData::ListIterator(ListIterator::new(list_id)));
        vm.heap.inc_ref(list_id);
        Ok(Value::Ref(iterator))
    }
}

impl HeapItem for SpecializedList {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - major GC optimization for lists of primitives
        if !self.contains_refs() {
            return;
        }
        let Self::Generic(List { items, .. }) = self else {
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
fn call_list_method<'h, T: ListStorage>(
    list: &mut HeapRead<'h, T>,
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
            list.get_mut(vm.heap).reverse();
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
fn list_insert<'h, T: ListStorage>(list: &mut HeapRead<'h, T>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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
fn list_pop<'h, T: ListStorage>(list: &mut HeapRead<'h, T>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let index_arg = args.get_zero_one_arg("list.pop", vm.heap)?;

    // Validate index type FIRST (if provided), matching Python's validation order.
    // Python raises TypeError for bad index type even on empty list.
    let index_i64 = if let Some(v) = index_arg {
        let result = v.as_int(vm);
        v.drop_with(vm);
        result?
    } else {
        -1
    };

    // THEN check empty list
    if list.get(vm.heap).is_empty() {
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
    Ok(list.get_mut(vm.heap).remove(idx))
}

/// Implements Python's `list.remove(value)` method.
///
/// Removes the first occurrence of value. Raises ValueError if not found.
fn list_remove<'h, T: ListStorage>(list: &mut HeapRead<'h, T>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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
                let removed = this.remove(idx);
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
fn list_clear<'h, T: ListStorage>(list: &mut HeapRead<'h, T>, vm: &mut VM<'h>) {
    mem::take(list.get_mut(vm.heap)).discard(vm);
}

/// Implements Python's `list.copy()` method.
///
/// Returns a shallow copy of the list, preflighting the slot bytes like
/// `clone_all_items` so a huge copy fails with a graceful `MemoryError`.
fn list_copy<T: ListStorage>(list: &T, heap: &Heap) -> RunResult<Value> {
    heap.tracker
        .check_allocation(list.len().saturating_mul(list.element_size()))?;
    let copied = list.copy(heap);
    Ok(Value::Ref(heap.allocate(HeapData::SpecializedList(copied))))
}

/// Implements Python's `list.extend(iterable)` method.
///
/// Extends the list by appending all items from the iterable.
fn list_extend<'h, T: ListStorage>(list: &mut HeapRead<'h, T>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let iterable = args.get_one_arg("list.extend", vm.heap)?;
    let items: SmallVec<[_; 2]> = collect_owned_iterable(iterable, vm)?;
    list.extend_items(vm, items.into_vec())?;

    Ok(Value::None)
}

/// Implements Python's `list.index(value[, start[, end]])` method.
///
/// Returns the index of the first occurrence of value.
/// Raises ValueError if the value is not found.
fn list_index<'h, T: ListStorage>(list: &HeapRead<'h, T>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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

    // Search for the value in the specified range
    let iter = list.iter(vm)?;
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
fn list_count<'h, T: ListStorage>(list: &HeapRead<'h, T>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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

/// Performs an in-place sort on a list with optional key function and reverse flag.
///
/// To safely support user-supplied `key` callbacks (and rich-comparison `__lt__`
/// methods) that may reentrantly mutate the same list, we follow CPython's
/// strategy: the list's `items` vector is **detached** for the duration of the
/// sort so the list looks empty to any reentrant code. All sort work is then
/// performed on the detached buffer, which is always swapped back into the
/// list afterwards. If the user mutated the live (empty) list during the
/// sort, we additionally raise `ValueError: list modified during sort`,
/// matching CPython exactly.
fn do_list_sort<'h, T: ListStorage>(
    list: &mut HeapRead<'h, T>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> Result<(), RunError> {
    // Detach the list's items so reentrant access via the list's heap id sees
    // an empty list. The detached buffer is always swapped back into the list
    // when we're done. Done *before* parsing args so the reentrancy guard is
    // in place if parsing somehow allocates or drops user values that run
    // arbitrary code.
    let mut args_guard = DropGuard::new(args, vm);
    let (_, vm) = args_guard.as_parts_mut();
    let storage = list.get(vm.heap);
    if matches!(
        storage.specialized(),
        Some(SpecializedList::Int(_) | SpecializedList::Float(_))
    ) {
        vm.heap
            .tracker
            .check_allocation(storage.len().saturating_mul(VALUE_SIZE))?;
    }
    let (args, vm) = args_guard.into_parts();
    let items = mem::take(list.get_mut(vm.heap)).into_values();
    defer_drop_mut!(items, vm);

    let sort_result = parse_and_sort("sort() key argument", items, args, vm);

    // Restore the detached values, then release anything callbacks put on the live list.
    let restored = T::from_values(mem::take(items));
    let modified = mem::replace(list.get_mut(vm.heap), restored);
    let was_modified = !modified.is_empty();
    modified.discard(vm);

    // Surface any sort error first; otherwise the modification error (if any).
    sort_result?;
    if was_modified {
        Err(SimpleException::new_msg(ExcType::ValueError, "list modified during sort").into())
    } else {
        Ok(())
    }
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
/// * `get_item` - Returns the i-th value via brief immutable heap access, or
///   `None` once `i` is out of range. The `for<'r>` bound means only
///   heap-derived borrows fit — containers whose repr snapshots up front
///   (deque, set, dict) own their clones already and use [`repr_items_fmt`].
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
    // for an index that turned out to be out of range. `repr_check_time` bounds
    // reprs that keep growing the sequence from inside a user `__repr__`.
    for i in 0.. {
        let Some(item) = get_item(vm.heap, i) else { break };
        defer_drop!(item, vm);
        if i > 0 {
            if repr_check_time(i, vm) {
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
            if repr_check_time(i, vm) {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        item.py_repr_fmt(f, vm, heap_ids)?;
    }
    Ok(())
}

/// Polls the time/memory soft limits every 64th item rather than every item,
/// keeping per-item overhead to one branch while still truncating huge reprs
/// promptly with `", ...[timeout]"` (tested in `resource_limits.rs`).
pub(crate) fn repr_check_time(i: usize, vm: &VM<'_>) -> bool {
    vm.heap.tracker.check_memory_time_every(i).is_err()
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
        let len = match heap.get(self.list) {
            HeapData::List(list) => list.len(),
            HeapData::SpecializedList(list) => list.len(),
            _ => unreachable!("list iterator must reference a list"),
        };
        len.saturating_sub(self.index)
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

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::ListIterator
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(self.clone_value(vm.heap))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let (list_id, index) = {
            let iterator = self.get(vm.heap);
            (iterator.list, iterator.index)
        };
        let item = match vm.heap.get(list_id) {
            HeapData::List(list) => list.get(index).map(|item| item.clone_with_heap(vm.heap)),
            HeapData::SpecializedList(list) => list.get(index).map(|item| item.clone_with_heap(vm.heap)),
            _ => unreachable!("list iterator must reference a list"),
        };
        if item.is_some() {
            self.get_mut(vm.heap).index += 1;
        }
        Ok(item)
    }
}

impl<'h, T: ListStorage> PyDeepCopy<'h> for HeapRead<'h, T> {
    /// Copies into a shell memoized before its items are, so a list holding
    /// itself resolves to the shell rather than recursing forever.
    #[inline(never)]
    fn py_deep_copy(&self, source: &Value, memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value> {
        let copy_id = vm
            .heap
            .allocate(HeapData::SpecializedList(SpecializedList::new(Vec::new())));
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
            let HeapReadOutput::SpecializedList(mut dest) = vm.heap.read(copy_id) else {
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
    use std::time::Duration;

    use monty_types::{PrintWriter, ResourceLimits, ResourceTracker};
    use num_bigint::BigInt;

    use super::*;
    use crate::{
        heap::{Heap, HeapReader},
        run::{Program, SessionTables},
        types::{LongInt, Slice},
    };

    #[test]
    fn promotion_preserves_heap_identity_handles_and_iterators() {
        let mut heap = Heap::new(16, ResourceTracker::default());
        let id = heap.allocate(HeapData::SpecializedList(SpecializedList::Int(vec![1, 2])));
        let legacy_id = heap.allocate(HeapData::List(List::new(vec![Value::Int(1), Value::Int(2)])));
        let value = Value::Ref(id);
        let legacy_value = Value::Ref(legacy_id);
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        let iterator = HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let HeapReadOutput::SpecializedList(mut list) = vm.heap.read(id) else {
                unreachable!()
            };
            let HeapReadOutput::SpecializedList(alias) = vm.heap.read(id) else {
                unreachable!()
            };
            let HeapReadOutput::List(legacy) = vm.heap.read(legacy_id) else {
                unreachable!()
            };

            assert!(value.py_eq(&legacy_value, &mut vm).unwrap());
            assert!(legacy_value.py_eq(&value, &mut vm).unwrap());
            assert_eq!(
                HeapRead::py_cmp(&list, &legacy, &mut vm).unwrap(),
                CmpOrder::Ordered(Ordering::Equal)
            );
            let concatenated = list.py_add_impl(&legacy_value, &mut vm).unwrap().unwrap();
            assert_eq!(concatenated.py_len(&vm), Some(4));
            concatenated.drop_with(&mut vm);

            let mut iterator = list.py_iter(&mut vm).unwrap();
            assert!(matches!(iterator.py_next(&mut vm).unwrap(), Some(Value::Int(1))));
            list.append(&mut vm, Value::None).unwrap();

            assert_eq!(list.id(), alias.id());
            assert!(matches!(alias.get(vm.heap), SpecializedList::Generic(_)));
            assert!(matches!(alias.clone_item(2, &mut vm), Value::None));
            assert!(matches!(iterator.py_next(&mut vm).unwrap(), Some(Value::Int(2))));
            assert!(matches!(iterator.py_next(&mut vm).unwrap(), Some(Value::None)));
            assert!(iterator.py_next(&mut vm).unwrap().is_none());
            iterator
        });
        iterator.drop_with(&mut heap);
        value.drop_with(&mut heap);
        legacy_value.drop_with(&mut heap);
    }

    #[test]
    fn numeric_sum_checks_the_deadline_inside_the_buffer_walk() {
        let tracker = ResourceTracker::new(ResourceLimits::default().max_turn_duration(Duration::ZERO));
        let mut heap = Heap::new(16, tracker);
        let id = heap.allocate(HeapData::SpecializedList(SpecializedList::default()));
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let HeapReadOutput::SpecializedList(mut list) = vm.heap.read(id) else {
                unreachable!()
            };
            list.get_mut(vm.heap).extend_owned((0..1024).map(Value::Int).collect());
            vm.heap.tracker.on_execution_start();
            assert!(list.sum(&Value::Int(0), &vm).is_err());
            *list.get_mut(vm.heap) = SpecializedList::Float(vec![1.0; 1024]);
            assert!(list.sum(&Value::Int(0), &vm).is_err());
        });
        Value::Ref(id).drop_with(&mut heap);
    }

    /// Creates a heap with a list and a LongInt index, bypassing into_value() demotion.
    ///
    /// This allows testing the defensive code path where a LongInt contains an i64-fitting value.
    fn create_heap_with_list_and_longint(list_items: Vec<Value>, index_value: BigInt) -> (Heap, HeapId, HeapId) {
        let heap = Heap::new(16, ResourceTracker::default());
        let list = SpecializedList::new(list_items);
        let list_id = heap.allocate(HeapData::SpecializedList(list));
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
            let HeapReadOutput::SpecializedList(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_ok());

        // Verify the list was updated by checking it matches expected Int value
        let HeapData::SpecializedList(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(*list.get(1).unwrap(), Value::Int(99)));

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
            let HeapReadOutput::SpecializedList(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_ok());

        // Verify the last element was updated
        let HeapData::SpecializedList(list) = heap.get(list_id) else {
            panic!("expected list");
        };
        assert!(matches!(*list.get(2).unwrap(), Value::Int(99)));

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
            let HeapReadOutput::SpecializedList(mut list) = vm.heap.read(list_id) else {
                panic!("expected list");
            };
            list.py_setitem(key, new_value, &mut vm)
        });

        assert!(result.is_err());

        Value::Ref(list_id).drop_with(&mut heap);
    }
    #[test]
    fn generic_numeric_slices_and_repeats_keep_the_preflighted_representation() {
        for items in [
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::Float(1.0), Value::Float(2.0)],
        ] {
            let mut heap = Heap::new(16, ResourceTracker::default());
            let id = heap.allocate(HeapData::SpecializedList(SpecializedList::Generic(List {
                items,
                contains_refs: false,
            })));
            let mut tables = SessionTables::default();
            let program = Program::for_tests();
            HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
                let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
                let HeapReadOutput::SpecializedList(list) = vm.heap.read(id) else {
                    unreachable!()
                };
                let slice = list.getitem_slice(&Slice::new(None, None, None), &mut vm).unwrap();
                let repeated = list.py_mul_impl(&Value::Int(2), &mut vm).unwrap().unwrap();
                for result in [slice, repeated] {
                    let Value::Ref(result_id) = &result else { unreachable!() };
                    assert!(matches!(
                        vm.heap.get(*result_id),
                        HeapData::SpecializedList(SpecializedList::Generic(_))
                    ));
                    result.drop_with(&mut vm);
                }
            });
            Value::Ref(id).drop_with(&mut heap);
        }
    }

    #[test]
    fn numeric_storage_has_eight_byte_slots_and_promotes_without_coercion() {
        let mut ints = SpecializedList::new((0..128).map(Value::Int).collect());
        assert_eq!(ints.element_size(), 8);
        assert_eq!(ints.capacity(), 128);
        ints.push(Value::Bool(true));
        assert!(matches!(ints, SpecializedList::Generic(_)));
        assert!(matches!(*ints.get(0).unwrap(), Value::Int(0)));
        assert!(matches!(*ints.get(128).unwrap(), Value::Bool(true)));
        ints.remove(128);
        assert!(matches!(ints, SpecializedList::Generic(_)));
        ints.drop_with(&mut Heap::new(16, ResourceTracker::default()));
    }

    #[test]
    fn snapshots_preserve_list_variants() {
        for list in [
            SpecializedList::Empty,
            SpecializedList::Int(vec![i64::MIN, 0, i64::MAX]),
            SpecializedList::Int(Vec::new()),
            SpecializedList::Float(vec![-0.0, f64::INFINITY, f64::NAN]),
            SpecializedList::Float(Vec::new()),
            SpecializedList::Generic(List {
                items: vec![Value::None, Value::Bool(true)],
                contains_refs: false,
            }),
        ] {
            let encoded = minicbor_serde::to_vec(&list).unwrap();
            let restored: SpecializedList = minicbor_serde::from_slice(&encoded).unwrap();
            assert_eq!(mem::discriminant(&list), mem::discriminant(&restored));
            assert_eq!(encoded, minicbor_serde::to_vec(&restored).unwrap());
            let mut heap = Heap::new(16, ResourceTracker::default());
            list.drop_with(&mut heap);
            restored.drop_with(&mut heap);
        }
    }
}
