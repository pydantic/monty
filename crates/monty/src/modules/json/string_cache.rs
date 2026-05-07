//! Per-run string cache for `json.loads()`.
//!
//! Repeated JSON parsing within a single execution often encounters the same
//! strings — especially dict keys like `"id"`, `"name"`, `"type"` — across
//! multiple `json.loads()` calls. This cache deduplicates heap allocations for
//! those strings by hashing incoming bytes and returning a `clone_with_heap` of
//! a previously allocated `Value` on cache hits.
//!
//! The design is modelled on jiter's `PyStringCache`: a fixed-capacity
//! fully-associative cache with linear probing and no eviction (when all probe
//! slots are occupied, the string is allocated without caching). The cache is
//! lazily initialized so programs that never call `json.loads()` pay zero cost.
//!
//! The cache lives on the [`VM`] and is scoped to a single execution run. It
//! is cleaned up when the VM is dropped and its entries are registered as GC
//! roots in [`VM::run_gc()`].

use std::iter;

use ahash::RandomState;

use crate::{
    heap::{ContainsHeap, HeapData, HeapId, HeapReader},
    resource::{ResourceError, ResourceTracker},
    types::str::Str,
    value::Value,
};

/// Number of slots in the cache.
///
/// Must be a power of two so the compiler can convert modulo to a bitwise AND.
const CAPACITY: usize = 16_384;

/// Minimum string length eligible for caching.
///
/// Empty strings and single-ASCII-character strings are already interned by
/// `allocate_string`, so caching them would be redundant.
const MIN_LEN: usize = 2;

/// Maximum string length eligible for caching.
///
/// Very long strings rarely repeat and would waste cache space.
const MAX_LEN: usize = 64;

/// Entry in the string cache: `(hash, raw string, allocated Value)`.
type CacheEntry = Option<(u64, Box<str>, Value)>;

/// Lazily-initialized string cache for `json.loads()`.
///
/// Wraps an `Option<CacheInner>` so the backing array is only allocated on the
/// first eligible string, and programs that never parse JSON pay nothing.
///
/// # Lifecycle
///
/// - Created as empty (`None`) when the VM starts.
/// - Backing storage allocated on the first `get_or_allocate` call with an
///   eligible string (2–64 bytes).
/// - Persists across multiple `json.loads()` calls within the same run.
/// - Cleaned up when the VM is dropped via [`drop_all`](Self::drop_all).
/// - Cached values are reported as GC roots via [`gc_roots`](Self::gc_roots).
#[derive(Default)]
pub(crate) struct JsonStringCache {
    inner: Option<CacheInner>,
}

/// Backing storage: a fixed-size array of cache entries plus a hash builder.
struct CacheInner {
    entries: Box<[CacheEntry; CAPACITY]>,
    hash_builder: RandomState,
}

impl JsonStringCache {
    /// Looks up `s` in the cache. On hit, returns a cloned `Value` (with its
    /// refcount incremented). On miss, allocates the string on the heap, stores
    /// a clone in the cache, and returns the original.
    ///
    /// Strings shorter than [`MIN_LEN`] or longer than [`MAX_LEN`] bypass the
    /// cache entirely and are allocated directly.
    ///
    /// The backing array is allocated lazily on the first eligible string.
    pub fn get_or_allocate(
        &mut self,
        s: String,
        heap: &HeapReader<'_, impl ResourceTracker>,
    ) -> Result<Value, ResourceError> {
        let len = s.len();
        if !(MIN_LEN..=MAX_LEN).contains(&len) {
            let heap_id = heap.heap().allocate(HeapData::Str(Str::new(s)))?;
            return Ok(Value::Ref(heap_id));
        }

        let inner = self.inner.get_or_insert_with(CacheInner::new);
        inner.get_or_allocate(s, heap)
    }

    /// Drops all cached values, decrementing their refcounts.
    ///
    /// Called during `VM::drop()` before the heap is torn down.
    pub fn drop_all(&mut self, heap: &mut impl ContainsHeap) {
        if let Some(inner) = &mut self.inner {
            for entry in inner.entries.iter_mut() {
                if let Some((_, _, value)) = entry.take() {
                    value.drop_with_heap(heap);
                }
            }
        }
    }

    /// Yields the `HeapId` of every cached value so the GC treats them as roots.
    ///
    /// Without this, a GC cycle could free a heap string that the cache still
    /// references, leading to a use-after-free on the next cache hit.
    pub fn gc_roots(&self) -> impl Iterator<Item = HeapId> + '_ {
        self.inner
            .iter()
            .flat_map(|inner| inner.entries.iter())
            .filter_map(|entry| entry.as_ref().and_then(|(_, _, value)| value.ref_id()))
    }
}

impl CacheInner {
    /// Creates a new cache with zeroed entries.
    ///
    /// Uses `iter::repeat_with().collect()` to avoid allocating the large array
    /// on the stack (see jiter PR #239).
    fn new() -> Self {
        Self {
            entries: iter::repeat_with(|| None)
                .take(CAPACITY)
                .collect::<Vec<_>>()
                .into_boxed_slice()
                .try_into()
                .expect("Vec length equals CAPACITY"),
            hash_builder: RandomState::default(),
        }
    }

    /// Looks up `s` in the cache. On hit, returns a cloned `Value`. On miss,
    /// allocates on the heap and inserts into the cache.
    fn get_or_allocate(
        &mut self,
        s: String,
        heap: &HeapReader<'_, impl ResourceTracker>,
    ) -> Result<Value, ResourceError> {
        let hash = self.hash_builder.hash_one(s.as_str());
        // Truncation is intentional — we only need the low bits for indexing.
        #[expect(clippy::cast_possible_truncation)]
        let primary = hash as usize & (CAPACITY - 1);

        // Linear probe up to 5 contiguous slots, wrapping around the table end.
        for offset in 0..5 {
            let index = (primary + offset) & (CAPACITY - 1);
            let entry = &mut self.entries[index];
            match entry {
                Some((entry_hash, cached_str, cached_value)) => {
                    if *entry_hash == hash && **cached_str == *s {
                        return Ok(cached_value.clone_with_heap(heap));
                    }
                }
                None => {
                    // Empty slot — allocate and insert.
                    return self.insert_at(index, hash, s, heap);
                }
            }
        }
        // All 5 probe slots occupied — allocate without caching.
        let heap_id = heap.heap().allocate(HeapData::Str(Str::new(s)))?;
        Ok(Value::Ref(heap_id))
    }

    /// Allocates `s` on the heap, stores a clone in `entries[index]`, and
    /// returns the original `Value`.
    fn insert_at(
        &mut self,
        index: usize,
        hash: u64,
        s: String,
        heap: &HeapReader<'_, impl ResourceTracker>,
    ) -> Result<Value, ResourceError> {
        let key = s.clone().into_boxed_str();
        let heap_id = heap.heap().allocate(HeapData::Str(Str::new(s)))?;
        let value = Value::Ref(heap_id);
        let cached = value.clone_with_heap(heap);
        self.entries[index] = Some((hash, key, cached));
        Ok(value)
    }
}
