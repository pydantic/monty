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

// Allow unused items while the bytecode module is being built out.
// These will be used once the VM is implemented.
#![allow(dead_code, unused_imports)]

mod builder;
mod code;
mod compiler;
mod op;
mod vm;

pub use builder::{CodeBuilder, JumpLabel};
pub use code::{Code, ConstPool, ExceptionEntry, LocationEntry};
pub use compiler::Compiler;
pub use op::Opcode;
pub use vm::{CallFrame, VMResult, VM};
