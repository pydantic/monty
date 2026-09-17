// napi macros generate code that triggers some clippy lints
#![expect(clippy::needless_pass_by_value, clippy::trivially_copy_pass_by_ref)]
#![doc = include_str!("../README.md")]

//! # Rust binding internals
//!
//! This crate is native-only. Browsers (where subprocesses do not exist) run
//! the sandbox in a Web Worker via the lean `monty-wasm-runtime` module and the
//! TypeScript pool in `ts/worker/`, not through napi — so there is no longer an
//! in-process napi surface or a wasm napi build.

mod convert;
mod exceptions;
mod limits;
mod pool;
mod telemetry;

pub use exceptions::{ExceptionInfo, Frame, JsMontyException};
pub use limits::JsResourceLimits;
pub use pool::{NativeCheckoutOptions, NativeMount, NativePool, NativePoolOptions, NativeSession, MAX_VALUE_DEPTH};
pub use telemetry::{flush_telemetry, install_telemetry, set_telemetry_metrics_enabled};

/// Returns the package version used for the OpenTelemetry instrumentation scope.
#[napi_derive::napi(js_name = "_montyVersion")]
#[must_use]
pub const fn monty_version() -> &'static str {
    monty_types::MONTY_VERSION
}
