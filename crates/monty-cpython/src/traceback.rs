//! Rebuilding a Monty traceback from an embedded-CPython `PyErr`.
//!
//! The wire protocol carries a full `RaisedException.traceback`, and
//! `error_from_exception` serializes whatever frames a `MontyException` holds —
//! but a `MontyException` converted from a `PyErr` via `exc_py_to_monty` arrives
//! with none. This module walks the CPython traceback object and rebuilds the
//! sandbox frames so the parent sees a real stack, not just `Type: message`.
//!
//! Only frames whose filename is the session's `script_name` (the name fed code
//! compiles under — see `runner.py`'s `run`) are kept; the driver frames inside
//! `runner.py` (`run`, `drive_async`, the `eval` calls) are dropped so the
//! traceback shows only the user's code.
//!
//! This is "tier 1" fidelity: filename, line, and function name only. CPython
//! column ranges and source-line previews are not reconstructed, so frames
//! carry no caret markers (`hide_caret: true`).

use monty::{CodeLoc, StackFrame};
use pyo3::prelude::*;

/// CPython's `co_name` for module-level code, which Monty renders as `<module>`
/// from a `None` frame name.
const MODULE_FRAME_NAME: &str = "<module>";

/// Walks `err`'s CPython traceback into Monty stack frames, outermost first.
///
/// `script_name` is the filename fed code was compiled under; only frames from
/// that file are kept (driver frames live in `runner.py`).
///
/// Best-effort: any failure to read a traceback attribute stops the walk and
/// returns the frames gathered so far, so a malformed traceback degrades to a
/// shorter (or empty) one rather than masking the original exception.
pub fn py_traceback_frames(py: Python<'_>, err: &PyErr, script_name: &str) -> Vec<StackFrame> {
    let mut frames = Vec::new();
    let Some(tb) = err.traceback(py) else {
        return frames;
    };
    let mut current = tb.into_any();
    loop {
        match frame_from_tb(&current, script_name) {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) => {}   // a runner/internal frame, skipped
            Err(_) => break, // malformed traceback: keep what we have
        }
        current = match current.getattr("tb_next") {
            Ok(next) if !next.is_none() => next,
            _ => break,
        };
    }
    frames
}

/// Builds a `StackFrame` for one CPython traceback node, or `None` if the node
/// belongs to another file (a `runner.py` driver frame to be dropped).
fn frame_from_tb(tb: &Bound<'_, PyAny>, script_name: &str) -> PyResult<Option<StackFrame>> {
    let frame = tb.getattr("tb_frame")?;
    let code = frame.getattr("f_code")?;
    let filename: String = code.getattr("co_filename")?.extract()?;
    if filename != script_name {
        return Ok(None);
    }
    let lineno: u32 = tb.getattr("tb_lineno")?.extract()?;
    let name: String = code.getattr("co_name")?.extract()?;
    // CPython names module-level code `<module>`; Monty renders that from a
    // `None` frame name, so map it back rather than carrying the literal.
    let frame_name = (name != MODULE_FRAME_NAME).then_some(name);
    // Tier 1: line only. No column range or source preview is reconstructed, so
    // the start/end positions are a bare line and carets are suppressed. The
    // wire decoder only validates columns when a preview line is present, so a
    // zero column here roundtrips cleanly.
    let position = CodeLoc {
        line: lineno,
        column: 0,
    };
    Ok(Some(StackFrame {
        filename,
        start: position,
        end: position,
        frame_name,
        preview_line: None,
        hide_caret: true,
        hide_frame_name: false,
    }))
}
