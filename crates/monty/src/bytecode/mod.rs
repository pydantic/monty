//! Bytecode VM module for Monty.
//!
//! This module contains the bytecode representation, compiler, and virtual machine
//! for executing Python code. The bytecode VM replaces the tree-walking interpreter
//! with a stack-based execution model.
//!
//! # Module Structure
//!
//! - `op` - Opcode enum definitions
//! - `code` - Code object containing bytecode and metadata
//! - `builder` - CodeBuilder for emitting bytecode during compilation
//! - `compiler` - AST to bytecode compiler
//! - `vm` - Virtual machine for bytecode execution

mod builder;
mod code;
mod compiler;
mod op;
mod vm;

pub use code::Code;
pub use compiler::Compiler;
pub(crate) use vm::CallResult;
pub use vm::{FrameExit, VM, VMSnapshot};

/// Module-level dunder names Monty exposes with fixed values for CPython
/// compatibility (e.g. so `if __name__ == '__main__':` works).
///
/// Monty has no module object or `globals()` dict, so these are not real
/// namespace entries: they are resolved on *read* when the global slot is
/// `Undefined` (see `VM::module_dunder`, whose match must stay in sync with
/// this list) and are read-only — assigning one at module or global scope is
/// rejected at compile time with a `NotImplementedError` (see
/// `Compiler::compile_store`) rather than being silently ignored.
/// Function-local variables that happen to share these names are unaffected.
pub(crate) const RESERVED_MODULE_DUNDERS: [&str; 6] = [
    "__name__",
    "__debug__",
    "__doc__",
    "__annotations__",
    "__spec__",
    "__package__",
];
