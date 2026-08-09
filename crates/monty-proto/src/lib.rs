#![doc = include_str!("../README.md")]

use std::ops::RangeInclusive;

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

/// Protocol versions this build serves; a `Configure` outside it is fatal.
///
/// Zero is excluded by construction, so a peer that declared nothing — one
/// predating `Configure.protocol_version`, or not a monty peer at all — is
/// rejected rather than assumed compatible.
const SUPPORTED_PROTOCOL_VERSIONS: RangeInclusive<u32> = MIN_SUPPORTED_PROTOCOL_VERSION..=PROTOCOL_VERSION;

/// Checks a peer's declared [`pb::Configure::protocol_version`] against the
/// range this build serves.
///
/// Lives here rather than in [`worker`] because every peer that terminates a
/// `Configure` runs the same check and owes the peer the same sentence: the
/// child, and equally a relay that serves clients over its own transport. The
/// `Err` string is that sentence, ready to put in a `FatalError` — callers
/// append their own diagnostics (package versions, say) rather than reword it,
/// so a client reads one wording wherever it is refused.
///
/// ```
/// use monty_proto::{PROTOCOL_VERSION, check_protocol_version};
///
/// assert!(check_protocol_version(PROTOCOL_VERSION).is_ok());
/// // A peer that declared no version sends zero, which is never servable.
/// assert!(check_protocol_version(0).is_err());
/// ```
///
/// # Errors
/// The refusal message naming what this build does support — the only way a
/// peer learns what to downgrade to, since the protocol has no handshake.
pub fn check_protocol_version(version: u32) -> Result<(), String> {
    if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(format!(
            "unsupported protocol version {version} ({})",
            supported_versions()
        ))
    }
}

/// Names [`SUPPORTED_PROTOCOL_VERSIONS`] for the refusal above. Singular while
/// the window is one version wide, which "1 to 1" reads as a range it is not.
fn supported_versions() -> String {
    if MIN_SUPPORTED_PROTOCOL_VERSION == PROTOCOL_VERSION {
        format!("this build supports protocol version {PROTOCOL_VERSION}")
    } else {
        format!("this build supports protocol versions {MIN_SUPPORTED_PROTOCOL_VERSION} to {PROTOCOL_VERSION}")
    }
}

pub use convert::{MAX_VALUE_DEPTH, ProtoConvertError, exceeds_max_value_depth, future_results_from_proto};
pub use frame::{
    DEFAULT_MAX_DECODE_BYTES, FrameError, FrameReader, MAX_FRAME_LEN, decode_frame, encode_framed_into,
    encode_to_capped_vec, exceeds_max_frame_len, write_frame,
};
pub use generated::pb;
pub use requirement::validate_requirement;
pub use wire::{WireFunctionCall, WireObject, reset_decode_budget};
