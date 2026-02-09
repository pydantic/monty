//! Code object containing compiled bytecode and metadata.
//!
//! A `Code` object represents a compiled function or module. It contains the raw
//! bytecode instructions, a constant pool, source location information for tracebacks,
//! and an exception handler table.

use std::collections::HashSet;

use crate::{intern::StringId, parse::CodeRange, value::Value};

/// Compiled bytecode for a function or module.
///
/// This is the output of the bytecode compiler and the input to the VM.
/// Each function has its own Code object; module-level code also gets one.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Code {
    /// Raw bytecode instructions as a byte vector.
    ///
    /// Opcodes are 1 byte each, followed by their operands (0-3 bytes depending
    /// on the instruction). The variable-width encoding gives better cache locality
    /// than fixed-width alternatives.
    bytecode: Vec<u8>,

    /// Constant pool for this code object.
    ///
    /// Values referenced by `LoadConst` instructions. Includes numbers, strings
    /// (as `Value::InternString`), and other literal values.
    constants: ConstPool,

    /// Source location table for tracebacks.
    ///
    /// Maps bytecode offsets to source locations. Used to generate Python-style
    /// tracebacks with line numbers and caret markers when exceptions occur.
    location_table: Vec<LocationEntry>,

    /// Exception handler table.
    ///
    /// Maps protected bytecode ranges to their exception handlers. Consulted when
    /// an exception is raised to find the appropriate handler. Entries are ordered
    /// innermost-first for nested try blocks.
    exception_table: Vec<ExceptionEntry>,

    /// Number of local variables (namespace slots needed).
    ///
    /// Used to pre-allocate the namespace when entering this code.
    num_locals: u16,

    /// Maximum stack depth needed during execution.
    ///
    /// Used as a hint for pre-allocating the operand stack. Computed during
    /// compilation by tracking push/pop operations.
    stack_size: u16,

    /// Local variable names for error messages.
    ///
    /// Maps slot indices to variable names. Used to generate proper NameError
    /// messages when accessing undefined local variables (e.g., "name 'x' is not defined").
    local_names: Vec<StringId>,

    /// Local variable slots that are assigned somewhere in this function.
    ///
    /// Used to determine whether to raise `UnboundLocalError` (slot is assigned somewhere
    /// but accessed before assignment) or `NameError` (name doesn't exist in any scope).
    assigned_locals: HashSet<u16>,
}

impl Code {
    /// Creates a new Code object with all components.
    ///
    /// This is typically called by `CodeBuilder::build()` after compilation.
    #[must_use]
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        bytecode: Vec<u8>,
        constants: ConstPool,
        location_table: Vec<LocationEntry>,
        exception_table: Vec<ExceptionEntry>,
        num_locals: u16,
        stack_size: u16,
        local_names: Vec<StringId>,
        assigned_locals: HashSet<u16>,
    ) -> Self {
        Self {
            bytecode,
            constants,
            location_table,
            exception_table,
            num_locals,
            stack_size,
            local_names,
            assigned_locals,
        }
    }

    /// Returns the raw bytecode bytes.
    #[must_use]
    pub fn bytecode(&self) -> &[u8] {
        &self.bytecode
    }

    /// Returns the constant pool.
    #[must_use]
    pub fn constants(&self) -> &ConstPool {
        &self.constants
    }

    /// Returns the local variable name for a given slot index.
    ///
    /// Used to generate proper NameError messages when accessing undefined locals.
    #[must_use]
    pub fn local_name(&self, slot: u16) -> Option<StringId> {
        self.local_names.get(slot as usize).copied()
    }

    /// Returns whether the slot is an assigned local (vs an undefined reference).
    ///
    /// Used to determine whether to raise `UnboundLocalError` (true) or `NameError` (false)
    /// when loading an undefined local variable.
    #[must_use]
    pub fn is_assigned_local(&self, slot: u16) -> bool {
        self.assigned_locals.contains(&slot)
    }

    /// Finds the location entry for a given bytecode offset.
    ///
    /// Location entries are recorded at instruction boundaries. This method finds
    /// the most recent entry at or before the given offset.
    ///
    /// Returns `None` if the location table is empty or the offset is before
    /// the first recorded location.
    #[must_use]
    pub fn location_for_offset(&self, offset: usize) -> Option<&LocationEntry> {
        // Location entries are in order by bytecode offset.
        // Find the last entry where bytecode_offset <= offset.
        let offset_u32 = u32::try_from(offset).expect("bytecode offset exceeds u32");
        self.location_table
            .iter()
            .rev()
            .find(|entry| entry.bytecode_offset <= offset_u32)
    }

    /// Finds an exception handler for the given bytecode offset.
    ///
    /// Searches the exception table for an entry whose protected range contains
    /// the given offset. Returns the first (innermost) matching handler, since
    /// entries are ordered innermost-first for nested try blocks.
    ///
    /// Returns `None` if no handler covers this offset.
    #[must_use]
    pub fn find_exception_handler(&self, offset: u32) -> Option<&ExceptionEntry> {
        self.exception_table.iter().find(|entry| entry.contains(offset))
    }

    /// Returns the first source range whose start line is after the given line.
    ///
    /// Useful for locating the first executable line inside a compound statement body.
    #[must_use]
    pub(crate) fn first_location_after_line(&self, line: u16) -> Option<CodeRange> {
        let mut best: Option<CodeRange> = None;
        for entry in &self.location_table {
            let range = entry.range();
            if range.start().line > line {
                let replace = best
                    .as_ref()
                    .is_none_or(|current| range.start().line < current.start().line);
                if replace {
                    best = Some(range);
                }
            }
        }
        best
    }
}

/// TODO remove, this doesn't add any value
/// Constant pool for a code object.
///
/// Stores literal values referenced by `LoadConst` instructions. Strings are stored
/// as `Value::InternString(StringId)` pointing to the global `Interns` table, not
/// duplicated here. At runtime, constants are loaded via `clone_with_heap()` to
/// handle reference counting properly.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ConstPool {
    /// The constant values, indexed by the operand of `LoadConst`.
    values: Vec<Value>,
}

impl Clone for ConstPool {
    fn clone(&self) -> Self {
        let values = self.values.iter().map(Value::clone_immediate).collect();
        Self { values }
    }
}

impl ConstPool {
    /// Creates a constant pool from a vector of values.
    #[must_use]
    pub fn from_vec(values: Vec<Value>) -> Self {
        Self { values }
    }

    /// Returns the constant at the given index.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds. This should never happen with
    /// valid bytecode since indices come from the compiler.
    #[must_use]
    pub fn get(&self, index: u16) -> &Value {
        &self.values[index as usize]
    }
}

/// Source location for a bytecode instruction, used for tracebacks.
///
/// Python 3.11+ tracebacks show carets under the relevant expression:
/// ```text
///    File "test.py", line 2, in foo
///      return a + b + c
///             ~~^~~
/// ```
///
/// The `range` covers the full expression (`a + b`), while `focus` points
/// to the specific operator (`+`) that caused the error.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocationEntry {
    /// Bytecode offset this entry applies to.
    ///
    /// The entry applies from this offset until the next entry's offset
    /// (or end of bytecode).
    bytecode_offset: u32,

    /// Full source range of the expression (for the underline).
    range: CodeRange,

    /// Optional focus point within the range (for the ^ caret).
    ///
    /// If None, the entire range is underlined without a focus caret.
    /// This can be populated later for Python 3.11-style focused tracebacks.
    focus: Option<CodeRange>,
}

impl LocationEntry {
    /// Creates a new location entry.
    #[must_use]
    pub fn new(bytecode_offset: u32, range: CodeRange, focus: Option<CodeRange>) -> Self {
        Self {
            bytecode_offset,
            range,
            focus,
        }
    }

    /// Returns the full source range.
    #[must_use]
    pub fn range(&self) -> CodeRange {
        self.range
    }
}

/// Entry in the exception table - maps a protected bytecode range to its handler.
///
/// Instead of maintaining a runtime stack of handlers (push/pop during execution),
/// we use a static table that's consulted when an exception is raised. This is
/// simpler and matches CPython 3.11+'s approach.
///
/// For nested try blocks, multiple entries may cover the same bytecode offset.
/// Entries are ordered innermost-first, so the VM uses the first matching entry.
///
/// # Example
///
/// For `try: x = bar(); y = baz() except ValueError as e: print(e)`:
/// ```text
/// 0:  LOAD_GLOBAL 'bar'
/// 4:  CALL_FUNCTION 0
/// 8:  STORE_LOCAL 'x'
/// ...
/// 24: JUMP 50              # skip handler if no exception
/// 30: <handler code>       # exception handler starts here
/// ```
/// Entry: `{ start: 0, end: 24, handler: 30, stack_depth: 0 }`
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ExceptionEntry {
    /// Start of protected bytecode range (inclusive).
    start: u32,

    /// End of protected bytecode range (exclusive).
    end: u32,

    /// Bytecode offset of the exception handler.
    handler: u32,

    /// Stack depth when entering the try block.
    ///
    /// Used to unwind the operand stack before jumping to handler.
    /// The VM pops values until the stack reaches this depth, then
    /// pushes the exception value.
    stack_depth: u16,
}

impl ExceptionEntry {
    /// Creates a new exception table entry.
    #[must_use]
    pub fn new(start: u32, end: u32, handler: u32, stack_depth: u16) -> Self {
        Self {
            start,
            end,
            handler,
            stack_depth,
        }
    }

    /// Returns the handler bytecode offset.
    #[must_use]
    pub fn handler(&self) -> u32 {
        self.handler
    }

    /// Returns the stack depth to unwind to.
    #[must_use]
    pub fn stack_depth(&self) -> u16 {
        self.stack_depth
    }

    /// Returns true if the given bytecode offset is within this entry's protected range.
    #[must_use]
    pub fn contains(&self, offset: u32) -> bool {
        offset >= self.start && offset < self.end
    }
}
