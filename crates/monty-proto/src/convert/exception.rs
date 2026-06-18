//! `MontyException` ↔ `pb::RaisedException` conversions, including full traceback
//! frames so an exception raised on one side of the process boundary renders
//! identically on the other.

use std::sync::Arc;

use monty::{CodeLoc, MontyException, StackFrame};

use crate::{convert::ProtoConvertError, pb};

impl From<&MontyException> for pb::RaisedException {
    fn from(exc: &MontyException) -> Self {
        Self {
            exc_type: exc.exc_type().to_string(),
            message: exc.message().map(ToOwned::to_owned),
            traceback: exc.traceback().iter().map(pb::StackFrame::from).collect(),
        }
    }
}

impl TryFrom<pb::RaisedException> for MontyException {
    type Error = ProtoConvertError;

    fn try_from(err: pb::RaisedException) -> Result<Self, ProtoConvertError> {
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
            start: Some(frame.start.into()),
            end: Some(frame.end.into()),
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
        let start = CodeLoc::from(frame.start.ok_or(ProtoConvertError::MissingField("StackFrame.start"))?);
        let end = CodeLoc::from(frame.end.ok_or(ProtoConvertError::MissingField("StackFrame.end"))?);
        // Frames are untrusted wire data, and `StackFrame`'s `Display` derives
        // caret padding/width from the columns when a preview is present.
        // Unvalidated coordinates would let a compromised peer trigger an
        // integer-underflow panic or a huge allocation when the traceback is
        // rendered. Monty only attaches a preview for same-line spans with
        // in-bounds columns, so rejecting anything else loses no real frames.
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

impl From<CodeLoc> for pb::CodeLoc {
    fn from(loc: CodeLoc) -> Self {
        Self {
            line: loc.line,
            column: loc.column,
        }
    }
}

/// Total in both directions — a `CodeLoc` is just a line/column pair. The
/// column-range validation deliberately lives in `StackFrame`'s `TryFrom`,
/// where `end` can be checked against `start` and the preview line.
impl From<pb::CodeLoc> for CodeLoc {
    fn from(loc: pb::CodeLoc) -> Self {
        Self {
            line: loc.line,
            column: loc.column,
        }
    }
}
