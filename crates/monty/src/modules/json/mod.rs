//! Implementation of Python's `json` module.
//!
//! This module currently implements the two high-traffic entry points used by
//! most programs:
//! - `json.loads()` for parsing JSON text into Monty values
//! - `json.dumps()` for serializing Monty values to JSON text
//!
//! The implementation is split by direction so parsing and serialization logic
//! stay isolated:
//! - [`load`] handles JSON text -> Monty values
//! - [`dump`] handles Monty values -> JSON text

mod dump;
mod load;
mod string_cache;

pub(crate) use string_cache::JsonStringCache;

use super::ModuleFunctions;
use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::Module,
    value::Value,
};

/// Functions exposed by the `json` module.
///
/// Each variant corresponds to a module-level callable stored in the module's
/// attribute dictionary. The display form matches the Python-visible function
/// name so reprs look like normal builtins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum JsonFunctions {
    /// `json.loads()` parses JSON text into Monty values.
    Loads,
    /// `json.dumps()` serializes Monty values into JSON text.
    Dumps,
}

/// Creates the `json` module and allocates it on the heap.
///
/// The module exposes `loads`, `dumps`, and `JSONDecodeError`. These are the
/// most widely used parts of CPython's `json` module and are sufficient for
/// common data interchange and round-tripping use cases inside the sandbox.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Json);
    module.set_attr(
        StaticStrings::Loads,
        Value::ModuleFunction(ModuleFunctions::Json(JsonFunctions::Loads)),
        vm,
    );
    module.set_attr(
        StaticStrings::Dumps,
        Value::ModuleFunction(ModuleFunctions::Json(JsonFunctions::Dumps)),
        vm,
    );
    module.set_attr(
        StaticStrings::JsonDecodeError,
        Value::Builtin(Builtins::ExcType(ExcType::JsonDecodeError)),
        vm,
    );
    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a `json` module function call.
///
/// Both functions are pure computations that return ordinary Monty values and
/// never need host involvement, so the dispatcher returns `Value` directly.
pub(super) fn call(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    function: JsonFunctions,
    args: ArgValues,
) -> RunResult<Value> {
    match function {
        JsonFunctions::Loads => load::call_loads(vm, args),
        JsonFunctions::Dumps => dump::call_dumps(vm, args),
    }
}
