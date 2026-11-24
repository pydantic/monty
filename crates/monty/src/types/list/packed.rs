//! Compact list buffers for integers, floats, booleans, and `None`.
//!
//! [`Packing`] determines whether incoming values fit the current layout or require a wider
//! or generic buffer. [`PackedList`] owns the slots and implements operations on them.
//! Nullable buffers mark `None` in a separate bitmap. A list containing only `None` can
//! reserve that bitmap before it needs any value slots.

use std::{mem, ops::Range};

use monty_types::ResourceTracker;

use super::{SequenceItems, bitmap::Bitmap};
use crate::{
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::range::Range as PythonRange,
    value::Value,
};

const COPY_BATCH: usize = 1024;

/// Nonnegative integers through 255 fit in one byte. Other `i64` values need wide slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum IntWidth {
    Byte,
    Wide,
}

impl IntWidth {
    pub(super) fn of(value: i64) -> Self {
        if u8::try_from(value).is_ok() {
            Self::Byte
        } else {
            Self::Wide
        }
    }

    pub(super) fn size(self) -> usize {
        match self {
            Self::Byte => size_of::<u8>(),
            Self::Wide => size_of::<i64>(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Packing {
    Empty,
    Nones,
    Int { width: IntWidth, nullable: bool },
    Float { nullable: bool },
    Bool { nullable: bool },
    Generic,
}

impl Packing {
    pub(super) fn of(value: &Value) -> Self {
        match value {
            Value::None => Self::Nones,
            Value::Int(value) => Self::Int {
                width: IntWidth::of(*value),
                nullable: false,
            },
            Value::Bool(_) => Self::Bool { nullable: false },
            Value::Float(_) => Self::Float { nullable: false },
            _ => Self::Generic,
        }
    }

    pub(super) fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Empty, b) | (b, Self::Empty) => b,
            (Self::Nones, Self::Nones) => Self::Nones,
            (Self::Nones, Self::Int { width, .. }) | (Self::Int { width, .. }, Self::Nones) => {
                Self::Int { width, nullable: true }
            }
            (Self::Nones, Self::Bool { .. }) | (Self::Bool { .. }, Self::Nones) => Self::Bool { nullable: true },
            (Self::Int { width: aw, nullable: a }, Self::Int { width: bw, nullable: b }) => Self::Int {
                width: aw.max(bw),
                nullable: a || b,
            },
            (Self::Bool { nullable: a }, Self::Bool { nullable: b }) => Self::Bool { nullable: a || b },
            (Self::Nones, Self::Float { .. }) | (Self::Float { .. }, Self::Nones) => Self::Float { nullable: true },
            (Self::Float { nullable: a }, Self::Float { nullable: b }) => Self::Float { nullable: a || b },
            _ => Self::Generic,
        }
    }

    pub(super) fn for_values(items: &[Value], tracker: &ResourceTracker) -> RunResult<Self> {
        let mut packing = Self::Empty;
        for (index, item) in items.iter().enumerate() {
            tracker.check_time_every(index)?;
            packing = packing.merge(Self::of(item));
            if packing == Self::Generic {
                break;
            }
        }
        Ok(packing)
    }
}

#[derive(Debug)]
pub(super) enum PackedList {
    Empty,
    Nones(Box<Nones>),
    ByteInts(Vec<u8>),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
    NullableBytes(Box<NullableSlots<u8>>),
    NullableInts(Box<NullableSlots<i64>>),
    NullableFloats(Box<NullableSlots<f64>>),
    Bools(Box<Bools>),
}

impl PackedList {
    pub(super) fn new(capacity: usize, packing: Packing, tracker: &ResourceTracker) -> RunResult<Self> {
        let mut items = Self::Empty;
        if capacity != 0 {
            tracker.check_pending_allocation(items.growth_bytes(capacity, packing))?;
            items.reserve(capacity, packing)?;
        }
        Ok(items)
    }

    pub(super) fn from_bytes(bytes: &[u8], tracker: &ResourceTracker) -> RunResult<Self> {
        if bytes.is_empty() {
            return Ok(Self::Empty);
        }
        tracker.check_pending_allocation(bytes.len())?;
        let mut values = Vec::new();
        reserve_vec(&mut values, bytes.len())?;
        for chunk in bytes.chunks(COPY_BATCH) {
            tracker.check_time()?;
            values.extend_from_slice(chunk);
        }
        Ok(Self::ByteInts(values))
    }

    pub(super) fn from_range(range: &PythonRange, tracker: &ResourceTracker) -> RunResult<Self> {
        let length = range.len_i128();
        if length == 0 {
            return Ok(Self::Empty);
        }
        let capacity = usize::try_from(length).map_err(|_| ExcType::allocation_too_large())?;
        let last = i128::from(range.start) + (length - 1) * i128::from(range.step);
        let last = i64::try_from(last).expect("range endpoint fits in an i64");
        let width = IntWidth::of(range.start).max(IntWidth::of(last));

        let mut result = Self::new(capacity, Packing::Int { width, nullable: false }, tracker)?;
        match &mut result {
            Self::ByteInts(values) => fill_range(values, range, capacity, tracker)?,
            Self::Ints(values) => fill_range(values, range, capacity, tracker)?,
            _ => unreachable!("range requires integer storage"),
        }
        Ok(result)
    }

    pub(super) fn packing(&self) -> Packing {
        match self {
            Self::Empty => Packing::Empty,
            Self::Nones(_) => Packing::Nones,
            Self::Floats(_) => Packing::Float { nullable: false },
            Self::NullableFloats(_) => Packing::Float { nullable: true },
            Self::ByteInts(_) => Packing::Int {
                width: IntWidth::Byte,
                nullable: false,
            },
            Self::Ints(_) => Packing::Int {
                width: IntWidth::Wide,
                nullable: false,
            },
            Self::NullableBytes(_) => Packing::Int {
                width: IntWidth::Byte,
                nullable: true,
            },
            Self::NullableInts(_) => Packing::Int {
                width: IntWidth::Wide,
                nullable: true,
            },
            Self::Bools(items) => Packing::Bool {
                nullable: items.nulls.is_some(),
            },
        }
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Nones(items) => items.len,
            Self::ByteInts(items) => items.len(),
            Self::Ints(items) => items.len(),
            Self::Floats(items) => items.len(),
            Self::NullableBytes(items) => items.values.len(),
            Self::NullableInts(items) => items.values.len(),
            Self::NullableFloats(items) => items.values.len(),
            Self::Bools(items) => items.len,
        }
    }

    pub(super) fn capacity(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Nones(items) => items.slots.capacity(),
            Self::ByteInts(items) => items.capacity(),
            Self::Ints(items) => items.capacity(),
            Self::Floats(items) => items.capacity(),
            Self::NullableBytes(items) => items.values.capacity(),
            Self::NullableInts(items) => items.values.capacity(),
            Self::NullableFloats(items) => items.values.capacity(),
            Self::Bools(items) => items.values.capacity(),
        }
    }

    pub(super) fn growth_bytes(&self, capacity: usize, packing: Packing) -> usize {
        match packing {
            Packing::Empty => 0,
            Packing::Nones => match self {
                Self::Nones(items) => items.slots.growth_bytes(capacity),
                _ => Bitmap::allocation_bytes(capacity).saturating_add(size_of::<Nones>()),
            },
            Packing::Int {
                width: IntWidth::Byte,
                nullable,
            } => self.slot_growth::<u8>(capacity, nullable),
            Packing::Int {
                width: IntWidth::Wide,
                nullable,
            } => self.slot_growth::<i64>(capacity, nullable),
            Packing::Float { nullable } => self.slot_growth::<f64>(capacity, nullable),
            Packing::Bool { nullable } => match self {
                Self::Bools(items) => items.values.growth_bytes(capacity).saturating_add(match &items.nulls {
                    Some(nulls) => nulls.growth_bytes(capacity),
                    None if nullable => Bitmap::allocation_bytes(capacity),
                    None => 0,
                }),
                Self::Nones(items) => Bitmap::allocation_bytes(capacity)
                    .saturating_add(items.slots.growth_bytes(capacity))
                    .saturating_add(size_of::<Bools>()),
                _ => Bitmap::allocation_bytes(capacity)
                    .saturating_mul(1 + usize::from(nullable))
                    .saturating_add(size_of::<Bools>()),
            },
            Packing::Generic => unreachable!("generic storage has its own buffer"),
        }
    }

    fn slot_growth<T>(&self, capacity: usize, nullable: bool) -> usize {
        let (current, nulls) = match self {
            Self::ByteInts(items) if size_of::<T>() == 1 => (items.capacity(), None),
            Self::NullableBytes(items) if size_of::<T>() == 1 => (items.values.capacity(), Some(&items.nulls)),
            Self::Ints(items) => (items.capacity(), None),
            Self::Floats(items) => (items.capacity(), None),
            Self::NullableInts(items) => (items.values.capacity(), Some(&items.nulls)),
            Self::NullableFloats(items) => (items.values.capacity(), Some(&items.nulls)),
            _ => (0, None),
        };
        let values = capacity.saturating_sub(current).saturating_mul(size_of::<T>());
        let bitmap = if let Some(nulls) = nulls {
            nulls.growth_bytes(capacity)
        } else if nullable {
            let allocation = match self {
                Self::Nones(items) => items.slots.growth_bytes(capacity),
                _ => Bitmap::allocation_bytes(capacity),
            };
            allocation.saturating_add(size_of::<NullableSlots<T>>())
        } else {
            0
        };
        values.saturating_add(bitmap)
    }

    pub(super) fn needs_wider_ints(&self, packing: Packing) -> bool {
        matches!(self, Self::ByteInts(_) | Self::NullableBytes(_))
            && matches!(
                packing,
                Packing::Int {
                    width: IntWidth::Wide,
                    ..
                }
            )
    }

    pub(super) fn widen(&self, capacity: usize, packing: Packing, tracker: &ResourceTracker) -> RunResult<Self> {
        let mut result = Self::new(capacity, packing, tracker)?;
        result.copy_range(self, 0..self.len(), tracker)?;
        Ok(result)
    }

    /// The caller preflights payload and bitmap allocations together.
    pub(super) fn reserve(&mut self, capacity: usize, packing: Packing) -> RunResult<()> {
        match self {
            Self::Empty => {
                *self = match packing {
                    Packing::Nones => {
                        let mut items = Nones::default();
                        items.slots.reserve(capacity)?;
                        Self::Nones(Box::new(items))
                    }
                    Packing::Int {
                        width: IntWidth::Byte,
                        nullable: false,
                    } => Self::ByteInts(new_slots(capacity, 0)?),
                    Packing::Int {
                        width: IntWidth::Wide,
                        nullable: false,
                    } => Self::Ints(new_slots(capacity, 0)?),
                    Packing::Int {
                        width: IntWidth::Byte,
                        nullable: true,
                    } => Self::NullableBytes(Box::new(NullableSlots::new(capacity)?)),
                    Packing::Int {
                        width: IntWidth::Wide,
                        nullable: true,
                    } => Self::NullableInts(Box::new(NullableSlots::new(capacity)?)),
                    Packing::Float { nullable: false } => Self::Floats(new_slots(capacity, 0)?),
                    Packing::Float { nullable: true } => Self::NullableFloats(Box::new(NullableSlots::new(capacity)?)),
                    Packing::Bool { nullable } => {
                        let mut items = Bools::default();
                        items.reserve(capacity, nullable)?;
                        Self::Bools(Box::new(items))
                    }
                    _ => unreachable!("checked packing for a nonempty list"),
                };
            }
            Self::Nones(items) => {
                items.slots.reserve(capacity)?;
                match packing {
                    Packing::Nones => {}
                    Packing::Int {
                        width: IntWidth::Byte,
                        nullable: true,
                    } => *self = Self::NullableBytes(Box::new(items.with_slots(capacity)?)),
                    Packing::Int {
                        width: IntWidth::Wide,
                        nullable: true,
                    } => *self = Self::NullableInts(Box::new(items.with_slots(capacity)?)),
                    Packing::Float { nullable: true } => {
                        *self = Self::NullableFloats(Box::new(items.with_slots(capacity)?));
                    }
                    Packing::Bool { nullable: true } => {
                        let mut values = Bitmap::default();
                        values.reserve(capacity)?;
                        items.slots.fill(0, items.len, true);
                        let nulls = Some(mem::take(&mut items.slots));
                        *self = Self::Bools(Box::new(Bools {
                            len: items.len,
                            values,
                            nulls,
                        }));
                    }
                    _ => unreachable!("lists containing None require nullable storage"),
                }
            }
            Self::ByteInts(values) => {
                if matches!(packing, Packing::Int { nullable: true, .. }) {
                    *self = Self::NullableBytes(Box::new(NullableSlots::from_slots(values, capacity)?));
                } else {
                    reserve_vec(values, capacity)?;
                }
            }
            Self::Ints(values) => {
                if matches!(packing, Packing::Int { nullable: true, .. }) {
                    *self = Self::NullableInts(Box::new(NullableSlots::from_slots(values, capacity)?));
                } else {
                    reserve_vec(values, capacity)?;
                }
            }
            Self::Floats(values) => {
                if matches!(packing, Packing::Float { nullable: true }) {
                    *self = Self::NullableFloats(Box::new(NullableSlots::from_slots(values, capacity)?));
                } else {
                    reserve_vec(values, capacity)?;
                }
            }
            Self::NullableBytes(items) => items.reserve(capacity)?,
            Self::NullableInts(items) => items.reserve(capacity)?,
            Self::NullableFloats(items) => items.reserve(capacity)?,
            Self::Bools(items) => items.reserve(capacity, packing == (Packing::Bool { nullable: true }))?,
        }
        Ok(())
    }

    pub(super) fn view(&self) -> PackedView<'_> {
        PackedView(self)
    }

    #[inline]
    pub(super) fn get(&self, index: usize) -> Option<Value> {
        match self {
            Self::Empty => None,
            Self::Nones(items) => (index < items.len).then_some(Value::None),
            Self::ByteInts(items) => items.get(index).map(|&item| Value::Int(i64::from(item))),
            Self::Ints(items) => items.get(index).copied().map(Value::Int),
            Self::Floats(items) => items.get(index).copied().map(Value::Float),
            Self::NullableBytes(items) => items.get(index),
            Self::NullableInts(items) => items.get(index),
            Self::NullableFloats(items) => items.get(index),
            Self::Bools(items) => (index < items.len).then(|| items.get(index)),
        }
    }

    pub(super) fn push(&mut self, value: &Value) {
        self.insert(self.len(), value);
    }

    pub(super) fn insert(&mut self, index: usize, value: &Value) {
        match self {
            Self::Empty => unreachable!("reserved storage before insert"),
            Self::Nones(items) => items.len += 1,
            Self::ByteInts(items) => items.insert(index, u8::from_value(value)),
            Self::Ints(items) => items.insert(index, i64::from_value(value)),
            Self::Floats(items) => items.insert(index, f64::from_value(value)),
            Self::NullableBytes(items) => items.insert(index, value),
            Self::NullableInts(items) => items.insert(index, value),
            Self::NullableFloats(items) => items.insert(index, value),
            Self::Bools(items) => {
                items.move_within(index, index + 1, items.len - index);
                items.set(index, value);
                items.len += 1;
            }
        }
    }

    pub(super) fn slice(&self, start: i64, step: i64, count: usize, tracker: &ResourceTracker) -> RunResult<Self> {
        if count == 0 {
            return Ok(Self::Empty);
        }
        match self {
            Self::ByteInts(values) => return Ok(Self::ByteInts(slice_slots(values, start, step, count, tracker)?)),
            Self::Ints(values) => return Ok(Self::Ints(slice_slots(values, start, step, count, tracker)?)),
            Self::Floats(values) => return Ok(Self::Floats(slice_slots(values, start, step, count, tracker)?)),
            _ => {}
        }

        let mut result = Self::new(count, self.packing(), tracker)?;
        if let Self::Nones(items) = &mut result {
            items.len = count;
        } else if step == 1 {
            let start = usize::try_from(start).expect("slice index is within the list");
            result.copy_range(self, start..start + count, tracker)?;
        } else {
            for (offset, index) in slice_indices(start, step, count).enumerate() {
                tracker.check_time_every(offset)?;
                result.push(&self.get(index).expect("checked list index"));
            }
        }
        Ok(result)
    }

    pub(super) fn replace_slice(
        &mut self,
        start: i64,
        step: i64,
        count: usize,
        values: SequenceItems<'_>,
        tracker: &ResourceTracker,
    ) -> RunResult<()> {
        let len = values.len();
        if let Self::Nones(items) = self {
            items.len = items.len - count + len;
            return Ok(());
        }
        if len != count {
            // Only unit-step slices can change length. Move the suffix backwards when growing
            // and forwards when shrinking so overlapping slots are read before they are overwritten.
            let start = usize::try_from(start).expect("unit-step slice start is non-negative");
            let old_len = self.len();
            let tail = old_len - (start + count);
            for index in count..len {
                tracker.check_time_every(index)?;
                values.map_item(index, |value| self.push(value));
            }
            for offset in 0..tail {
                tracker.check_time_every(offset)?;
                let i = if len > count { tail - 1 - offset } else { offset };
                self.swap(start + count + i, start + len + i);
            }
            if len < count {
                self.delete_range(old_len - (count - len)..old_len);
            }
        }
        if step == 1
            && let SequenceItems::Packed(source) = values
        {
            let start = usize::try_from(start).expect("unit-step slice start is non-negative");
            match (&mut *self, source.0) {
                (Self::ByteInts(dst), Self::ByteInts(src)) => return copy_slots(dst, start, src, tracker),
                (Self::Ints(dst), Self::Ints(src)) => return copy_slots(dst, start, src, tracker),
                (Self::Floats(dst), Self::Floats(src)) => return copy_slots(dst, start, src, tracker),
                _ => {}
            }
        }
        for (offset, index) in slice_indices(start, step, len).enumerate() {
            tracker.check_time_every(offset)?;
            values.map_item(offset, |value| self.set(index, value));
        }
        Ok(())
    }

    pub(super) fn delete_range(&mut self, range: Range<usize>) {
        match self {
            Self::Empty => debug_assert!(range.is_empty()),
            Self::Nones(items) => items.len -= range.len(),
            Self::ByteInts(items) => drop(items.drain(range)),
            Self::Ints(items) => drop(items.drain(range)),
            Self::Floats(items) => drop(items.drain(range)),
            Self::NullableBytes(items) => items.delete_range(range),
            Self::NullableInts(items) => items.delete_range(range),
            Self::NullableFloats(items) => items.delete_range(range),
            Self::Bools(items) => {
                items.move_within(range.end, range.start, items.len - range.end);
                items.len -= range.len();
            }
        }
    }

    pub(super) fn set(&mut self, index: usize, value: &Value) {
        assert!(self.try_set(index, value), "incompatible packed list assignment");
    }

    /// Returns `false` without mutating the list if `value` requires a storage change.
    #[inline]
    pub(super) fn try_set(&mut self, index: usize, value: &Value) -> bool {
        match (self, value) {
            (Self::Nones(_), Value::None) => {}
            (Self::ByteInts(items), Value::Int(value)) if let Ok(byte) = u8::try_from(*value) => {
                items[index] = byte;
            }
            (Self::Ints(items), Value::Int(value)) => items[index] = *value,
            (Self::Floats(items), Value::Float(value)) => items[index] = *value,
            (Self::NullableBytes(items), Value::Int(value)) if let Ok(byte) = u8::try_from(*value) => {
                items.set_slot(index, byte, false);
            }
            (Self::NullableInts(items), Value::Int(value)) => items.set_slot(index, *value, false),
            (Self::NullableFloats(items), Value::Float(value)) => items.set_slot(index, *value, false),
            (Self::NullableBytes(items), Value::None) => items.set_slot(index, 0, true),
            (Self::NullableInts(items), Value::None) => items.set_slot(index, 0, true),
            (Self::NullableFloats(items), Value::None) => items.set_slot(index, 0.0, true),
            (Self::Bools(items), value) => return items.try_set(index, value),
            _ => return false,
        }
        true
    }

    pub(super) fn remove(&mut self, index: usize) -> Value {
        match self {
            Self::ByteInts(items) => return Value::Int(i64::from(items.remove(index))),
            Self::Ints(items) => return Value::Int(items.remove(index)),
            Self::Floats(items) => return Value::Float(items.remove(index)),
            _ => {}
        }
        let value = self.get(index).expect("checked list index");
        self.delete_range(index..index + 1);
        value
    }

    pub(super) fn swap(&mut self, a: usize, b: usize) {
        match self {
            Self::Empty | Self::Nones(_) => {}
            Self::ByteInts(items) => items.swap(a, b),
            Self::Ints(items) => items.swap(a, b),
            Self::Floats(items) => items.swap(a, b),
            Self::NullableBytes(items) => items.swap(a, b),
            Self::NullableInts(items) => items.swap(a, b),
            Self::NullableFloats(items) => items.swap(a, b),
            Self::Bools(items) => {
                items.values.swap(a, b);
                if let Some(nulls) = &mut items.nulls {
                    nulls.swap(a, b);
                }
            }
        }
    }

    pub(super) fn reverse(&mut self) {
        match self {
            Self::Empty | Self::Nones(_) => {}
            Self::ByteInts(items) => items.reverse(),
            Self::Ints(items) => items.reverse(),
            Self::Floats(items) => items.reverse(),
            Self::NullableBytes(items) => items.reverse(),
            Self::NullableInts(items) => items.reverse(),
            Self::NullableFloats(items) => items.reverse(),
            Self::Bools(items) => {
                items.values.reverse(items.len);
                if let Some(nulls) = &mut items.nulls {
                    nulls.reverse(items.len);
                }
            }
        }
    }

    pub(super) fn extend_from_within(&mut self, range: Range<usize>) {
        match self {
            Self::Empty => unreachable!("reserved storage before extending"),
            Self::Nones(items) => items.len += range.len(),
            Self::ByteInts(items) => items.extend_from_within(range),
            Self::Ints(items) => items.extend_from_within(range),
            Self::Floats(items) => items.extend_from_within(range),
            Self::NullableBytes(items) => items.extend_from_within(range),
            Self::NullableInts(items) => items.extend_from_within(range),
            Self::NullableFloats(items) => items.extend_from_within(range),
            Self::Bools(items) => {
                items.move_within(range.start, items.len, range.len());
                items.len += range.len();
            }
        }
    }

    /// Requires space for `total` elements to be reserved first.
    pub(super) fn repeat_prefix(&mut self, total: usize, tracker: &ResourceTracker) -> RunResult<()> {
        let prefix = self.len();
        let mut filled = prefix;
        while filled < total {
            tracker.check_time()?;
            filled = self.repeat_chunk(prefix, total);
        }
        Ok(())
    }

    /// Appends one chunk from the original prefix. The caller reserves `total` slots.
    pub(super) fn repeat_chunk(&mut self, prefix: usize, total: usize) -> usize {
        if let Self::Nones(items) = self {
            items.len = total;
            return total;
        }
        let len = self.len();
        let start = len % prefix;
        let count = (total - len).min(len - start).min(COPY_BATCH);
        self.extend_from_within(start..start + count);
        len + count
    }

    pub(super) fn copy(&self, tracker: &ResourceTracker) -> RunResult<Self> {
        self.slice(0, 1, self.len(), tracker)
    }

    pub(super) fn copy_range(
        &mut self,
        source: &Self,
        range: Range<usize>,
        tracker: &ResourceTracker,
    ) -> RunResult<()> {
        if let (Self::Nones(dst), Self::Nones(_)) = (&mut *self, source) {
            dst.len += range.len();
            return Ok(());
        }
        let mut start = range.start;
        while start < range.end {
            tracker.check_time()?;
            let end = start.saturating_add(COPY_BATCH).min(range.end);
            self.append_range(source, start..end);
            start = end;
        }
        Ok(())
    }

    pub(super) fn append_range(&mut self, source: &Self, range: Range<usize>) {
        match (&mut *self, source) {
            (Self::Nones(dst), Self::Nones(_)) => dst.len += range.len(),
            (Self::ByteInts(dst), Self::ByteInts(src)) => dst.extend_from_slice(&src[range]),
            (Self::Ints(dst), Self::Ints(src)) => dst.extend_from_slice(&src[range]),
            (Self::Floats(dst), Self::Floats(src)) => dst.extend_from_slice(&src[range]),
            (Self::Ints(dst), Self::ByteInts(src)) => dst.extend(src[range].iter().copied().map(i64::from)),
            (Self::NullableBytes(dst), Self::Nones(_)) => dst.append_nones(range.len()),
            (Self::NullableInts(dst), Self::Nones(_)) => dst.append_nones(range.len()),
            (Self::NullableFloats(dst), Self::Nones(_)) => dst.append_nones(range.len()),
            (Self::Bools(dst), Self::Nones(_)) => {
                dst.nulls
                    .as_mut()
                    .expect("reserved nullable storage")
                    .fill(dst.len, range.len(), true);
                dst.len += range.len();
            }
            (Self::NullableBytes(dst), Self::ByteInts(src)) => dst.append_slots(&src[range]),
            (Self::NullableInts(dst), Self::Ints(src)) => dst.append_slots(&src[range]),
            (Self::NullableFloats(dst), Self::Floats(src)) => dst.append_slots(&src[range]),
            (Self::NullableInts(dst), Self::ByteInts(src)) => {
                dst.nulls.fill(dst.values.len(), range.len(), false);
                dst.values.extend(src[range].iter().copied().map(i64::from));
            }
            (Self::NullableBytes(dst), Self::NullableBytes(src)) => dst.append_nullable(src, range),
            (Self::NullableInts(dst), Self::NullableInts(src)) => dst.append_nullable(src, range),
            (Self::NullableFloats(dst), Self::NullableFloats(src)) => dst.append_nullable(src, range),
            (Self::NullableInts(dst), Self::NullableBytes(src)) => {
                dst.nulls
                    .copy_from(dst.values.len(), &src.nulls, range.start, range.len());
                dst.values.extend(src.values[range].iter().copied().map(i64::from));
            }
            (Self::Bools(dst), Self::Bools(src)) => {
                dst.values.copy_from(dst.len, &src.values, range.start, range.len());
                if let Some(nulls) = &mut dst.nulls {
                    if let Some(src_nulls) = &src.nulls {
                        nulls.copy_from(dst.len, src_nulls, range.start, range.len());
                    } else {
                        nulls.fill(dst.len, range.len(), false);
                    }
                }
                dst.len += range.len();
            }
            _ => {
                for index in range {
                    self.push(&source.get(index).expect("checked list index"));
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PackedView<'a>(&'a PackedList);

impl PackedView<'_> {
    pub(crate) fn len(self) -> usize {
        self.0.len()
    }

    pub(crate) fn get(self, index: usize) -> Option<Value> {
        self.0.get(index)
    }

    pub(crate) fn iter(self) -> impl DoubleEndedIterator<Item = Value> + ExactSizeIterator {
        (0..self.len()).map(move |index| self.get(index).expect("checked list index"))
    }

    pub(crate) fn extend_reversed(self, output: &mut Vec<Value>) {
        match self.0 {
            PackedList::ByteInts(items) => output.extend(items.iter().rev().map(|&item| Value::Int(i64::from(item)))),
            PackedList::Ints(items) => output.extend(items.iter().rev().copied().map(Value::Int)),
            PackedList::Floats(items) => output.extend(items.iter().rev().copied().map(Value::Float)),
            PackedList::Bools(items) => output.extend((0..items.len).rev().map(|i| items.get(i))),
            _ => output.extend(self.iter().rev()),
        }
    }
}

/// Reserves the null bitmap before the list gains its first non-null value.
#[derive(Debug, Default)]
pub(super) struct Nones {
    len: usize,
    slots: Bitmap,
}

impl Nones {
    fn with_slots<T: NumericSlot>(&mut self, capacity: usize) -> RunResult<NullableSlots<T>> {
        let values = new_slots(capacity, self.len)?;
        self.slots.fill(0, self.len, true);
        Ok(NullableSlots {
            values,
            nulls: mem::take(&mut self.slots),
        })
    }
}

#[derive(Debug)]
pub(super) struct NullableSlots<T> {
    values: Vec<T>,
    nulls: Bitmap,
}

impl<T: NumericSlot> NullableSlots<T> {
    fn new(capacity: usize) -> RunResult<Self> {
        Self::from_slots(&mut Vec::new(), capacity)
    }

    fn from_slots(values: &mut Vec<T>, capacity: usize) -> RunResult<Self> {
        let mut nulls = Bitmap::default();
        nulls.reserve(capacity)?;
        reserve_vec(values, capacity)?;
        Ok(Self {
            values: mem::take(values),
            nulls,
        })
    }

    fn reserve(&mut self, capacity: usize) -> RunResult<()> {
        self.nulls.reserve(capacity)?;
        reserve_vec(&mut self.values, capacity)
    }

    fn get(&self, index: usize) -> Option<Value> {
        let value = *self.values.get(index)?;
        Some(if self.nulls.get(index) {
            Value::None
        } else {
            value.into_value()
        })
    }

    fn insert(&mut self, index: usize, value: &Value) {
        self.nulls.move_within(index, index + 1, self.values.len() - index);
        self.values.insert(index, T::from_nullable(value));
        self.nulls.set(index, matches!(value, Value::None));
    }

    fn delete_range(&mut self, range: Range<usize>) {
        self.nulls
            .move_within(range.end, range.start, self.values.len() - range.end);
        drop(self.values.drain(range));
    }

    fn set_slot(&mut self, index: usize, value: T, is_null: bool) {
        self.values[index] = value;
        self.nulls.set(index, is_null);
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.values.swap(a, b);
        self.nulls.swap(a, b);
    }

    fn reverse(&mut self) {
        self.values.reverse();
        self.nulls.reverse(self.values.len());
    }

    fn extend_from_within(&mut self, range: Range<usize>) {
        self.nulls.move_within(range.start, self.values.len(), range.len());
        self.values.extend_from_within(range);
    }

    fn append_nones(&mut self, len: usize) {
        self.nulls.fill(self.values.len(), len, true);
        self.values.resize(self.values.len() + len, T::default());
    }

    fn append_slots(&mut self, values: &[T]) {
        self.nulls.fill(self.values.len(), values.len(), false);
        self.values.extend_from_slice(values);
    }

    fn append_nullable(&mut self, source: &Self, range: Range<usize>) {
        self.nulls
            .copy_from(self.values.len(), &source.nulls, range.start, range.len());
        self.values.extend_from_slice(&source.values[range]);
    }
}

#[derive(Debug, Default)]
pub(super) struct Bools {
    len: usize,
    values: Bitmap,
    nulls: Option<Bitmap>,
}

impl Bools {
    fn reserve(&mut self, capacity: usize, nullable: bool) -> RunResult<()> {
        if let Some(nulls) = &mut self.nulls {
            nulls.reserve(capacity)?;
        } else if nullable {
            let mut nulls = Bitmap::default();
            nulls.reserve(capacity)?;
            self.values.reserve(capacity)?;
            self.nulls = Some(nulls);
            return Ok(());
        }
        self.values.reserve(capacity)
    }

    fn get(&self, index: usize) -> Value {
        if self.nulls.as_ref().is_some_and(|nulls| nulls.get(index)) {
            Value::None
        } else {
            Value::Bool(self.values.get(index))
        }
    }

    fn set(&mut self, index: usize, value: &Value) {
        assert!(self.try_set(index, value), "incompatible boolean list assignment");
    }

    fn try_set(&mut self, index: usize, value: &Value) -> bool {
        match *value {
            Value::Bool(boolean) => {
                self.values.set(index, boolean);
                if let Some(nulls) = &mut self.nulls {
                    nulls.set(index, false);
                }
            }
            Value::None if let Some(nulls) = &mut self.nulls => {
                self.values.set(index, false);
                nulls.set(index, true);
            }
            _ => return false,
        }
        true
    }

    fn move_within(&mut self, src: usize, dst: usize, count: usize) {
        self.values.move_within(src, dst, count);
        if let Some(nulls) = &mut self.nulls {
            nulls.move_within(src, dst, count);
        }
    }
}

pub(super) trait NumericSlot: Copy + Default {
    fn from_value(value: &Value) -> Self;
    fn into_value(self) -> Value;

    fn from_nullable(value: &Value) -> Self {
        if matches!(value, Value::None) {
            Self::default()
        } else {
            Self::from_value(value)
        }
    }
}

impl NumericSlot for i64 {
    fn from_value(value: &Value) -> Self {
        let Value::Int(value) = value else {
            unreachable!("expected an integer")
        };
        *value
    }

    fn into_value(self) -> Value {
        Value::Int(self)
    }
}

impl NumericSlot for u8 {
    fn from_value(value: &Value) -> Self {
        Self::try_from(i64::from_value(value)).expect("byte-sized list integer")
    }

    fn into_value(self) -> Value {
        Value::Int(i64::from(self))
    }
}

impl NumericSlot for f64 {
    fn from_value(value: &Value) -> Self {
        let Value::Float(value) = value else {
            unreachable!("expected a float")
        };
        *value
    }

    fn into_value(self) -> Value {
        Value::Float(self)
    }
}

fn fill_range<T: TryFrom<i64>>(
    values: &mut Vec<T>,
    range: &PythonRange,
    len: usize,
    tracker: &ResourceTracker,
) -> RunResult<()> {
    let mut next = i128::from(range.start);
    let step = i128::from(range.step);
    for index in 0..len {
        tracker.check_time_every(index)?;
        let value = i64::try_from(next).expect("range element fits in an i64");
        values.push(T::try_from(value).unwrap_or_else(|_| unreachable!("range element fits the selected slot")));
        next += step;
    }
    Ok(())
}

fn new_slots<T: NumericSlot>(capacity: usize, len: usize) -> RunResult<Vec<T>> {
    let mut values = Vec::new();
    reserve_vec(&mut values, capacity)?;
    values.resize(len, T::default());
    Ok(values)
}

pub(super) fn slice_indices(start: i64, step: i64, count: usize) -> impl Iterator<Item = usize> {
    let mut index = i128::from(start);
    (0..count).map(move |_| {
        let current = index;
        index += i128::from(step);
        usize::try_from(current).expect("slice index is within the list")
    })
}

fn slice_slots<T: NumericSlot>(
    source: &[T],
    start: i64,
    step: i64,
    count: usize,
    tracker: &ResourceTracker,
) -> RunResult<Vec<T>> {
    tracker.check_pending_allocation(count.saturating_mul(size_of::<T>()))?;
    let mut target = new_slots(count, 0)?;
    let index = usize::try_from(start).expect("slice starts within the list");
    match step {
        1 => {
            for chunk in source[index..index + count].chunks(COPY_BATCH) {
                tracker.check_time()?;
                target.extend_from_slice(chunk);
            }
        }
        -1 => {
            for chunk in source[index + 1 - count..=index].rchunks(COPY_BATCH) {
                tracker.check_time()?;
                target.extend(chunk.iter().rev().copied());
            }
        }
        _ => {
            for (offset, index) in slice_indices(start, step, count).enumerate() {
                tracker.check_time_every(offset)?;
                target.push(source[index]);
            }
        }
    }
    Ok(target)
}

fn copy_slots<T: Copy>(target: &mut [T], start: usize, source: &[T], tracker: &ResourceTracker) -> RunResult<()> {
    let target = &mut target[start..start + source.len()];
    if source.len() < COPY_BATCH {
        target.copy_from_slice(source);
    } else {
        for (dst, src) in target.chunks_mut(COPY_BATCH).zip(source.chunks(COPY_BATCH)) {
            dst.copy_from_slice(src);
            tracker.check_time()?;
        }
    }
    Ok(())
}

fn reserve_vec<T>(items: &mut Vec<T>, capacity: usize) -> RunResult<()> {
    items
        .try_reserve_exact(capacity.saturating_sub(items.len()))
        .map_err(|_| ExcType::allocation_too_large())
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use monty_types::ResourceLimits;

    use super::*;
    use crate::exception_private::RunError;

    #[test]
    fn packed_slice_replacement_polls_during_the_copy() {
        let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::ZERO));
        tracker.on_execution_start();
        thread::sleep(Duration::from_millis(1));
        tracker.on_execution_stop();

        let len = COPY_BATCH * 3;
        let source = PackedList::ByteInts(vec![1; len]);
        for (start, step) in [(0, 1), (i64::try_from(len).unwrap() - 1, -1)] {
            let mut target = PackedList::ByteInts(vec![0; len]);
            let result = target.replace_slice(start, step, len, SequenceItems::Packed(source.view()), &tracker);
            assert!(
                matches!(result, Err(RunError::UncatchableExc(exc)) if exc.exc.exc_type() == ExcType::TimeoutError)
            );
            let PackedList::ByteInts(slots) = target else {
                unreachable!()
            };
            assert!(slots.contains(&0));
            assert!(slots.contains(&1));
        }
    }

    #[test]
    fn packed_range_fill_checks_the_deadline() {
        let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::ZERO));
        tracker.on_execution_start();
        thread::sleep(Duration::from_millis(1));
        tracker.on_execution_stop();

        for start in [0, -256] {
            let range = PythonRange::new(start, start + 256, 1);
            let result = PackedList::from_range(&range, &tracker);
            assert!(
                matches!(result, Err(RunError::UncatchableExc(exc)) if exc.exc.exc_type() == ExcType::TimeoutError)
            );
        }
    }
}
