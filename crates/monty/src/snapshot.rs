use std::fmt::Debug;

use crate::{
    args::ArgValues,
    exception_private::{ExceptionRaise, SimpleException},
    for_iterator::ForIterator,
    intern::{ExtFunctionId, FunctionId, StringId},
    namespace::NamespaceId,
    parse::CodeRange,
    value::Value,
};

/// Result of executing a frame - return, yield, or external function call.
///
/// When a frame encounters a `return` statement, it produces `Return(value)`.
///
/// When a frame encounters a call to an external function, it produces
/// `FunctionCall` to pause execution and let the host provide the return value.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum FrameExit {
    /// Normal return from a function or end of module execution.
    Return(Value),
    /// External function call pauses execution.
    ///
    /// The host must provide the return value to resume execution. The arguments
    /// have already been evaluated and converted to `Value`.
    ExternalCall(ExternalCall),
}

/// State of a suspended function call for snapshot/resume.
///
/// When an external function is called from inside a user-defined function, we need to
/// preserve the function's execution state so we can resume after the external call
/// completes. This struct captures all information needed to resume a suspended function.
///
/// The call stack is stored as a `Vec<FunctionFrame>` with the outermost function first
/// and the innermost (where the external call occurred) last.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FunctionFrame {
    /// The function being executed.
    pub function_id: FunctionId,
    /// Index of this function's namespace in the Namespaces stack.
    /// The namespace is kept alive while suspended so local variables persist.
    pub namespace_idx: NamespaceId,
    /// The function's name for stack traces (used when exception occurs inside this function).
    pub name_id: StringId,
    /// Number of captured cells from enclosing scopes (for closures).
    /// Used for proper cleanup - these cells had their ref counts incremented.
    pub captured_cell_count: usize,
    /// Saved position stack for resuming this function's execution.
    /// Each frame has its own position stack to handle nested control flow within the function.
    pub saved_positions: Vec<CodePosition>,
    /// Source position where this function was called from.
    /// Used to build proper tracebacks when exceptions propagate through suspended frames.
    pub call_position: CodeRange,
}

/// Represents a paused external function call with all information needed
/// to resume execution.
///
/// If the external call occurs inside user-defined functions, the `call_stack` contains
/// the suspended function frames from outermost to innermost.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ExternalCall {
    /// The ID of the external function being called.
    pub function_id: ExtFunctionId,
    /// The evaluated arguments to the function.
    pub args: ArgValues,
    /// Stack of suspended function frames (outermost first, innermost last).
    /// Empty when the external call is at module level.
    pub call_stack: Vec<FunctionFrame>,
    /// The source position of this external call.
    /// Used to match return values to the correct call site when resuming.
    pub call_position: CodeRange,
}

impl ExternalCall {
    /// Creates a new external function call at module level (no suspended functions).
    pub fn new(function_id: ExtFunctionId, args: ArgValues, call_position: CodeRange) -> Self {
        Self {
            function_id,
            args,
            call_stack: Vec::new(),
            call_position,
        }
    }

    /// Pushes a function frame onto the call stack.
    ///
    /// Called when an external call propagates up through a user-defined function.
    /// Frames are pushed in order as the call unwinds, so innermost is first.
    /// The caller must reverse the stack before resuming to get outermost-first order.
    pub fn push_frame(&mut self, frame: FunctionFrame) {
        self.call_stack.push(frame);
    }
}

/// Cached state for partially-evaluated arguments when an external call suspends.
///
/// When evaluating arguments for a call and one of them triggers an external call,
/// we need to remember which arguments have already been evaluated to avoid
/// re-evaluating them (and their side effects) on resume.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ArgumentCache {
    /// The position of the call expression whose arguments are being cached.
    pub call_position: CodeRange,
    /// Arguments that were evaluated before the external call suspended execution.
    /// These are stored in order and should be used directly on resume.
    pub evaluated_args: Vec<Value>,
    /// Index of the argument that triggered the external call.
    /// On resume, this argument gets the cached return value, and evaluation
    /// continues from the next argument.
    pub suspended_at_arg: usize,
}

pub trait AbstractSnapshotTracker: Debug {
    /// Get the next position to execute from
    fn next(&mut self) -> CodePosition;

    /// When suspending execution, set the position to resume from
    fn record(&mut self, index: usize);

    /// When leaving an if statement or for loop, set the position to resume from
    fn set_clause_state(&mut self, clause_state: ClauseState);

    /// Whether to clear return values, this is only necessary when position is being tracked
    fn clear_return_values() -> bool;

    /// Returns the current depth of the position stack.
    /// Used to track how many positions existed before entering a function.
    fn depth(&self) -> usize;

    /// Extracts all positions added after the given depth.
    /// Returns positions added after `initial_depth` while keeping positions at or before
    /// that depth in the stack.
    fn extract_after(&mut self, initial_depth: usize) -> Vec<CodePosition>;
}

#[derive(Debug, Clone)]
pub struct NoSnapshotTracker;

impl AbstractSnapshotTracker for NoSnapshotTracker {
    fn next(&mut self) -> CodePosition {
        CodePosition::default()
    }

    fn record(&mut self, _index: usize) {}

    fn set_clause_state(&mut self, _clause_state: ClauseState) {}

    fn clear_return_values() -> bool {
        false
    }

    fn depth(&self) -> usize {
        0
    }

    fn extract_after(&mut self, _initial_depth: usize) -> Vec<CodePosition> {
        Vec::new()
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SnapshotTracker {
    /// stack of positions, note this is reversed (last value is the outermost position)
    /// as we push the outermost position last and pop it first
    stack: Vec<CodePosition>,
    clause_state: Option<ClauseState>,
}

impl SnapshotTracker {
    pub fn new(stack: Vec<CodePosition>) -> Self {
        SnapshotTracker {
            stack,
            clause_state: None,
        }
    }

    pub fn into_stack(self) -> Vec<CodePosition> {
        self.stack
    }
}

impl AbstractSnapshotTracker for SnapshotTracker {
    fn next(&mut self) -> CodePosition {
        self.stack.pop().unwrap_or_default()
    }

    fn record(&mut self, index: usize) {
        self.stack.push(CodePosition {
            index,
            clause_state: self.clause_state.take(),
        });
    }

    fn set_clause_state(&mut self, clause_state: ClauseState) {
        self.clause_state = Some(clause_state);
    }

    fn clear_return_values() -> bool {
        true
    }

    fn depth(&self) -> usize {
        self.stack.len()
    }

    fn extract_after(&mut self, initial_depth: usize) -> Vec<CodePosition> {
        if self.stack.len() > initial_depth {
            self.stack.split_off(initial_depth)
        } else {
            Vec::new()
        }
    }
}

/// Represents a position within nested control flow for snapshotting and code resumption.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CodePosition {
    /// Index of the next node to execute within the node array
    pub index: usize,
    /// indicates how to resume within the nested control flow if relevant
    pub clause_state: Option<ClauseState>,
}

/// State for resuming execution within control flow structures.
///
/// When execution suspends inside a control flow structure (if/for), this records
/// which branch was taken so we can skip re-evaluating the condition on resume.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum ClauseState {
    /// When resuming within the if statement,
    /// whether the condition was met - true to resume the if branch and false to resume the else branch
    If(bool),
    /// When resuming within a for loop, `ForIterator` holds the value and the index of the next element
    /// for iteration.
    For(ForIterator),
    /// When resuming within a try/except/finally block.
    Try(TryClauseState),
}

/// State for resuming within a try/except/finally block after an external call.
///
/// Tracks which phase of the try/except we're in and any pending state that must
/// survive external calls in the finally block. Pending exceptions and returns
/// are stored here so finally blocks can make external calls and still properly
/// propagate exceptions or return values afterward.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TryClauseState {
    /// Which phase of the try/except block we're in.
    pub phase: TryPhase,
    /// If in ExceptHandler phase, which handler index we're executing.
    pub handler_index: Option<u16>,
    /// Pending exception to re-raise after finally completes.
    pub pending_exception: Option<ExceptionRaise>,
    /// Pending return value to return after finally completes.
    pub pending_return: Option<Value>,
    /// Previous current_exception for nested handlers so bare raise keeps working.
    pub enclosing_exception: Option<SimpleException>,
}

/// Which phase of a try/except/finally block we're executing.
///
/// The order of variants matters for `PartialOrd` - earlier phases come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TryPhase {
    /// Executing the try body.
    TryBody,
    /// Executing an except handler body.
    ExceptHandler,
    /// Executing the else block (runs if no exception).
    Else,
    /// Executing the finally block.
    Finally,
}
