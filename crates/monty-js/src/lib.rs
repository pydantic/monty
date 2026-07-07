// napi macros generate code that triggers some clippy lints
#![allow(clippy::needless_pass_by_value, clippy::trivially_copy_pass_by_ref)]

//! Node.js/TypeScript bindings for the Monty sandboxed Python interpreter.
//!
//! Two execution surfaces share this crate:
//!
//! - **The subprocess pool** ([`NativePool`]/[`NativeSession`], native targets
//!   only): crash-isolated execution in `monty subprocess` workers via the
//!   `monty-pool` crate — the primary API, wrapped by `ts/` into the public
//!   `Monty`/`MontySession` classes.
//! - **The in-process API** ([`Monty`], [`MontyRepl`], [`MontySnapshot`],
//!   ...): runs the interpreter inside this process. The only option on
//!   wasm/browsers (no subprocesses there) and exposed under the
//!   `@pydantic/monty/wasm` subpath; a sandbox crash (stack overflow,
//!   allocator abort) takes the host process with it.

mod convert;
mod exceptions;
mod limits;
mod monty_cls;
mod mount;
// The subprocess pool spawns worker processes, which wasm cannot do — wasm
// builds expose only the in-process API.
#[cfg(not(target_arch = "wasm32"))]
mod pool;

pub use exceptions::{ExceptionInfo, Frame, JsMontyException, MontyTypingError};
pub use limits::JsResourceLimits;
pub use monty_cls::{
    ExceptionInput, Monty, MontyComplete, MontyNameLookup, MontyOptions, MontyRepl, MontySnapshot,
    NameLookupLoadOptions, NameLookupResumeOptions, ResumeOptions, RunOptions, SnapshotLoadOptions, StartOptions,
};
pub use mount::{MountDir, MountDirOptions};
#[cfg(not(target_arch = "wasm32"))]
pub use pool::{NativeCheckoutOptions, NativeMount, NativePool, NativePoolOptions, NativeSession, MAX_VALUE_DEPTH};
