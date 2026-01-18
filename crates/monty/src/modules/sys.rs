//! Implementation of the `sys` module.
//!
//! Provides a minimal implementation of Python's `sys` module with:
//! - `version`: Python version string (e.g., "3.14.0 (Monty)")
//! - `version_info`: Named tuple (3, 14, 0, 'final', 0)
//! - `platform`: Platform identifier ("monty")
//! - `stdout`: Marker for standard output (no real functionality)
//! - `stderr`: Marker for standard error (no real functionality)

use crate::{
    heap::{Heap, HeapData, HeapId},
    intern::{InternerBuilder, Interns},
    resource::{ResourceError, ResourceTracker},
    types::{Module, NamedTuple},
    value::{Marker, Value},
};

/// Pre-interns all strings needed by the sys module.
///
/// Called during `InternerBuilder::build_base` to ensure all sys module
/// strings are always available without needing to check for imports.
pub(crate) fn intern_module_strings(interner: &mut InternerBuilder) {
    // Module name and attributes
    interner.intern("sys");
    interner.intern("version");
    interner.intern("version_info");
    interner.intern("platform");
    interner.intern("stdout");
    interner.intern("stderr");
    // sys.version_info field names (for NamedTuple attribute access)
    interner.intern("major");
    interner.intern("minor");
    interner.intern("micro");
    interner.intern("releaselevel");
    interner.intern("serial");
    interner.intern("final");
    interner.intern("3.14.0 (Monty)");
    interner.intern("monty");
}

/// Creates the `sys` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_sys_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new("sys", interns);

    // sys.version - Python version string
    let version_str_id = interns.find_known_string_id("3.14.0 (Monty)");
    module.set_attr("version", Value::InternString(version_str_id), heap, interns);

    // sys.version_info - named tuple with (major=3, minor=14, micro=0, releaselevel='final', serial=0)
    let version_info = NamedTuple::new(
        "sys.version_info".to_string(),
        vec![
            interns.find_known_string_id("major"),
            interns.find_known_string_id("minor"),
            interns.find_known_string_id("micro"),
            interns.find_known_string_id("releaselevel"),
            interns.find_known_string_id("serial"),
        ],
        vec![
            Value::Int(3),
            Value::Int(14),
            Value::Int(0),
            Value::InternString(interns.find_known_string_id("final")),
            Value::Int(0),
        ],
    );
    let version_info_id = heap.allocate(HeapData::NamedTuple(version_info))?;
    module.set_attr("version_info", Value::Ref(version_info_id), heap, interns);

    // sys.platform - "monty"
    let platform_str_id = interns.find_known_string_id("monty");
    module.set_attr("platform", Value::InternString(platform_str_id), heap, interns);

    // sys.stdout - marker for stdout
    module.set_attr("stdout", Value::Marker(Marker::Stdout), heap, interns);

    // sys.stderr - marker for stderr
    module.set_attr("stderr", Value::Marker(Marker::Stderr), heap, interns);

    heap.allocate(HeapData::Module(module))
}
