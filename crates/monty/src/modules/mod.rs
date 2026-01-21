//! Built-in module implementations.
//!
//! This module provides implementations for Python built-in modules like `sys` and `typing`.
//! These are created on-demand when import statements are executed.

use strum::{EnumString, FromRepr};

use crate::{
    expressions::Expr,
    heap::{Heap, HeapId},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
};

pub(crate) mod sys;
pub(crate) mod typing;

/// Built-in modules that can be imported.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRepr, EnumString)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum BuiltinModule {
    /// The `sys` module providing system-specific parameters and functions.
    Sys,
    /// The `typing` module providing type hints support.
    Typing,
}

impl BuiltinModule {
    /// Creates a new instance of this module on the heap.
    ///
    /// Returns a HeapId pointing to the newly allocated module.
    ///
    /// # Panics
    ///
    /// Panics if the required strings have not been pre-interned during prepare phase.
    pub fn create(self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
        match self {
            Self::Sys => sys::create_module(heap, interns),
            Self::Typing => typing::create_module(heap, interns),
        }
    }

    /// Resolves a `from <module> import X` to an expression value.
    ///
    /// Returns the expression to assign for known names, or `None` if the name
    /// is not found in the module.
    pub fn import_from(self, name: &str) -> Option<Expr> {
        match self {
            Self::Sys => sys::import_from(name),
            Self::Typing => typing::import_from(name),
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
