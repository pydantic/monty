//! Structural identities and their injective Python integer encoding.
//!
//! Immediate values need allocation-free identity comparison, while `id()`
//! must distinguish every identity category. Compact identities remain inline
//! Python integers; only encodings outside `i64` require a heap `LongInt`.

use monty_types::{ResourceError, ResourceTracker};
use num_bigint::{BigInt, Sign};
use serde::Serialize;
use smallvec::SmallVec;

use crate::{
    builtins::Builtins,
    bytecode::VM,
    heap::{Heap, HeapData},
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
/// injective and used to expose the same key through `id()`.
#[derive(PartialEq, Eq)]
pub(crate) enum Identity<'a> {
    /// Internal uninitialized-value sentinel.
    Undefined,
    /// Python's `Ellipsis` singleton.
    Ellipsis,
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
    /// Name-based identity retained for host-supplied callables.
    ExtFunction(&'a str),
    /// Identity of an interpreter marker.
    Marker(Marker),
    /// Identity of an interpreter property descriptor.
    Property(Property),
    /// Identity of an arena-allocated object.
    Heap(usize),
}

impl<'a> Identity<'a> {
    /// Builds the structural identity used by both `is` and `id()`.
    pub(crate) fn new(value: &Value, vm: &'a VM<'_, impl ResourceTracker>) -> Self {
        match value {
            Value::Undefined => Self::Undefined,
            Value::Ellipsis => Self::Ellipsis,
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
            Value::ExtFunction(name) => Self::ExtFunction(vm.interns.get_str(*name)),
            Value::Marker(marker) => Self::Marker(*marker),
            Value::Property(property) => Self::Property(*property),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::ExtFunction(name) => Self::ExtFunction(name),
                _ => Self::Heap(id.index()),
            },
            #[cfg(feature = "memory-model-checks")]
            Value::Dereferenced => panic!("Cannot get identity of Dereferenced object"),
        }
    }

    /// Encodes this key as a nonnegative Python integer.
    ///
    /// Results fitting `i64` remain immediate. Wider fixed identities and long
    /// external-function names allocate a heap `LongInt` for the returned value.
    pub(crate) fn into_value(self, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
        if let Some(encoded) = self.fixed_encoding() {
            encoded_u128_to_value(encoded, heap)
        } else if let Self::ExtFunction(name) = self {
            encode_long_name(name, heap)
        } else {
            unreachable!("only long external-function names lack a fixed encoding")
        }
    }

    /// Encodes identities that fit in a `u128`, avoiding temporary allocation.
    fn fixed_encoding(&self) -> Option<u128> {
        let payload = match self {
            Self::Undefined | Self::Ellipsis | Self::None => 0,
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
            Self::ExtFunction(name) if name.len() <= MAX_FIXED_BYTES => bytes_payload(name.as_bytes()),
            Self::ExtFunction(_) => return None,
            Self::Marker(value) => fixed_serde_payload(value),
            Self::Property(value) => fixed_serde_payload(value),
        };
        Some((payload << TAG_BITS) | u128::from(self.tag()))
    }

    /// Returns the stable low-bit category tag for this identity variant.
    fn tag(&self) -> u8 {
        match self {
            Self::Undefined => 0,
            Self::Ellipsis => 1,
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
            Self::ExtFunction(_) => 13,
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

/// Returns an immediate integer when possible, otherwise allocating a `LongInt`.
fn encoded_u128_to_value(encoded: u128, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    if let Ok(encoded) = i64::try_from(encoded) {
        Ok(Value::Int(encoded))
    } else {
        allocate_identity_int(BigInt::from(encoded), heap)
    }
}

/// Encodes an arbitrary external-function name into its necessarily wide identity.
fn encode_long_name(name: &str, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    let mut bytes = SmallVec::<[u8; 32]>::with_capacity(name.len() + 1);
    bytes.push(1);
    bytes.extend_from_slice(name.as_bytes());
    let payload = BigInt::from_bytes_be(Sign::Plus, &bytes);
    let encoded = (payload << TAG_BITS) + BigInt::from(Identity::ExtFunction(name).tag());
    allocate_identity_int(encoded, heap)
}

/// Allocates a wide identity integer without repeating the known-failing `i64` check.
fn allocate_identity_int(encoded: BigInt, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    let id = heap.allocate(HeapData::LongInt(LongInt::new(encoded)))?;
    Ok(Value::Ref(id))
}
