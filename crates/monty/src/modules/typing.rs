//! Implementation of the `typing` module.
//!
//! Provides a minimal implementation of Python's `typing` module with:
//! - `TYPE_CHECKING`: Always False (used for conditional imports)
//! - Common type hints as `Marker` values (Any, Optional, List, Dict, etc.)
//!
//! These markers exist so code that imports typing constructs works correctly,
//! though Monty doesn't perform static type checking.

use crate::{
    heap::{Heap, HeapData, HeapId},
    intern::{InternerBuilder, Interns},
    resource::{ResourceError, ResourceTracker},
    types::Module,
    value::{Marker, Value},
};

/// Pre-interns all strings needed by the typing module.
///
/// Called during `InternerBuilder::build_base` to ensure all typing module
/// strings are always available without needing to check for imports.
pub(crate) fn intern_module_strings(interner: &mut InternerBuilder) {
    interner.intern("typing");
    interner.intern("TYPE_CHECKING");
    // Common type hint markers
    interner.intern("Any");
    interner.intern("Optional");
    interner.intern("Union");
    interner.intern("List");
    interner.intern("Dict");
    interner.intern("Tuple");
    interner.intern("Set");
    interner.intern("FrozenSet");
    interner.intern("Callable");
    interner.intern("Type");
    interner.intern("Sequence");
    interner.intern("Mapping");
    interner.intern("Iterable");
    interner.intern("Iterator");
    interner.intern("Generator");
    interner.intern("ClassVar");
    interner.intern("Final");
    interner.intern("Literal");
    interner.intern("TypeVar");
    interner.intern("Generic");
    interner.intern("Protocol");
    interner.intern("Annotated");
    interner.intern("Self");
    interner.intern("Never");
    interner.intern("NoReturn");
}

/// Creates the `typing` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_typing_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new("typing", interns);

    // typing.TYPE_CHECKING - always False
    module.set_attr("TYPE_CHECKING", Value::Bool(false), heap, interns);

    // Export all typing markers as module attributes
    module.set_attr("Any", Value::Marker(Marker::Any), heap, interns);
    module.set_attr("Optional", Value::Marker(Marker::Optional), heap, interns);
    module.set_attr("Union", Value::Marker(Marker::Union), heap, interns);
    module.set_attr("List", Value::Marker(Marker::List), heap, interns);
    module.set_attr("Dict", Value::Marker(Marker::Dict), heap, interns);
    module.set_attr("Tuple", Value::Marker(Marker::Tuple), heap, interns);
    module.set_attr("Set", Value::Marker(Marker::Set), heap, interns);
    module.set_attr("FrozenSet", Value::Marker(Marker::FrozenSet), heap, interns);
    module.set_attr("Callable", Value::Marker(Marker::Callable), heap, interns);
    module.set_attr("Type", Value::Marker(Marker::Type), heap, interns);
    module.set_attr("Sequence", Value::Marker(Marker::Sequence), heap, interns);
    module.set_attr("Mapping", Value::Marker(Marker::Mapping), heap, interns);
    module.set_attr("Iterable", Value::Marker(Marker::Iterable), heap, interns);
    module.set_attr("Iterator", Value::Marker(Marker::Iterator), heap, interns);
    module.set_attr("Generator", Value::Marker(Marker::Generator), heap, interns);
    module.set_attr("ClassVar", Value::Marker(Marker::ClassVar), heap, interns);
    module.set_attr("Final", Value::Marker(Marker::Final), heap, interns);
    module.set_attr("Literal", Value::Marker(Marker::Literal), heap, interns);
    module.set_attr("TypeVar", Value::Marker(Marker::TypeVar), heap, interns);
    module.set_attr("Generic", Value::Marker(Marker::Generic), heap, interns);
    module.set_attr("Protocol", Value::Marker(Marker::Protocol), heap, interns);
    module.set_attr("Annotated", Value::Marker(Marker::Annotated), heap, interns);
    module.set_attr("Self", Value::Marker(Marker::TypeSelf), heap, interns);
    module.set_attr("Never", Value::Marker(Marker::Never), heap, interns);
    module.set_attr("NoReturn", Value::Marker(Marker::NoReturn), heap, interns);

    heap.allocate(HeapData::Module(module))
}
