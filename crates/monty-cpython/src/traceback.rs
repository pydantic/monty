//! Rebuilding a Monty traceback from an embedded-CPython exception.
//!
//! The wire protocol carries a full `RaisedException.traceback`, and
//! `error_from_exception` serializes whatever frames a `MontyException` holds —
//! but a `MontyException` converted from a `PyErr` via `exc_py_to_monty` arrives
//! with none. This module fills them in.
//!
//! The heavy lifting lives in `runner.py`'s `extract_traceback`, which walks the
//! CPython traceback using CPython's own machinery: it keeps only user frames,
//! recovers each frame's source line from `linecache`, and takes the anchored
//! column span and caret-visibility decision straight from CPython's renderer
//! (so `raise` and whole-line cases match exactly). This Rust side just hands it
//! the traceback object and maps the returned tuples onto [`StackFrame`].

use std::{collections::HashMap, sync::Arc};

use monty::{CodeLoc, StackFrame};
use pyo3::prelude::*;

use crate::pyexec::Runner;

/// One frame as returned by `runner.py`'s `extract_traceback`, in declaration
/// order: `(filename, start_line, start_col, end_line, end_col, frame_name,
/// preview_line, hide_caret, hide_frame_name)`. Lines/columns are 1-based and
/// columns count characters.
type FrameTuple = (String, u32, u32, u32, u32, Option<String>, Option<String>, bool, bool);

/// Rebuilds `err`'s traceback into Monty stack frames, outermost first, with
/// `script_name` as every frame's reported filename.
///
/// Best-effort: a failure talking to the Python extractor yields an empty
/// traceback rather than masking the original exception. This is only a true
/// last resort — `extract_traceback` itself already falls back to a stdlib-free
/// walk when the rich path fails (e.g. the sandbox monkey-patched
/// `traceback`/`linecache`), so it still returns frames rather than erasing
/// them.
pub fn py_traceback_frames(py: Python<'_>, runner: &Runner, err: &PyErr, script_name: &str) -> Vec<StackFrame> {
    extract(py, runner, err, script_name).unwrap_or_default()
}

/// The fallible core: invokes `extract_traceback` and converts its result.
fn extract(py: Python<'_>, runner: &Runner, err: &PyErr, script_name: &str) -> PyResult<Vec<StackFrame>> {
    let Some(traceback) = err.traceback(py) else {
        return Ok(Vec::new());
    };
    let result = runner.extract_traceback(py, &traceback, script_name)?;
    let mut frames = Vec::new();
    // Share one `Arc` per distinct preview line, the same sharing `StackFrame`
    // relies on: a deep recursion on a long line would otherwise clone the whole
    // line into every frame and amplify memory by the call depth.
    let mut previews: HashMap<String, Arc<str>> = HashMap::new();
    for item in result.try_iter()? {
        let (filename, start_line, start_col, end_line, end_col, frame_name, preview_line, hide_caret, hide_frame_name): FrameTuple =
            item?.extract()?;
        let preview_line = preview_line.map(|line| {
            previews
                .entry(line)
                .or_insert_with_key(|line| Arc::from(line.as_str()))
                .clone()
        });
        frames.push(StackFrame {
            filename,
            start: CodeLoc {
                line: start_line,
                column: start_col,
            },
            end: CodeLoc {
                line: end_line,
                column: end_col,
            },
            frame_name,
            preview_line,
            hide_caret,
            hide_frame_name,
        });
    }
    Ok(frames)
}
