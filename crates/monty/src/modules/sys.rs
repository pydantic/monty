//! Implementation of the `sys` module.
//!
//! Provides a minimal implementation of Python's `sys` module with:
//! - `version`: Python version string (e.g., "3.14.0 (Monty)")
//! - `version_info`: Named tuple (3, 14, 0, 'final', 0)
//! - `platform`: Platform identifier ("monty")
//! - `stdout`: Marker for standard output (no real functionality)
//! - `stderr`: Marker for standard error (no real functionality)

use crate::{
    expressions::{Expr, Literal},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::{Module, NamedTuple},
    value::{Marker, Value},
};

/// Resolves a `from sys import X` to an expression value.
///
/// Returns the expression to assign for known names:
/// - `version` → Version string
/// - `platform` → "monty"
/// - `stdout` → Marker for stdout
/// - `stderr` → Marker for stderr
/// - `version_info` → `None` (not supported, requires heap allocation)
/// - Unknown names → `None`
pub fn import_from(name: &str) -> Option<Expr> {
    simple_attrs()
        .find(|(key, _)| <StaticStrings as Into<&str>>::into(*key) == name)
        .map(|(_, value)| value_to_expr(&value))
}

/// Creates the `sys` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Sys);

    // Add all simple attributes
    for (attr_name, value) in simple_attrs() {
        module.set_attr(attr_name, value, heap, interns);
    }

    // sys.version_info - named tuple with (major=3, minor=14, micro=0, releaselevel='final', serial=0)
    // This requires heap allocation so cannot be in simple_attrs()
    let version_info = NamedTuple::new(
        StaticStrings::SysVersionInfo.as_string_id(),
        vec![
            StaticStrings::Major.as_string_id(),
            StaticStrings::Minor.as_string_id(),
            StaticStrings::Micro.as_string_id(),
            StaticStrings::Releaselevel.as_string_id(),
            StaticStrings::Serial.as_string_id(),
        ],
        vec![
            Value::Int(3),
            Value::Int(14),
            Value::Int(0),
            Value::InternString(StaticStrings::Final.as_string_id()),
            Value::Int(0),
        ],
    );
    let version_info_id = heap.allocate(HeapData::NamedTuple(version_info))?;
    module.set_attr(
        StaticStrings::VersionInfo.as_string_id(),
        Value::Ref(version_info_id),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}

/// Returns all simple sys attributes as (key, value) pairs.
///
/// Simple attributes are those that can be represented without heap allocation.
/// This is used by both `create_module` and `import_from` to avoid duplication.
fn simple_attrs() -> impl Iterator<Item = (StaticStrings, Value)> {
    [
        (
            StaticStrings::Version,
            Value::InternString(StaticStrings::MontyVersionString.as_string_id()),
        ),
        (StaticStrings::Platform, StaticStrings::Monty.into()),
        (StaticStrings::Stdout, Value::Marker(Marker(StaticStrings::Stdout))),
        (StaticStrings::Stderr, Value::Marker(Marker(StaticStrings::Stderr))),
    ]
    .into_iter()
}

/// Converts a simple `Value` to its compile-time `Expr` representation.
///
/// Only handles value types that don't require heap allocation.
fn value_to_expr(value: &Value) -> Expr {
    match value {
        Value::Bool(b) => Expr::Literal(Literal::Bool(*b)),
        Value::Int(i) => Expr::Literal(Literal::Int(*i)),
        Value::Float(f) => Expr::Literal(Literal::Float(*f)),
        Value::InternString(id) => Expr::Literal(Literal::Str(*id)),
        Value::Marker(m) => Expr::Literal(Literal::Marker(*m)),
        Value::None => Expr::Literal(Literal::None),
        _ => panic!("Cannot convert heap-allocated Value to Expr"),
    }
}
