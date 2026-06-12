//! Wire protocol shared by `monty --subprocess` children and their parents.
//!
//! Monty executes untrusted Python, and a monty process can never be made
//! fully crash-proof against memory errors (stack overflow, allocator
//! aborts). The subprocess protocol isolates those crashes: a parent drives a
//! pool of `monty --subprocess` children over stdin/stdout, and a dead child
//! is simply respawned. Protobuf is used (rather than monty's internal
//! postcard format) so a parent can be implemented in any language — see
//! `proto/monty/v1/monty.proto` for the schema and protocol rules.
//!
//! This crate provides everything both sides need:
//!
//! - [`pb`] — prost-generated message types (checked in; regenerate with
//!   `make generate-proto`)
//! - [`FrameReader`] / [`FrameWriter`] — 4-byte LE length-prefixed framing
//! - conversions between [`pb`] types and monty's public types
//!   ([`monty::MontyObject`], [`monty::MontyException`], ...)
//!
//! Conversions from proto to Rust are fallible by design: a parent must treat
//! frames from a (possibly compromised) child as untrusted input.

mod convert;
mod frame;
mod generated;
mod wire;

pub use convert::{
    MAX_VALUE_DEPTH, ProtoConvertError, build_mount_table, exceeds_max_value_depth, future_results_from_proto,
    pairs_from_proto, pairs_to_proto, values_from_proto, values_to_proto,
};
pub use frame::{FrameError, FrameReader, FrameWriter, MAX_FRAME_LEN};
pub use generated::pb;
pub use wire::WireObject;

/// Version of the wire protocol implemented by this crate.
///
/// Exchanged in the `Hello` handshake; bumped only on semantic breaks
/// (additive schema changes do not require a bump).
pub const PROTOCOL_VERSION: u32 = 1;
