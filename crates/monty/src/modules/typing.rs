//! Implementation of the `typing` module.
//!
//! Provides a minimal implementation of Python's `typing` module with:
//! - `TYPE_CHECKING`: Always False (used for conditional imports)
//! - Common type hints as `Marker` values (Any, Optional, List, Dict, etc.)
//! - `Union`, the type behind `int | None` (see `types/union.rs`)
//!
//! These markers exist so code that imports typing constructs works correctly,
//! though Monty doesn't perform static type checking. `Optional[X]` is the one
//! marker that can be subscripted, producing `X | None`.

use crate::{
    builtins::Builtins,
    bytecode::VM,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    types::{Module, Type},
    value::{Marker, Value},
};

/// Creates the `typing` module and allocates it on the heap.
///
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Typing, vm.interns);

    // typing.TYPE_CHECKING - always False
    module.set_attr(StaticStrings::TypeChecking, Value::Bool(false), vm);

    // `typing.Union` is the type of `int | None` (one object with
    // `types.UnionType` since 3.14), so it is a real type rather than a marker.
    module.set_attr(
        StaticStrings::UnionType,
        Value::Builtin(Builtins::Type(Type::Union)),
        vm,
    );

    // Export all typing markers as module attributes
    for ss in MARKER_ATTRS {
        module.set_attr(*ss, Value::Marker(Marker(*ss)), vm);
    }

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Typing marker attributes exported by this module.
///
/// Each marker wraps its corresponding `StaticStrings` variant as both the
/// attribute name and the marker value.
const MARKER_ATTRS: &[StaticStrings] = &[
    StaticStrings::Any,
    StaticStrings::Optional,
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
