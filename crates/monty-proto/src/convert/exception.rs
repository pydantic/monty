//! `MontyException` ↔ `pb::MontyError` conversions, including full traceback
//! frames so an exception raised on one side of the process boundary renders
//! identically on the other.

use std::sync::Arc;

use monty::{CodeLoc, MontyException, StackFrame};

use crate::{convert::ProtoConvertError, pb};

impl From<&MontyException> for pb::MontyError {
    fn from(exc: &MontyException) -> Self {
        Self {
            exc_type: exc.exc_type().to_string(),
            message: exc.message().map(ToOwned::to_owned),
            traceback: exc.traceback().iter().map(pb::StackFrame::from).collect(),
        }
    }
}

impl TryFrom<pb::MontyError> for MontyException {
    type Error = ProtoConvertError;

    fn try_from(err: pb::MontyError) -> Result<Self, ProtoConvertError> {
        let exc_type = err
            .exc_type
            .parse()
            .map_err(|_| ProtoConvertError::UnknownExcType(err.exc_type))?;
        let traceback = err
            .traceback
            .into_iter()
            .map(StackFrame::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::with_traceback(exc_type, err.message, traceback))
    }
}

impl From<&StackFrame> for pb::StackFrame {
    fn from(frame: &StackFrame) -> Self {
        Self {
            filename: frame.filename.clone(),
            start: Some(code_loc_to_proto(frame.start)),
            end: Some(code_loc_to_proto(frame.end)),
            frame_name: frame.frame_name.clone(),
            preview_line: frame.preview_line.as_ref().map(ToString::to_string),
            hide_caret: frame.hide_caret,
            hide_frame_name: frame.hide_frame_name,
        }
    }
}

impl TryFrom<pb::StackFrame> for StackFrame {
    type Error = ProtoConvertError;

    fn try_from(frame: pb::StackFrame) -> Result<Self, ProtoConvertError> {
        let start = code_loc_from_proto(frame.start.ok_or(ProtoConvertError::MissingField("StackFrame.start"))?);
        let end = code_loc_from_proto(frame.end.ok_or(ProtoConvertError::MissingField("StackFrame.end"))?);
        // Frames are untrusted wire data, and `StackFrame`'s `Display` derives
        // caret padding/width from the columns when a preview line is present
        // (`" ".repeat(start.column..)`, `end.column - start.column`).
        // Unvalidated coordinates would let a compromised peer trigger an
        // integer-underflow panic or a multi-gigabyte allocation the moment
        // the traceback is rendered. Monty itself only attaches a preview
        // when start and end lie on the same line with columns inside it, so
        // rejecting anything else loses no legitimate frames.
        if let Some(preview) = &frame.preview_line {
            if end.column < start.column {
                return Err(ProtoConvertError::InvalidValue {
                    field: "StackFrame.end.column",
                    reason: format!("{} is before start column {}", end.column, start.column),
                });
            }
            // +2 slack: columns are 1-indexed with an exclusive end, and
            // resolving the end of a CRLF line lands one further past the
            // stripped preview text — the exact bound matters less than
            // keeping the caret math proportional to the line
            let line_chars = u32::try_from(preview.chars().count()).unwrap_or(u32::MAX);
            if end.column > line_chars.saturating_add(2) {
                return Err(ProtoConvertError::InvalidValue {
                    field: "StackFrame.end.column",
                    reason: format!("{} is beyond the {line_chars}-character preview line", end.column),
                });
            }
        }
        Ok(Self {
            filename: frame.filename,
            start,
            end,
            frame_name: frame.frame_name,
            preview_line: frame.preview_line.map(Arc::from),
            hide_caret: frame.hide_caret,
            hide_frame_name: frame.hide_frame_name,
        })
    }
}

fn code_loc_to_proto(loc: CodeLoc) -> pb::CodeLoc {
    pb::CodeLoc {
        line: loc.line,
        column: loc.column,
    }
}

fn code_loc_from_proto(loc: pb::CodeLoc) -> CodeLoc {
    CodeLoc {
        line: loc.line,
        column: loc.column,
    }
}
