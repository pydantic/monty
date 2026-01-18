//! Built-in module implementations.
//!
//! This module provides implementations for Python built-in modules like `sys` and `typing`.
//! These are created on-demand when import statements are executed.

mod sys;
mod typing;

use crate::{
    heap::{Heap, HeapId},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
};

/// Built-in modules that can be imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinModule {
    /// The `sys` module providing system-specific parameters and functions.
    Sys,
    /// The `typing` module providing type hints support.
    Typing,
}

impl BuiltinModule {
    /// Converts a module ID byte to a `BuiltinModule`.
    ///
    /// Returns `None` for invalid IDs.
    pub fn from_u8(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Sys),
            1 => Some(Self::Typing),
            _ => None,
        }
    }

    /// Converts a `BuiltinModule` to its module ID byte.
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Sys => 0,
            Self::Typing => 1,
        }
    }

    /// Parses a module name string into a `BuiltinModule`.
    ///
    /// Returns `None` if the module name is not recognized.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "sys" => Some(Self::Sys),
            "typing" => Some(Self::Typing),
            _ => None,
        }
    }

    /// Creates a new instance of this module on the heap.
    ///
    /// Returns a HeapId pointing to the newly allocated module.
    ///
    /// # Panics
    ///
    /// Panics if the required strings have not been pre-interned during prepare phase.
    pub fn create(self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
        match self {
            Self::Sys => sys::create_sys_module(heap, interns),
            Self::Typing => typing::create_typing_module(heap, interns),
        }
    }
}

/// Creates a built-in module and returns its HeapId.
///
/// This is a convenience function for the VM to use.
pub fn create_builtin_module(
    module: BuiltinModule,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> Result<HeapId, ResourceError> {
    module.create(heap, interns)
}
