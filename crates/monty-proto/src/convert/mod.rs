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
//! Values cross as a flat arena per message ([`crate::WireArena`]) with the
//! message naming its roots by index, so a value's nesting depth is never a
//! wire concern; the conversions here only pair each root with its arena and
//! check the index is in range.

mod exception;
mod limits;
mod os_call;
mod resume;
mod type_checking;

use std::{error, fmt};

use monty_types::{MontyGraph, MontyObject, NamedValues, NodeId};
pub use os_call::{os_call_from_proto, os_call_to_proto};
pub use resume::{
    ext_result_from_proto, ext_result_to_proto, future_results_from_proto, future_results_to_proto, resume_call_result,
};

use crate::{
    pb,
    wire::{WireArena, graph_error},
};

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

impl From<MontyObject> for pb::Complete {
    fn from(value: MontyObject) -> Self {
        Self {
            value: value.root.0,
            values: Some(WireArena::new(value.graph)),
        }
    }
}

impl TryFrom<pb::Complete> for MontyObject {
    type Error = ProtoConvertError;

    fn try_from(complete: pb::Complete) -> Result<Self, ProtoConvertError> {
        value_from_parts(complete.values, complete.value, "Complete.values")
    }
}

/// Projects named inputs onto the wire: the `NamedRef`s and the arena they index.
#[must_use]
pub fn named_values_to_proto(inputs: NamedValues) -> (Vec<pb::NamedRef>, WireArena) {
    let refs = inputs
        .names
        .into_iter()
        .map(|(name, id)| pb::NamedRef { name, value: id.0 })
        .collect();
    (refs, WireArena::new(inputs.values))
}

/// Validates decoded named inputs against their arena.
pub fn named_values_from_proto(
    inputs: Vec<pb::NamedRef>,
    values: Option<WireArena>,
) -> Result<NamedValues, ProtoConvertError> {
    let named = NamedValues {
        values: arena_or_empty(values)?,
        names: inputs
            .into_iter()
            .map(|input| (input.name, NodeId(input.value)))
            .collect(),
    };
    named.check_roots().map_err(|err| graph_error(&err))?;
    Ok(named)
}

/// Pairs a message's arena with the root it names, rejecting an absent arena
/// (`field` names it) or an out-of-range root.
pub(crate) fn value_from_parts(
    values: Option<WireArena>,
    root: u32,
    field: &'static str,
) -> Result<MontyObject, ProtoConvertError> {
    let graph = values.ok_or(ProtoConvertError::MissingField(field))?.into_graph()?;
    MontyObject::new(graph, NodeId(root)).map_err(|err| graph_error(&err))
}

/// A message's arena, or an empty one when the field is absent (a message
/// with no value-typed fields set never needs one).
pub(crate) fn arena_or_empty(values: Option<WireArena>) -> Result<MontyGraph, ProtoConvertError> {
    values.map_or_else(|| Ok(MontyGraph::new()), WireArena::into_graph)
}
