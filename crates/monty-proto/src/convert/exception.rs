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
        Ok(Self {
            filename: frame.filename,
            start: code_loc_from_proto(frame.start.ok_or(ProtoConvertError::MissingField("StackFrame.start"))?),
            end: code_loc_from_proto(frame.end.ok_or(ProtoConvertError::MissingField("StackFrame.end"))?),
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
