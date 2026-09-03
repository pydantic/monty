//! Structural identities and their injective Python integer encoding.
//!
//! Immediate values need allocation-free identity comparison, while `id()`
//! must distinguish every identity category. Compact identities remain inline
//! Python integers; only encodings outside `i64` require a heap `LongInt`.

use serde::Serialize;

use crate::{
    builtins::Builtins,
    heap::{Heap, HeapId},
    modules::ModuleFunctions,
    types::{LongInt, Property},
    value::{Marker, Value},
};

/// Number of low bits reserved for the identity category.
const TAG_BITS: u8 = 5;
/// Largest byte payload that can be shifted into a `u128` after adding its sentinel.
const MAX_FIXED_BYTES: usize = 14;

/// Complete identity key for a runtime value.
///
/// Equality is the implementation of Python's `is`; the integer encoding is
/// injective and used to expose the same key through `id()`. `Hash` lets
/// identity sets answer "seen this object?" in O(1) (see the dict/set probes).
#[derive(PartialEq, Eq, Hash)]
pub(crate) enum Identity {
    /// Internal uninitialized-value sentinel.
    Undefined,
    /// Python's `Ellipsis` singleton.
    Ellipsis,
    /// Python's `NotImplemented` singleton.
    NotImplemented,
    /// Python's `None` singleton.
    None,
    /// Boolean singleton identity.
    Bool(bool),
    /// Value-based identity for an immediate integer.
    Int(i64),
    /// Bitwise identity for an immediate float.
    Float(u64),
    /// Identity of an interned string.
    InternString(usize),
    /// Identity of an interned bytes value.
    InternBytes(usize),
    /// Identity of an interned long integer literal.
    InternLongInt(usize),
    /// Identity of an interpreter builtin.
    Builtin(Builtins),
    /// Identity of a standard-library function.
    ModuleFunction(ModuleFunctions),
    /// Identity of a sandbox-defined function.
    DefFunction(usize),
    /// Identity of an interpreter marker.
    Marker(Marker),
    /// Identity of an interpreter property descriptor.
    Property(Property),
    /// Identity of an arena-allocated object.
    Heap(usize),
}

impl Identity {
    /// Builds the structural identity used by both `is` and `id()`.
    pub(crate) fn new(value: &Value) -> Self {
        match value {
            Value::Undefined => Self::Undefined,
            Value::Ellipsis => Self::Ellipsis,
            Value::NotImplemented => Self::NotImplemented,
            Value::None => Self::None,
            Value::Bool(value) => Self::Bool(*value),
            Value::Int(value) => Self::Int(*value),
            Value::Float(value) => Self::Float(value.to_bits()),
            Value::InternString(id) => Self::InternString(id.index()),
            Value::InternBytes(id) => Self::InternBytes(id.index()),
            Value::InternLongInt(id) => Self::InternLongInt(id.index()),
            Value::Builtin(builtin) => Self::Builtin(*builtin),
            Value::ModuleFunction(function) => Self::ModuleFunction(*function),
            Value::DefFunction(id) => Self::DefFunction(id.index()),
            Value::Marker(marker) => Self::Marker(*marker),
            Value::Property(property) => Self::Property(*property),
            Value::Ref(id) => Self::Heap(id.index()),
            #[cfg(feature = "memory-model-checks")]
            Value::Dereferenced => panic!("Cannot get identity of Dereferenced object"),
        }
    }

    /// Builds the identity of an arena-allocated object.
    pub(crate) fn from_heap_id(id: HeapId) -> Self {
        Self::Heap(id.index())
    }

    /// Encodes this key as the nonnegative integer exposed by Python's `id()`.
    pub(crate) fn encoded(&self) -> u128 {
        let payload = match self {
            Self::Undefined | Self::Ellipsis | Self::NotImplemented | Self::None => 0,
            Self::Bool(value) => u128::from(*value),
            Self::Int(value) => u128::from(zigzag_i64(*value)),
            Self::Float(bits) => u128::from(compact_float_bits(*bits)),
            Self::InternString(index)
            | Self::InternBytes(index)
            | Self::InternLongInt(index)
            | Self::DefFunction(index)
            | Self::Heap(index) => u128::try_from(*index).expect("usize fits in u128"),
            Self::Builtin(value) => fixed_serde_payload(value),
            Self::ModuleFunction(value) => fixed_serde_payload(value),
            Self::Marker(value) => fixed_serde_payload(value),
            Self::Property(value) => fixed_serde_payload(value),
        };
        (payload << TAG_BITS) | u128::from(self.tag())
    }

    /// Encodes this key as a Python integer.
    ///
    /// Returns an immediate integer when possible, otherwise allocating a `LongInt`.
    pub(crate) fn into_value(self, heap: &Heap) -> Value {
        LongInt::value_from_u128(self.encoded(), heap)
    }

    /// Returns the stable low-bit category tag for this identity variant.
    fn tag(&self) -> u8 {
        match self {
            Self::Undefined => 0,
            Self::Ellipsis => 1,
            Self::NotImplemented => 2,
            Self::None => 3,
            Self::Bool(_) => 4,
            Self::Int(_) => 5,
            Self::Float(_) => 6,
            Self::InternString(_) => 7,
            Self::InternBytes(_) => 8,
            Self::InternLongInt(_) => 9,
            Self::Builtin(_) => 10,
            Self::ModuleFunction(_) => 11,
            Self::DefFunction(_) => 12,
            Self::Marker(_) => 14,
            Self::Property(_) => 15,
            Self::Heap(_) => 16,
        }
    }
}

/// Returns a prefix-preserving integer for a short byte sequence.
fn bytes_payload(bytes: &[u8]) -> u128 {
    bytes
        .iter()
        .fold(1, |payload, byte| (payload << u8::BITS) | u128::from(*byte))
}

/// Serializes a small enum payload into a stack buffer and preserves its length.
fn fixed_serde_payload(value: &impl Serialize) -> u128 {
    let mut buffer = [0; MAX_FIXED_BYTES];
    let serialized = postcard::to_slice(value, &mut buffer).expect("identity enum payload fits in 14 bytes");
    bytes_payload(serialized)
}

/// Maps signed integers into `u64` while keeping small magnitudes compact.
fn zigzag_i64(value: i64) -> u64 {
    if value >= 0 {
        value.unsigned_abs() << 1
    } else {
        ((value.unsigned_abs() - 1) << 1) | 1
    }
}

/// Reorders float fields so common powers of two have compact identities.
fn compact_float_bits(bits: u64) -> u64 {
    const MANTISSA_BITS: u8 = 52;
    const EXPONENT_MASK: u64 = (1 << 11) - 1;
    const MANTISSA_MASK: u64 = (1 << MANTISSA_BITS) - 1;

    let sign = bits >> 63;
    let exponent = (bits >> MANTISSA_BITS) & EXPONENT_MASK;
    let mantissa = bits & MANTISSA_MASK;
    (mantissa << 12) | (sign << 11) | exponent
}
