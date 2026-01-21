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
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::Module,
    value::{Marker, Value},
};

/// Creates the `typing` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_typing_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Typing);

    // typing.TYPE_CHECKING - always False
    module.set_attr(StaticStrings::TypeChecking, Value::Bool(false), heap, interns);

    // Export all typing markers as module attributes
    module.set_attr(StaticStrings::Any, Value::Marker(Marker::Any), heap, interns);
    module.set_attr(StaticStrings::Optional, Value::Marker(Marker::Optional), heap, interns);
    module.set_attr(StaticStrings::Union, Value::Marker(Marker::Union), heap, interns);
    module.set_attr(StaticStrings::ListType, Value::Marker(Marker::List), heap, interns);
    module.set_attr(StaticStrings::DictType, Value::Marker(Marker::Dict), heap, interns);
    module.set_attr(StaticStrings::TupleType, Value::Marker(Marker::Tuple), heap, interns);
    module.set_attr(StaticStrings::SetType, Value::Marker(Marker::Set), heap, interns);
    module.set_attr(
        StaticStrings::FrozenSet,
        Value::Marker(Marker::FrozenSet),
        heap,
        interns,
    );
    module.set_attr(StaticStrings::Callable, Value::Marker(Marker::Callable), heap, interns);
    module.set_attr(StaticStrings::Type, Value::Marker(Marker::Type), heap, interns);
    module.set_attr(StaticStrings::Sequence, Value::Marker(Marker::Sequence), heap, interns);
    module.set_attr(StaticStrings::Mapping, Value::Marker(Marker::Mapping), heap, interns);
    module.set_attr(StaticStrings::Iterable, Value::Marker(Marker::Iterable), heap, interns);
    module.set_attr(
        StaticStrings::IteratorType,
        Value::Marker(Marker::Iterator),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Generator,
        Value::Marker(Marker::Generator),
        heap,
        interns,
    );
    module.set_attr(StaticStrings::ClassVar, Value::Marker(Marker::ClassVar), heap, interns);
    // module.set_attr(StaticStrings::Final, Value::Marker(Marker::Final), heap, interns);
    // module.set_attr(StaticStrings::Literal, Value::Marker(Marker::Literal), heap, interns);
    // module.set_attr(StaticStrings::TypeVar, Value::Marker(Marker::TypeVar), heap, interns);
    // module.set_attr(StaticStrings::Generic, Value::Marker(Marker::Generic), heap, interns);
    // module.set_attr(StaticStrings::Protocol, Value::Marker(Marker::Protocol), heap, interns);
    // module.set_attr(
    //     StaticStrings::Annotated,
    //     Value::Marker(Marker::Annotated),
    //     heap,
    //     interns,
    // );
    // module.set_attr(StaticStrings::Self, Value::Marker(Marker::TypeSelf), heap, interns);
    // module.set_attr(StaticStrings::Never, Value::Marker(Marker::Never), heap, interns);
    // module.set_attr(StaticStrings::NoReturn, Value::Marker(Marker::NoReturn), heap, interns);

    heap.allocate(HeapData::Module(module))
}
