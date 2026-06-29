// napi macros generate code that triggers some clippy lints
#![allow(clippy::needless_pass_by_value, clippy::trivially_copy_pass_by_ref)]

//! Node.js bindings for the Monty sandboxed Python interpreter: the subprocess
//! pool ([`NativePool`]/[`NativeSession`]), which runs crash-isolated execution
//! in `monty --subprocess` workers via the `monty-pool` crate. `ts/` wraps it
//! into the public `Monty`/`MontySession` classes.
//!
//! This crate is native-only. Browsers (where subprocesses do not exist) run
//! the sandbox in a Web Worker via the lean `monty-wasm` module and the
//! TypeScript pool in `ts/worker/`, not through napi — so there is no longer an
//! in-process napi surface or a wasm napi build.

mod convert;
mod exceptions;
mod limits;
mod pool;

pub use exceptions::{ExceptionInfo, Frame, JsMontyException, MontyTypingError};
pub use limits::JsResourceLimits;
pub use pool::{NativeCheckoutOptions, NativeMount, NativePool, NativePoolOptions, NativeSession, MAX_VALUE_DEPTH};
