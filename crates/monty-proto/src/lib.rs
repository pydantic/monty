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

pub use convert::{MAX_VALUE_DEPTH, ProtoConvertError, exceeds_max_value_depth, future_results_from_proto};
pub use frame::{
    DEFAULT_MAX_DECODE_BYTES, FrameError, FrameReader, MAX_FRAME_LEN, decode_frame, encode_framed_into,
    encode_to_capped_vec, exceeds_max_frame_len, write_frame,
};
pub use generated::pb;
pub use requirement::validate_requirement;
pub use wire::{WireFunctionCall, WireObject, reset_decode_budget};
