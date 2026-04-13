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
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Module, Type},
    value::Value,
};

/// Creates the `pathlib` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Pathlib);

    // pathlib.Path - the Path class (callable to create Path instances)
    module.set_attr(StaticStrings::PathClass, Value::Builtin(Builtins::Type(Type::Path)), vm);

    vm.heap.allocate(HeapData::Module(module))
}
