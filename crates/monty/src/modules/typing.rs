//! Implementation of the `typing` module.
//!
//! Provides a minimal implementation of Python's `typing` module with:
//! - `TYPE_CHECKING`: Always False (used for conditional imports)
//! - Common type hints as `Marker` values (Any, Optional, List, Dict, etc.)
//!
//! These markers exist so code that imports typing constructs works correctly,
//! though Monty doesn't perform static type checking.

use crate::{
    expressions::{Expr, Literal},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::Module,
    value::{Marker, Value},
};

/// Typing marker attributes exported by this module.
///
/// Each marker wraps its corresponding `StaticStrings` variant as both the
/// attribute name and the marker value.
const MARKER_ATTRS: &[StaticStrings] = &[
    StaticStrings::Any,
    StaticStrings::Optional,
    StaticStrings::UnionType,
    StaticStrings::ListType,
    StaticStrings::DictType,
    StaticStrings::TupleType,
    StaticStrings::SetType,
    StaticStrings::FrozenSet,
    StaticStrings::Callable,
    StaticStrings::Type,
    StaticStrings::Sequence,
    StaticStrings::Mapping,
    StaticStrings::Iterable,
    StaticStrings::IteratorType,
    StaticStrings::Generator,
    StaticStrings::ClassVar,
    StaticStrings::FinalType,
    StaticStrings::Literal,
    StaticStrings::TypeVar,
    StaticStrings::Generic,
    StaticStrings::Protocol,
    StaticStrings::Annotated,
    StaticStrings::SelfType,
    StaticStrings::Never,
    StaticStrings::NoReturn,
];

/// Resolves a `from typing import X` to an expression value.
///
/// Returns the expression to assign for known names:
/// - `TYPE_CHECKING` → `False`
/// - Known type hints (Any, Optional, etc.) → `Marker` values
/// - Unknown names → `None`
pub fn import_from(name: &str) -> Option<Expr> {
    if name == <StaticStrings as Into<&str>>::into(StaticStrings::TypeChecking) {
        Some(Expr::Literal(Literal::Bool(false)))
    } else {
        MARKER_ATTRS
            .iter()
            .find(|ss| <StaticStrings as Into<&str>>::into(**ss) == name)
            .map(|ss| Expr::Literal(Literal::Marker(Marker(*ss))))
    }
}

/// Creates the `typing` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Typing);

    // typing.TYPE_CHECKING - always False
    module.set_attr(StaticStrings::TypeChecking, Value::Bool(false), heap, interns);

    // Export all typing markers as module attributes
    for ss in MARKER_ATTRS {
        module.set_attr(*ss, Value::Marker(Marker(*ss)), heap, interns);
    }

    heap.allocate(HeapData::Module(module))
}
