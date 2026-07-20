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

/// The monty version this build speaks the wire protocol as, used for the
/// `Configure.monty_version` skew check. Parent and child must be deployed in
/// lockstep (the protocol has no in-band negotiation), so both sides compare
/// against this single constant instead of each reading `CARGO_PKG_VERSION`
/// independently. Equals the workspace version, since every crate shares it.
pub const MONTY_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use convert::{MAX_VALUE_DEPTH, ProtoConvertError, exceeds_max_value_depth, future_results_from_proto};
pub use frame::{
    DEFAULT_MAX_DECODE_BYTES, FrameError, FrameReader, MAX_FRAME_LEN, decode_frame, encode_to_capped_vec, write_frame,
};
pub use generated::pb;
pub use requirement::validate_requirement;
pub use wire::{WireFunctionCall, WireObject, reset_decode_budget};
