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
pub(crate) mod errors;
pub(crate) mod input;
pub(crate) mod iso;
pub(crate) mod json_writer;
pub(crate) mod python_input;
pub(crate) mod snapshot;
pub(crate) mod validate;

use crate::{
    builtins::Builtins,
    bytecode::VM,
    exception_private::ExcType,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    types::{BuiltinType, Module, allocate_string},
    value::Value,
};

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
    let version = allocate_string(PYDANTIC_VERSION, vm.heap);
    module.set_attr(StaticStrings::RandomVersion, version, vm);
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}
