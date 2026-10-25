//! Implementation of Python's `json` module.
//!
//! This module implements the four entry points used by most programs:
//! - `json.loads()` / `json.load()` for parsing JSON text into Monty values
//! - `json.dumps()` / `json.dump()` for serializing Monty values to JSON text
//!
//! The implementation is split by direction so parsing and serialization logic
//! stay isolated:
//! - [`load`] handles JSON text -> Monty values
//! - [`dump`] handles Monty values -> JSON text
//! - [`fp`] reaches the file object the `load` / `dump` variants take

mod dump;
mod fp;
mod load;
mod string_cache;

pub(crate) use load::apply_json_load;
pub(crate) use string_cache::JsonStringCache;

use super::ModuleFunctions;
use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
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
    /// `json.load()` parses the rest of a file object as JSON.
    Load,
    /// `json.dump()` serializes a Monty value into a file object.
    Dump,
}

/// Creates the `json` module and allocates it on the heap.
///
/// The module exposes `loads`, `dumps`, `load`, `dump` and `JSONDecodeError`.
/// These are the most widely used parts of CPython's `json` module and are
/// sufficient for common data interchange and round-tripping use cases inside
/// the sandbox.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Json, vm.interns);
    for (name, function) in [
        (StaticStrings::Loads, JsonFunctions::Loads),
        (StaticStrings::Dumps, JsonFunctions::Dumps),
        (StaticStrings::Load, JsonFunctions::Load),
        (StaticStrings::Dump, JsonFunctions::Dump),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Json(function)), vm);
    }
    module.set_attr(
        StaticStrings::JsonDecodeError,
        Value::Builtin(Builtins::ExcType(ExcType::JsonDecodeError)),
        vm,
    );
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a `json` module function call.
///
/// `loads` and `dumps` are pure computations; `load` and `dump` on an unread
/// or written `open()` handle suspend to the host like `fp.read()` / `fp.write()`.
pub(super) fn call(vm: &mut VM<'_>, function: JsonFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        JsonFunctions::Loads => load::call_loads(vm, args).map(CallResult::Value),
        JsonFunctions::Dumps => dump::call_dumps(vm, args).map(CallResult::Value),
        JsonFunctions::Load => load::call_load(vm, args),
        JsonFunctions::Dump => dump::call_dump(vm, args),
    }
}
