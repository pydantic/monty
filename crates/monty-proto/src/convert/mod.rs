//! Conversions between [`pb`](crate::pb) wire types and monty's public types.
//!
//! Direction conventions:
//!
//! - **Rust → proto is total** (`From<&T>`): every monty value has a wire
//!   representation, and borrowing avoids cloning large containers twice.
//! - **proto → Rust is fallible** (`TryFrom<T>` with [`ProtoConvertError`]):
//!   wire data comes from the other side of a process boundary and must be
//!   treated as untrusted — unknown names, out-of-range numbers, and missing
//!   oneof arms are errors, never panics.
//!
//! Nesting depth is bounded on the receiving side by prost's decode recursion
//! limit (100 message levels), so the recursive conversions here cannot be
//! driven arbitrarily deep by a malicious peer. Encoding has no such implicit
//! limit — senders must check [`exceeds_max_value_depth`] before shipping a
//! value, or the receiver will reject the frame as a protocol failure.

mod exception;
mod limits;
mod os_call;
mod resume;

use std::{error, fmt};

use monty::{DictPairs, MontyObject};
pub use resume::future_results_from_proto;

/// Why a wire value could not be converted into its monty equivalent.
///
/// Returned by all `TryFrom<pb::...>` impls in this crate. The variants are
/// deliberately specific so a parent can log exactly which field a misbehaving
/// child produced.
#[derive(Debug)]
pub enum ProtoConvertError {
    /// A required message field or oneof was absent.
    MissingField(&'static str),
    /// An exception type name that monty does not know.
    UnknownExcType(String),
    /// A type name that monty's `MontyType::from_type_name` does not know.
    UnknownType(String),
    /// A builtin function name that monty does not know.
    UnknownBuiltinFunction(String),
    /// A file handle mode string that is not a supported `open()` mode.
    InvalidFileMode(String),
    /// A field value was out of range or otherwise malformed.
    InvalidValue {
        /// The offending field, e.g. `"Date.month"`.
        field: &'static str,
        /// Human-readable explanation.
        reason: String,
    },
}

impl fmt::Display for ProtoConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field {field}"),
            Self::UnknownExcType(name) => write!(f, "unknown exception type {name:?}"),
            Self::UnknownType(name) => write!(f, "unknown type name {name:?}"),
            Self::UnknownBuiltinFunction(name) => write!(f, "unknown builtin function {name:?}"),
            Self::InvalidFileMode(mode) => write!(f, "invalid file mode {mode:?}"),
            Self::InvalidValue { field, reason } => write!(f, "invalid value for {field}: {reason}"),
        }
    }
}

impl error::Error for ProtoConvertError {}

/// prost's decode recursion limit: the hard ceiling on nested protobuf
/// message levels a receiver will process before rejecting the frame.
const PROST_RECURSION_LIMIT: usize = 100;

/// Message levels consumed by the frame around a value before the value's
/// own `MontyObject` begins. The deepest wrapper chains are three messages:
/// `Request` → `Feed` → `NamedValue` and `Event` → `FunctionCall` → `Pair`.
const FRAME_WRAPPER_DEPTH: usize = 3;

/// Proto message levels a value itself may consume and still decode inside
/// any frame: prost's limit minus the deepest frame wrapper chain.
const MAX_PROTO_VALUE_DEPTH: usize = PROST_RECURSION_LIMIT - FRAME_WRAPPER_DEPTH;

/// Proto message levels per list/tuple/set/frozenset/namedtuple level
/// (`MontyObject` plus its `ObjectList`/`NamedTuple` payload).
const LIST_COST: usize = 2;
/// Proto message levels per dict level (`MontyObject` + `Dict` + `Pair`).
const DICT_COST: usize = 3;
/// Proto message levels per dataclass level (`MontyObject` +
/// `Dataclass` + the attrs `Dict` + `Pair`).
const DATACLASS_COST: usize = 4;

/// Maximum nesting depth of a *list-like* value that can safely cross the
/// wire (the cheapest container shape, and so the deepest possible nesting).
///
/// Containers consume differing proto message levels against prost's decode
/// recursion limit (two per list-like, three per dict, four per dataclass), so
/// dicts only nest to ~32 levels and dataclasses to ~24.
/// [`exceeds_max_value_depth`] applies the exact per-shape accounting; this
/// constant is the headline bound for docs and error messages.
pub const MAX_VALUE_DEPTH: usize = (MAX_PROTO_VALUE_DEPTH - 1) / LIST_COST;

/// Whether `value` nests too deeply to decode inside a wire frame.
///
/// Charges each node's exact proto-level cost (scalars one, list-likes two,
/// dicts three, dataclasses four) against [`MAX_PROTO_VALUE_DEPTH`] and bails
/// out as soon as the budget is exhausted, so its own recursion stays bounded
/// even for adversarially deep values (which the sandbox can build
/// iteratively).
#[must_use]
pub fn exceeds_max_value_depth(value: &MontyObject) -> bool {
    depth_exceeds(value, MAX_PROTO_VALUE_DEPTH)
}

fn depth_exceeds(value: &MontyObject, budget: usize) -> bool {
    match value {
        MontyObject::List(items)
        | MontyObject::Tuple(items)
        | MontyObject::Set(items)
        | MontyObject::FrozenSet(items) => seq_exceeds(items, budget, LIST_COST),
        MontyObject::NamedTuple { values, .. } => seq_exceeds(values, budget, LIST_COST),
        MontyObject::Dict(pairs) => pairs_exceed(pairs, budget, DICT_COST),
        MontyObject::Dataclass { attrs, .. } => pairs_exceed(attrs, budget, DATACLASS_COST),
        // a scalar is one `MontyObject` message level
        _ => budget == 0,
    }
}

fn seq_exceeds(items: &[MontyObject], budget: usize, cost: usize) -> bool {
    match budget.checked_sub(cost) {
        // this container's own wrapper messages don't fit the budget
        None => true,
        Some(remaining) => items.iter().any(|child| depth_exceeds(child, remaining)),
    }
}

fn pairs_exceed(pairs: &DictPairs, budget: usize, cost: usize) -> bool {
    match budget.checked_sub(cost) {
        None => true,
        Some(remaining) => pairs
            .into_iter()
            .any(|(key, value)| depth_exceeds(key, remaining) || depth_exceeds(value, remaining)),
    }
}
