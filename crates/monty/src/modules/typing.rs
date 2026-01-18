//! Implementation of the `typing` module.
//!
//! Provides a minimal implementation of Python's `typing` module with:
//! - `TYPE_CHECKING`: Always False (used for conditional imports)
//!
//! Other typing imports (List, Dict, Optional, etc.) are handled specially
//! at the prepare phase - they are silently ignored since Monty doesn't
//! perform static type checking.

use crate::{
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
    types::{Dict, Module},
    value::Value,
};

/// Creates the `typing` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_typing_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let name = find_string(interns, "typing");

    // Create the attributes dictionary
    let mut attrs = Dict::new();

    // typing.TYPE_CHECKING - always False
    let type_checking_key = Value::InternString(find_string(interns, "TYPE_CHECKING"));
    let type_checking_value = Value::Bool(false);
    // Unwrap is safe because InternString keys are always hashable
    attrs
        .set(type_checking_key, type_checking_value, heap, interns)
        .unwrap();

    // Create and allocate the module
    let module = Module::new(name, attrs);
    heap.allocate(HeapData::Module(module))
}

/// Finds a pre-interned string, panicking if not found.
fn find_string(interns: &Interns, s: &str) -> crate::intern::StringId {
    interns
        .find_string_id(s)
        .unwrap_or_else(|| panic!("string '{s}' not pre-interned during prepare phase"))
}
