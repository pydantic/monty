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
//! Nesting depth is implicitly bounded in both directions by prost's decode
//! recursion limit (~100 message levels), so the recursive conversions here
//! cannot be driven arbitrarily deep by a malicious peer.

mod exception;
mod limits;
mod mount;
mod object;
mod resume;

use std::{error, fmt};

use monty::MontyObject;
pub use mount::build_mount_table;
pub use resume::future_results_from_proto;

use crate::pb;

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
    /// A type name that monty's `Type::from_type_name` does not know.
    UnknownType(String),
    /// A builtin function name that monty does not know.
    UnknownBuiltinFunction(String),
    /// A file handle mode string that is not a supported `open()` mode.
    InvalidFileMode(String),
    /// An output-only value kind (`cycle`) was used as an input.
    OutputOnly(&'static str),
    /// A field value was out of range or otherwise malformed.
    InvalidValue {
        /// The offending field, e.g. `"DateValue.month"`.
        field: &'static str,
        /// Human-readable explanation.
        reason: String,
    },
    /// A mount entry was invalid or could not be mounted.
    InvalidMount(String),
}

impl fmt::Display for ProtoConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field {field}"),
            Self::UnknownExcType(name) => write!(f, "unknown exception type {name:?}"),
            Self::UnknownType(name) => write!(f, "unknown type name {name:?}"),
            Self::UnknownBuiltinFunction(name) => write!(f, "unknown builtin function {name:?}"),
            Self::InvalidFileMode(mode) => write!(f, "invalid file mode {mode:?}"),
            Self::OutputOnly(kind) => write!(f, "{kind} values are output-only and cannot be used as inputs"),
            Self::InvalidValue { field, reason } => write!(f, "invalid value for {field}: {reason}"),
            Self::InvalidMount(reason) => write!(f, "invalid mount: {reason}"),
        }
    }
}

impl error::Error for ProtoConvertError {}

/// Converts a slice of monty values to wire values.
#[must_use]
pub fn values_to_proto(values: &[MontyObject]) -> Vec<pb::MontyValue> {
    values.iter().map(pb::MontyValue::from).collect()
}

/// Converts wire values back to monty values, failing on the first invalid one.
pub fn values_from_proto(values: Vec<pb::MontyValue>) -> Result<Vec<MontyObject>, ProtoConvertError> {
    values.into_iter().map(MontyObject::try_from).collect()
}

/// Converts monty key/value pairs (kwargs, dict contents) to wire pairs.
#[must_use]
pub fn pairs_to_proto(pairs: &[(MontyObject, MontyObject)]) -> Vec<pb::Pair> {
    pairs
        .iter()
        .map(|(key, value)| pb::Pair {
            key: Some(key.into()),
            value: Some(value.into()),
        })
        .collect()
}

/// Converts wire pairs back to monty key/value pairs.
pub fn pairs_from_proto(pairs: Vec<pb::Pair>) -> Result<Vec<(MontyObject, MontyObject)>, ProtoConvertError> {
    pairs
        .into_iter()
        .map(|pair| {
            let key = pair.key.ok_or(ProtoConvertError::MissingField("Pair.key"))?;
            let value = pair.value.ok_or(ProtoConvertError::MissingField("Pair.value"))?;
            Ok((key.try_into()?, value.try_into()?))
        })
        .collect()
}
