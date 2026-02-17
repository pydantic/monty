// napi macros generate code that triggers some clippy lints
#![allow(clippy::needless_pass_by_value)]

//! Node.js/TypeScript bindings for the Monty sandboxed Python interpreter.
//!
//! This module provides a JavaScript/TypeScript interface to Monty via napi-rs,
//! allowing execution of sandboxed Python code from Node.js with configurable
//! inputs, resource limits, and external function callbacks.
//!
//! ## Quick Start
//!
//! ```typescript
//! import { Monty } from 'monty';
//!
//! // Simple execution
//! const m = new Monty('1 + 2');
//! const result = m.run(); // returns 3
//!
//! // With inputs
//! const m2 = new Monty('x + y', { inputs: ['x', 'y'] });
//! const result2 = m2.run({ inputs: { x: 10, y: 20 } }); // returns 30
//!
//! // Iterative execution with external functions
//! const m3 = new Monty('external_func()', { externalFunctions: ['external_func'] });
//! let progress = m3.start();
//! if (progress instanceof MontySnapshot) {
//!     progress = progress.resume({ returnValue: 42 });
//! }
//! ```

mod convert;
mod exceptions;
mod limits;
mod monty_cls;

pub use exceptions::{ExceptionInfo, Frame, JsMontyException, MontyTypingError};
pub use limits::JsResourceLimits;
pub use monty_cls::{
    ExceptionInput, Monty, MontyComplete, MontyOptions, MontyRepl, MontySnapshot, ResumeOptions, RunOptions,
    SnapshotLoadOptions, StartOptions,
};
