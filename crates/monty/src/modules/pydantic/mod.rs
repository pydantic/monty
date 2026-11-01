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
pub(crate) mod json_input;
pub(crate) mod json_writer;
pub(crate) mod python_input;
pub(crate) mod serialize;
pub(crate) mod snapshot;
pub(crate) mod validate;

use crate::{
    builtins::Builtins,
    bytecode::VM,
    intern::StaticStrings,
    types::{Module, Type, allocate_string},
    value::Value,
};

/// The pydantic version whose messages, URLs and API the module follows.
pub(crate) const PYDANTIC_VERSION: &str = "2.13.4";

/// Creates the `pydantic` module.
pub fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Pydantic, vm.interns);
    for (name, ty) in [
        (StaticStrings::ValidationError, Type::ValidationError),
        (StaticStrings::PydanticUserError, Type::PydanticUserError),
        (
            StaticStrings::PydanticSchemaGenerationError,
            Type::PydanticSchemaGenerationError,
        ),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::Type(ty)), vm);
    }
    module.set_attr(
        StaticStrings::TypeAdapter,
        Value::Builtin(Builtins::Type(Type::TypeAdapter)),
        vm,
    );
    // `VERSION` and `__version__` are the same string, as in pydantic.
    let version = allocate_string(PYDANTIC_VERSION, vm.heap);
    module.set_attr(StaticStrings::DunderVersion, version.clone_with_heap(vm.heap), vm);
    module.set_attr(StaticStrings::RandomVersion, version, vm);
    module
}
