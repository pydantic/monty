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
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
    types::{Dict, Module, NamedTuple},
    value::{Marker, Value},
};

/// Creates the `sys` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_sys_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let name = find_string(interns, "sys");

    // Create the attributes dictionary
    let mut attrs = Dict::new();

    // sys.version - Python version string
    // Note: "3.14.0 (Monty)" may not be interned, so we use a pre-interned version string
    let version_key = Value::InternString(find_string(interns, "version"));
    let version_value = create_version_string(heap)?;
    // Unwrap is safe because InternString keys are always hashable
    attrs.set(version_key, version_value, heap, interns).unwrap();

    // sys.version_info - named tuple with (major=3, minor=14, micro=0, releaselevel='final', serial=0)
    let version_info_key = Value::InternString(find_string(interns, "version_info"));
    let version_info = NamedTuple::new(
        "sys.version_info".to_string(),
        vec![
            find_string(interns, "major"),
            find_string(interns, "minor"),
            find_string(interns, "micro"),
            find_string(interns, "releaselevel"),
            find_string(interns, "serial"),
        ],
        vec![
            Value::Int(3),
            Value::Int(14),
            Value::Int(0),
            Value::InternString(find_string(interns, "final")),
            Value::Int(0),
        ],
    );
    let version_info_id = heap.allocate(HeapData::NamedTuple(version_info))?;
    attrs
        .set(version_info_key, Value::Ref(version_info_id), heap, interns)
        .unwrap();

    // sys.platform - "monty"
    let platform_key = Value::InternString(find_string(interns, "platform"));
    let platform_value = create_platform_string(heap)?;
    attrs.set(platform_key, platform_value, heap, interns).unwrap();

    // sys.stdout - marker for stdout
    let stdout_key = Value::InternString(find_string(interns, "stdout"));
    let stdout_value = Value::Marker(Marker::Stdout);
    attrs.set(stdout_key, stdout_value, heap, interns).unwrap();

    // sys.stderr - marker for stderr
    let stderr_key = Value::InternString(find_string(interns, "stderr"));
    let stderr_value = Value::Marker(Marker::Stderr);
    attrs.set(stderr_key, stderr_value, heap, interns).unwrap();

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

/// Creates the version string as a heap-allocated Str.
fn create_version_string(heap: &mut Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    use crate::types::Str;
    let s = Str::new("3.14.0 (Monty)".to_string());
    let id = heap.allocate(HeapData::Str(s))?;
    Ok(Value::Ref(id))
}

/// Creates the platform string as a heap-allocated Str.
fn create_platform_string(heap: &mut Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    use crate::types::Str;
    let s = Str::new("monty".to_string());
    let id = heap.allocate(HeapData::Str(s))?;
    Ok(Value::Ref(id))
}
