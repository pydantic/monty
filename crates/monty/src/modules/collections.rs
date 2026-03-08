//! Implementation of the `collections` module.
//!
//! This module currently exposes only `namedtuple()`, but it is structured like
//! the other builtin modules so future collection helpers can be added without
//! revisiting import and dispatch plumbing.

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::RunResult,
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Module, NamedTupleFactory},
    value::Value,
};

/// Functions exposed by the `collections` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum CollectionsFunctions {
    /// `collections.namedtuple()` — create a callable factory for namedtuple instances.
    Namedtuple,
}

/// Creates the `collections` module and allocates it on the heap.
///
/// The module currently exports only `namedtuple`, mirroring the runtime surface
/// Monty supports today.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Collections);
    module.set_attr(
        StaticStrings::Namedtuple,
        Value::ModuleFunction(ModuleFunctions::Collections(CollectionsFunctions::Namedtuple)),
        heap,
        interns,
    );
    heap.allocate(HeapData::Module(module))
}

/// Dispatches a `collections` module function call.
pub(super) fn call(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    functions: CollectionsFunctions,
    args: ArgValues,
) -> RunResult<AttrCallResult> {
    match functions {
        CollectionsFunctions::Namedtuple => {
            NamedTupleFactory::create_from_namedtuple_call(vm, args).map(AttrCallResult::Value)
        }
    }
}
