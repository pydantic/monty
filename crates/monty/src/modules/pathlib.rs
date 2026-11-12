//! Implementation of the `pathlib` module.
//!
//! Provides a minimal implementation of Python's `pathlib` module with:
//! - `Path`: A class for filesystem path operations
//!
//! The `Path` class supports both pure methods (no I/O, handled directly) and
//! filesystem methods (require I/O, yield external function calls for host resolution).

use crate::{
    builtins::Builtins,
    bytecode::VM,
    intern::StaticStrings,
    types::{BuiltinType, Module},
    value::Value,
};

/// Creates the `pathlib` module.
///
pub fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Pathlib, vm.interns);

    // pathlib.Path - the Path class (callable to create Path instances)
    module.set_attr(
        StaticStrings::PathClass,
        Value::Builtin(Builtins::Type(BuiltinType::Path)),
        vm,
    );

    module
}
