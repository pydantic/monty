#![doc = include_str!("../README.md")]

use std::{ops::RangeInclusive, time::Duration};

mod budget_vec;
#[doc(hidden)]
pub mod budgeted_prost;
mod convert;
mod decode_budget;
mod frame;
mod generated;
// Python ↔ MontyObject value conversion; opt-in because it links pyo3, which
// pure-Rust consumers of the wire protocol must never pay for.
#[cfg(feature = "python")]
pub mod python;
mod requirement;
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub mod test_util;
mod wire;
#[cfg(feature = "worker")]
pub mod worker;

/// Version of the wire schema this build speaks, sent in
/// [`pb::Configure::protocol_version`] and range-checked by the child.
///
/// Bump on any change a peer at the previous version could mis-read: removing
/// or repurposing a field, changing a field's meaning, or adding one either
/// side requires. Purely additive changes an older peer can ignore do not need
/// a bump.
///
/// Version 6 added the required `position` on every suspension event: a
/// version 5 child would announce suspensions without it, which a parent at
/// this version rejects, so such a child refuses this version instead.
pub const PROTOCOL_VERSION: u32 = 6;

/// Oldest [`PROTOCOL_VERSION`] this build still serves.
///
/// A version 5 parent ignores the `position` a version 6 child sends, so it
/// is still served. Version 4 and below are not served. Version 3 carried values as recursive
/// `MontyObject` trees, where this build carries one flat `Arena` per message.
/// Version 4 both lacked `max_feed_duration`/`max_turn_duration` and had the
/// per-session `max_duration` this build dropped: a version 4 parent would
/// send a budget nothing enforces, and a version 4 child would accept the new
/// budgets and ignore them. Neither side can be told apart from a working one,
/// so both are refused.
pub const MIN_SUPPORTED_PROTOCOL_VERSION: u32 = 5;

/// How long the child holds buffered `print()` output before emitting it as a
/// `Print` event, when [`pb::Configure::print_flush_interval_ms`] says nothing.
///
/// Short enough to read as live output, long enough that a printing loop emits
/// events at a rate set by elapsed time rather than by how often the program
/// called `print()`.
pub const DEFAULT_PRINT_FLUSH_INTERVAL: Duration = Duration::from_millis(5);

// The supported range must be non-empty, and must exclude zero
const _: () = assert!(MIN_SUPPORTED_PROTOCOL_VERSION >= 1);
const _: () = assert!(MIN_SUPPORTED_PROTOCOL_VERSION <= PROTOCOL_VERSION);

/// Protocol versions this build serves; a `Configure` outside it is fatal.
const SUPPORTED_PROTOCOL_VERSIONS: RangeInclusive<u32> = MIN_SUPPORTED_PROTOCOL_VERSION..=PROTOCOL_VERSION;

/// Checks a peer's declared [`pb::Configure::protocol_version`] against the
/// range this build serves.
pub fn check_protocol_version(version: u32) -> Result<(), String> {
    if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
        Ok(())
    } else {
        // Singular while the window is one version wide — "2 to 2" reads as a range it is not.
        // `let`-bound so the `format_args!` temporaries live long enough to be formatted below.
        // use "server" since this error should only ever happen when using a client/server architecture
        let supported = if MIN_SUPPORTED_PROTOCOL_VERSION == PROTOCOL_VERSION {
            format_args!("server supports protocol version {PROTOCOL_VERSION}")
        } else {
            format_args!("server supports protocol versions {MIN_SUPPORTED_PROTOCOL_VERSION} to {PROTOCOL_VERSION}")
        };

        // Points the reader at the client for the refusal above: the worker is typically a deployed build the
        // reader cannot change, so the client is the half to move.
        let tip = if version < MIN_SUPPORTED_PROTOCOL_VERSION {
            "try updating to a newer client version"
        } else {
            "make sure you are using the correct client version"
        };

        Err(format!("unsupported protocol version {version} ({supported}, {tip})"))
    }
}

pub use budget_vec::BudgetVec;
pub use convert::{
    ProtoConvertError, ext_result_from_proto, ext_result_to_proto, future_results_from_proto, future_results_to_proto,
    named_values_from_proto, named_values_to_proto, os_call_from_proto, os_call_to_proto, resume_call_from_proto,
};
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub use decode_budget::{decode_budget_remaining, with_decode_budget};
pub use frame::{
    DEFAULT_MAX_DECODE_BYTES, FrameError, FrameReader, MAX_FRAME_LEN, decode_frame, encode_framed_into,
    encode_to_capped_vec, exceeds_max_frame_len, write_frame,
};
pub use generated::pb;
pub use requirement::validate_requirement;
pub use wire::{WireArena, WireFunctionCall, WireIndexes, WireNamedTuple, WireNodePairs};
