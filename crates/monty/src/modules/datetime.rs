//! Implementation of the `datetime` module.
//!
//! This module exposes a minimal phase-1 surface:
//! - `date`
//! - `datetime`
//! - `timedelta`
//! - `timezone`
//!
//! Behavior for constructors, arithmetic, and classmethods is implemented by the
//! corresponding runtime types.

use crate::{
    builtins::Builtins,
    bytecode::VM,
    heap::HeapId,
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Module, Type},
    value::Value,
};

/// Creates the `datetime` module and allocates it on the heap.
///
/// Returns a `HeapId` pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Datetime);

    module.set_attr(StaticStrings::Date, Value::Builtin(Builtins::Type(Type::Date)), vm);
    module.set_attr(
        StaticStrings::Datetime,
        Value::Builtin(Builtins::Type(Type::DateTime)),
        vm,
    );
    module.set_attr(
        StaticStrings::Timedelta,
        Value::Builtin(Builtins::Type(Type::TimeDelta)),
        vm,
    );
    module.set_attr(
        StaticStrings::Timezone,
        Value::Builtin(Builtins::Type(Type::TimeZone)),
        vm,
    );

    vm.heap.allocate(crate::heap::HeapData::Module(module))
}
