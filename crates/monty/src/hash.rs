//! Single source of truth for hashing Python values.
//!
//! Defines:
//!
//! * [`HashValue`] — a verified Python hash. Construction goes through
//!   [`HashValue::from_raw`]; storing or comparing hashes anywhere in the
//!   runtime should use this type rather than a bare `u64` so the invariant
//!   is enforced by the type system.
//!
//!   Internally backed by a [`NonZero<u64>`] holding the **bit-inverse** of
//!   the raw hash. The inversion is invisible to callers (`from_raw` /
//!   `get` translate at the boundary) but means `Option<HashValue>` is
//!   niche-packed into 8 bytes — `None` is the bit pattern `0`, the inverse
//!   of the reserved `u64::MAX` sentinel.
//! * [`hash_python_str`] / [`hash_python_bytes`] / [`hash_python_long_int`]
//!   — the canonical hash functions for `str`, `bytes` and arbitrary-precision
//!   `int`. Routing every `str`/`bytes`/`int` hash through these helpers
//!   keeps the invariant "interned and heap values with equal content hash
//!   identically" local rather than scattered, since otherwise dict lookups
//!   would silently miss.
//! * [`ASCII_HASHES`] / [`STATIC_HASHES`] — precomputed hashes for the
//!   pre-interned ASCII single-character and [`StaticStrings`] tables,
//!   built via `LazyLock` on first access (one-time cost, dwarfed by parse
//!   time for any non-trivial program).

use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::{Hash, Hasher},
    num::NonZero,
    sync::atomic::{AtomicU64, Ordering},
};

use num_bigint::BigInt;
use num_traits::ToPrimitive;
use strum::EnumCount;

use crate::intern::StaticStrings;

/// A verified Python hash value.
///
/// Internally a [`NonZero<u64>`] holding the bit-inverse of the raw hash, so
/// `Option<HashValue>` niche-packs into 8 bytes. Construct via
/// [`HashValue::from_raw`]; extract via [`HashValue::get`] (both translate
/// the inversion at the boundary).
///
/// The bit-inversion is purely an implementation detail of the niche
/// packing — none of the public traits leak it. [`Hash`], [`fmt::Debug`],
/// [`serde::Serialize`] and [`serde::Deserialize`] all hand-translate to/from
/// the raw `u64` form, so:
///
/// * Composite hashes that fold a `HashValue` see the same bytes `get()`
///   returns (independent of storage layout — a future change to e.g.
///   `NonMaxU64` would leave tuple/dict hashes invariant).
/// * Debug output shows the raw hash (`HashValue(5)`, not the inverted bits).
/// * Snapshots store the raw hash, so the wire format is decoupled from the
///   niche-packing representation.
///
/// Stored in interner hash tables, returned by `Value::py_hash`, and used
/// wherever a known-good hash needs to be passed around.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub(crate) struct HashValue(NonZero<u64>);

impl HashValue {
    /// Wraps a freshly-computed raw hash.
    ///
    /// Bit-inverts and stores as `NonZero<u64>`. The single edge case where
    /// `!hash == 0` (i.e. raw `hash == u64::MAX`) maps to
    /// [`NonZero::<u64>::MIN`] — equivalent to bumping the raw hash by 1.
    /// Mirrors CPython's reservation of `Py_hash_t = -1`. Bias: 1 in 2^64,
    /// no impact on common hashes like `hash(0) == 0`.
    #[inline]
    #[must_use]
    pub const fn new(hash: u64) -> Self {
        Self(match NonZero::new(!hash) {
            Some(nz) => nz,
            None => NonZero::<u64>::MIN,
        })
    }

    /// Returns the underlying `u64` for use in modulo arithmetic, hashbrown
    /// indexing, and other places that need a bare hash.
    #[inline]
    #[must_use]
    pub const fn raw(self) -> u64 {
        !self.0.get()
    }
}

impl Hash for HashValue {
    /// Folds the raw hash (not the stored, inverted form) so that
    /// composite hashes are independent of the niche-packing representation.
    /// Without this, a future change to the internal layout would silently
    /// alter every tuple / namedtuple / dataclass hash.
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw().hash(state);
    }
}

impl fmt::Debug for HashValue {
    /// Prints the raw hash, not the stored bit-inverted form, so debugging
    /// output matches `get()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HashValue").field(&self.raw()).finish()
    }
}

impl serde::Serialize for HashValue {
    /// Emits the raw hash so the wire format is decoupled from the
    /// niche-packing representation.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for HashValue {
    /// Reads a raw `u64` and reconstructs via [`HashValue::from_raw`], so
    /// even a pathological wire value of `u64::MAX` is handled by the same
    /// sentinel-collision path as fresh hashes.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::new(u64::deserialize(deserializer)?))
    }
}

/// Hashes a string using the canonical Python-string hash function.
///
/// Both heap-allocated `Str` values and interned strings must use this so
/// that an interned `"foo"` and a heap `"foo"` hash identically — required
/// for dict-key consistency.
#[inline]
pub(crate) fn hash_python_str(s: &str) -> HashValue {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    HashValue::new(hasher.finish())
}

/// Hashes a bytes value using the canonical Python-bytes hash function.
///
/// Counterpart to [`hash_python_str`] for `bytes`. Used by both heap `Bytes`
/// and the interned bytes table.
#[inline]
pub(crate) fn hash_python_bytes(b: &[u8]) -> HashValue {
    let mut hasher = DefaultHasher::new();
    b.hash(&mut hasher);
    HashValue::new(hasher.finish())
}

/// Hashes a `BigInt` consistently with Python's `int` hash.
///
/// For values that fit in `i64`, returns `i.cast_unsigned()` so that
/// `hash(LongInt(5)) == hash(Value::Int(5))`. For larger values, falls back
/// to hashing `(sign, little-endian bytes)` via [`DefaultHasher`].
///
/// Used by heap `LongInt::py_hash` and by `Interns::long_int_hash` so that
/// interned and heap-allocated long ints with equal value hash identically.
#[inline]
pub(crate) fn hash_python_long_int(bi: &BigInt) -> HashValue {
    if let Some(i) = bi.to_i64() {
        HashValue::new(i.cast_unsigned())
    } else {
        let mut hasher = DefaultHasher::new();
        let (sign, bytes) = bi.to_bytes_le();
        sign.hash(&mut hasher);
        bytes.hash(&mut hasher);
        HashValue::new(hasher.finish())
    }
}

/// A value paired with its precomputed Python [`HashValue`].
///
/// Used in the interner storage so each entry carries its own hash next to
/// the value — push-the-pair instead of parallel arrays. This makes it
/// impossible to forget to keep the value and hash in sync, and makes
/// serde recompute-on-deserialise local to this type.
///
/// Constructors and `Deserialize` impls are provided for the three concrete
/// `T` we use ([`String`], `Vec<u8>`, [`BigInt`]). Adding a fourth would
/// require its own `WithHash<NewT>` constructor and `Deserialize` impl.
///
/// # Wire format
///
/// `Serialize` is a hand-written passthrough — the on-the-wire form is
/// exactly `T`'s serialised form (the hash is recomputable). `Deserialize`
/// reads `T` and rebuilds the hash via the appropriate `hash_python_*`
/// helper. Round-tripping through serde is therefore lossless and any
/// deserialiser-supplied bytes always produce a hash consistent with the
/// canonical helpers.
#[derive(Debug, Clone)]
pub(crate) struct WithHash<T> {
    value: T,
    hash: HashValue,
}

impl<T> WithHash<T> {
    /// Borrow the wrapped value.
    #[inline]
    pub fn value(&self) -> &T {
        &self.value
    }

    /// The precomputed [`HashValue`].
    #[inline]
    pub fn hash(&self) -> HashValue {
        self.hash
    }
}

impl WithHash<String> {
    /// Construct from an owned `String`, hashing via [`hash_python_str`].
    #[inline]
    pub fn for_str(value: String) -> Self {
        let hash = hash_python_str(&value);
        Self { value, hash }
    }
}

impl WithHash<Vec<u8>> {
    /// Construct from an owned `Vec<u8>`, hashing via [`hash_python_bytes`].
    #[inline]
    pub fn for_bytes(value: Vec<u8>) -> Self {
        let hash = hash_python_bytes(&value);
        Self { value, hash }
    }
}

impl WithHash<BigInt> {
    /// Construct from an owned `BigInt`, hashing via [`hash_python_long_int`].
    #[inline]
    pub fn for_long_int(value: BigInt) -> Self {
        let hash = hash_python_long_int(&value);
        Self { value, hash }
    }
}

// `Serialize` is generic: just emit the inner value. The hash is recomputable
// from the value during deserialisation, so we don't waste bytes encoding it
// (and we don't risk locking the snapshot format to the current hash function).
impl<T: serde::Serialize> serde::Serialize for WithHash<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}

// `Deserialize` is hand-written per concrete `T` so the right
// `hash_python_*` helper is invoked.
impl<'de> serde::Deserialize<'de> for WithHash<String> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::for_str(String::deserialize(deserializer)?))
    }
}

impl<'de> serde::Deserialize<'de> for WithHash<Vec<u8>> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::for_bytes(Vec::<u8>::deserialize(deserializer)?))
    }
}

impl<'de> serde::Deserialize<'de> for WithHash<BigInt> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::for_long_int(BigInt::deserialize(deserializer)?))
    }
}

/// Per-slot lazy `static` hash table.
///
/// `[AtomicU64; N]` storing the inner bits of [`HashValue`] (i.e. the
/// bit-inverted raw hash). `0` is the "uncomputed" sentinel — naturally
/// equivalent to [`Option<HashValue>`]'s `None` since `HashValue`'s niche
/// is exactly `0`.
///
/// On the first access of slot `i` the closure is invoked, the result is
/// stored, and subsequent accesses return it directly. Two threads racing
/// to fill the same slot is benign: they compute the same value and one
/// wins the store; the other's store overwrites with the same bits.
///
/// Used for `static` precomputed-hash tables (ASCII / `StaticStrings`).
/// `Cell<Option<HashValue>>` would be the equivalent for non-`static` /
/// per-instance use (Phase 2's per-type heap caches).
pub(crate) struct LazyHashTable<const N: usize> {
    cells: [AtomicU64; N],
}

impl<const N: usize> LazyHashTable<N> {
    /// Construct an empty table — every slot starts uncomputed (`0`).
    pub const fn new() -> Self {
        Self {
            cells: [const { AtomicU64::new(0) }; N],
        }
    }

    /// Returns the cached [`HashValue`] for `index`, or computes and stores it.
    ///
    /// `Ordering::Relaxed` suffices: the only invariant is "any non-zero
    /// value observed is a valid `HashValue`'s inner bits" — true by
    /// construction since we only store `HashValue::0.get()` (which is
    /// guaranteed non-zero by `NonZero<u64>`).
    #[inline]
    pub fn get_or_compute(&self, index: usize, compute: impl FnOnce() -> HashValue) -> HashValue {
        if let Some(stored) = NonZero::new(self.cells[index].load(Ordering::Relaxed)) {
            HashValue(stored)
        } else {
            let h = compute();
            self.cells[index].store(h.0.get(), Ordering::Relaxed);
            h
        }
    }
}

/// Per-slot lazy hashes for the 128 ASCII single-character strings.
///
/// Indexed by the byte value (`0..128`). Each slot is filled on first
/// access via [`hash_python_str`] applied to the matching entry of
/// [`ASCII_STRS`].
pub(crate) static ASCII_HASHES: LazyHashTable<128> = LazyHashTable::new();

/// Per-slot lazy hashes for every [`StaticStrings`] variant.
///
/// Indexed by the variant's discriminant (`StaticStrings as usize`). Each
/// slot is filled on first access from the variant's `&'static str`
/// representation.
pub(crate) static STATIC_HASHES: LazyHashTable<{ StaticStrings::COUNT }> = LazyHashTable::new();
