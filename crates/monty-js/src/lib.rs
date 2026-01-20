//! Node.js bindings for the Monty sandboxed Python interpreter.
//!
//! This module provides a JavaScript/TypeScript interface to Monty via napi-rs,
//! allowing execution of sandboxed Python code from Node.js.

use monty::{CollectStringPrint, MontyRun, NoLimitTracker};
use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Runs Python code and returns the result as a string.
///
/// The code is executed in a sandboxed environment with no resource limits.
/// Print statements are captured and returned along with the final result.
///
/// # Arguments
/// * `code` - The Python code to execute
///
/// # Returns
/// A `RunResult` containing the printed output and the result of the last expression.
///
/// # Errors
/// Returns an error if the code fails to parse or encounters a runtime error.
#[napi]
pub fn run(code: String) -> Result<RunResult> {
    let runner = MontyRun::new(code, "main.py", vec![], vec![]).map_err(monty_err_to_napi)?;

    let mut print_output = CollectStringPrint::default();
    let result = runner
        .run(vec![], NoLimitTracker, &mut print_output)
        .map_err(monty_err_to_napi)?;

    Ok(RunResult {
        output: print_output.into_output(),
        result: format!("{result:?}"),
    })
}

/// Result of running Python code.
#[napi(object)]
pub struct RunResult {
    /// Any output from print statements during execution.
    pub output: String,
    /// The debug representation of the final result.
    pub result: String,
}

/// Converts a `MontyException` to a napi `Error`.
fn monty_err_to_napi(e: monty::MontyException) -> Error {
    Error::from_reason(e.to_string())
}
