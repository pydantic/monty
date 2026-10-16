//! A global cache of user-class method lookups, keyed by class version and name.
//!
//! `obj.method(...)` on an instance otherwise hashes the method name into the
//! class namespace on every call. This is the same scheme as CPython's type
//! attribute cache: a small direct-mapped table whose keys are a
//! [`Class::version`](crate::types::Class::version) tag and the name's
//! [`StringId`].

use crate::{
    heap::{Heap, HeapData, HeapId},
    intern::{FunctionId, StringId},
    value::Value,
};

/// Number of slots; a power of two so the index is a shift.
const SLOTS: usize = 256;

/// Maps `(class version, name)` to the method that name resolves to.
///
/// Holds no references: a version tag names one class with one exact namespace,
/// and is never reused, so a hit implies the class is alive (the receiver holds
/// it) and its namespace still owns the method. Only methods are cached, never
/// data attributes or misses. Lives on the VM and is never dumped.
#[derive(Debug, Default)]
pub(crate) struct MethodCache {
    /// Empty until the first insert, so runs without method calls pay nothing.
    slots: Vec<Option<Slot>>,
}

impl MethodCache {
    /// Returns the cached method for `name` on the class tagged `version`.
    #[inline]
    pub fn get(&self, version: u64, name: StringId, heap: &Heap) -> Option<Value> {
        self.slots
            .get(slot_index(version, name))
            .copied()
            .flatten()
            .filter(|slot| slot.version == version && slot.name == name)
            .map(|slot| slot.method.to_value(heap))
    }

    /// Records that `name` on the class tagged `version` resolves to `member`.
    ///
    /// Does nothing unless `member` is a method, i.e. a value that binds `self`.
    pub fn insert(&mut self, version: u64, name: StringId, member: &Value, heap: &Heap) {
        if let Some(method) = CachedMethod::from_value(member, heap) {
            if self.slots.is_empty() {
                self.slots = vec![None; SLOTS];
            }
            self.slots[slot_index(version, name)] = Some(Slot { version, name, method });
        }
    }
}

/// Mixes the key into a slot index with a Fibonacci hash.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the shift leaves at most log2(SLOTS) bits"
)]
fn slot_index(version: u64, name: StringId) -> usize {
    let key = version ^ ((name.index() as u64) << 32);
    let shift = u64::BITS - SLOTS.trailing_zeros();
    (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> shift) as usize
}

/// One cache entry.
#[derive(Debug, Clone, Copy)]
struct Slot {
    version: u64,
    name: StringId,
    method: CachedMethod,
}

/// A cached method value, stored without a reference count.
#[derive(Debug, Clone, Copy)]
enum CachedMethod {
    /// A plain `def` with no closure or defaults.
    Def(FunctionId),
    /// A closure, a function with defaults, or a `functools.partial`.
    Heap(HeapId),
}

impl CachedMethod {
    /// Classifies a method value; `None` for anything that does not bind `self`.
    ///
    /// Must agree with `is_method_value` in `types/instance.rs`.
    fn from_value(value: &Value, heap: &Heap) -> Option<Self> {
        match value {
            Value::DefFunction(id) => Some(Self::Def(*id)),
            Value::Ref(id)
                if matches!(
                    heap.get(*id),
                    HeapData::Closure(_) | HeapData::FunctionDefaults(_) | HeapData::Partial(_)
                ) =>
            {
                Some(Self::Heap(*id))
            }
            _ => None,
        }
    }

    /// Returns an owned value for the cached method, taking a new reference.
    fn to_value(self, heap: &Heap) -> Value {
        match self {
            Self::Def(id) => Value::DefFunction(id),
            Self::Heap(id) => {
                heap.inc_ref(id);
                Value::Ref(id)
            }
        }
    }
}
