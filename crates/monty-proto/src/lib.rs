#![doc = include_str!("../README.md")]

mod convert;
mod frame;
mod generated;
// Python ↔ MontyObject value conversion; opt-in because it links pyo3, which
// pure-Rust consumers of the wire protocol must never pay for.
#[cfg(feature = "python")]
pub mod python;
mod requirement;
mod wire;
#[cfg(feature = "worker")]
pub mod worker;

/// Version of the wire schema this build speaks, sent in
/// [`pb::Configure::protocol_version`] and range-checked by the child.
///
/// Bump on any change a peer at the previous version could mis-read: removing
/// or repurposing a field, changing a field's meaning, or adding one the child
/// requires. Purely additive changes an older peer can ignore do not need a
/// bump.
pub const PROTOCOL_VERSION: u32 = 1;

/// Oldest [`PROTOCOL_VERSION`] this build still serves.
///
/// The gap between this and `PROTOCOL_VERSION` is the migration window: how far
/// a separately-deployed parent (a WebSocket client, say) may lag behind its
/// worker before being rejected. Raise it only when supporting an old version
/// becomes a real cost — every raise is a breaking change for lagging clients.
pub const MIN_SUPPORTED_PROTOCOL_VERSION: u32 = 1;

// The supported range must be non-empty, and must exclude zero — which is what
// a parent that declared no version sends, and must never be servable.
const _: () = assert!(MIN_SUPPORTED_PROTOCOL_VERSION >= 1);
const _: () = assert!(MIN_SUPPORTED_PROTOCOL_VERSION <= PROTOCOL_VERSION);

pub use convert::{MAX_VALUE_DEPTH, ProtoConvertError, exceeds_max_value_depth, future_results_from_proto};
pub use frame::{
    DEFAULT_MAX_DECODE_BYTES, FrameError, FrameReader, MAX_FRAME_LEN, decode_frame, encode_framed_into,
    encode_to_capped_vec, exceeds_max_frame_len, write_frame,
};
pub use generated::pb;
pub use requirement::validate_requirement;
pub use wire::{WireFunctionCall, WireObject, reset_decode_budget};
