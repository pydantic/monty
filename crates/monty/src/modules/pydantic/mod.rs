//! The `pydantic` module: validation of sandbox values against type hints,
//! with pydantic's own error messages.
//!
//! The public surface is `TypeAdapter`, `ValidationError` and the error
//! classes pydantic raises for misuse. Validation runs in Rust against a
//! validator arena built from a type hint (see `types/type_adapter.rs`); the
//! error catalogue in `catalogue.rs` is copied from pydantic-core so messages
//! match it byte for byte.

pub(crate) mod arena;
pub(crate) mod build;
pub(crate) mod catalogue;
pub(crate) mod coerce;
pub(crate) mod config;
pub(crate) mod constraints;
pub(crate) mod dataclasses;
pub(crate) mod errors;
mod field;
pub(crate) mod input;
pub(crate) mod iso;
pub(crate) mod json_input;
pub(crate) mod json_schema;
pub(crate) mod json_writer;
pub(crate) mod python_input;
pub(crate) mod serialize;
pub(crate) mod snapshot;
pub(crate) mod validate;
pub(crate) mod validate_call;

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{BuiltinType, Module, allocate_string},
    value::Value,
};

/// The module's functions, dispatched by [`call`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
pub(crate) enum PydanticFunctions {
    /// `Field(...)`, building a `FieldInfo`.
    Field,
    /// `ConfigDict(...)`, a plain dict.
    ConfigDict,
    /// `pydantic.dataclasses.dataclass`.
    Dataclass,
    /// `pydantic.dataclasses.is_pydantic_dataclass`.
    IsPydanticDataclass,
    /// `validate_call(...)`.
    ValidateCall,
}

/// The pydantic version whose messages, URLs and API the module follows.
pub(crate) const PYDANTIC_VERSION: &str = "2.13.4";

/// Creates the `pydantic` module and allocates it on the heap.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Pydantic, vm.interns);
    for (name, exc_type) in [
        (StaticStrings::ValidationError, ExcType::ValidationError),
        (StaticStrings::PydanticUserError, ExcType::PydanticUserError),
        (
            StaticStrings::PydanticSchemaGenerationError,
            ExcType::PydanticSchemaGenerationError,
        ),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::ExcType(exc_type)), vm);
    }
    module.set_attr(
        StaticStrings::TypeAdapter,
        Value::Builtin(Builtins::Type(BuiltinType::TypeAdapter)),
        vm,
    );
    for (name, function) in [
        (StaticStrings::PydanticField, PydanticFunctions::Field),
        (StaticStrings::ConfigDict, PydanticFunctions::ConfigDict),
        (StaticStrings::ValidateCall, PydanticFunctions::ValidateCall),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Pydantic(function)), vm);
    }
    module.set_attr(
        StaticStrings::Dataclasses,
        Value::Ref(dataclasses::create_module(vm)),
        vm,
    );
    // `VERSION` and `__version__` are the same string, as in pydantic.
    let version = allocate_string(PYDANTIC_VERSION, vm.heap);
    module.set_attr(StaticStrings::DunderVersion, version.clone_with_heap(vm.heap), vm);
    module.set_attr(StaticStrings::RandomVersion, version, vm);
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a `pydantic` module function call.
pub(super) fn call(vm: &mut VM<'_>, function: PydanticFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        PydanticFunctions::Field => field::field(vm, args),
        PydanticFunctions::ConfigDict => field::config_dict(vm, args),
        PydanticFunctions::Dataclass => dataclasses::dataclass(vm, args),
        PydanticFunctions::IsPydanticDataclass => dataclasses::is_pydantic_dataclass(vm, args),
        PydanticFunctions::ValidateCall => validate_call::validate_call(vm, args),
    }
}
