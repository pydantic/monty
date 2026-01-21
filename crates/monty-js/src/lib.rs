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
//! ```

mod convert;
mod exceptions;
mod limits;
mod monty;

pub use exceptions::{
    ExceptionInfo, Frame, MontyError, MontyRuntimeError, MontySyntaxError, MontyTypingError, RuntimeErrorInfo,
};
pub use limits::JsResourceLimits;
pub use monty::{Monty, MontyOptions, RunOptions};
