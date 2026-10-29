//! The `zoneinfo` module: the `ZoneInfo` class, `available_timezones()` and the
//! `ZoneInfoNotFoundError` name, which is `KeyError` here (see `limitations/zoneinfo.md`).

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{BuiltinType, Module, zoneinfo},
    value::Value,
};

/// The module's functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ZoneinfoFunctions {
    AvailableTimezones,
}

/// Creates the `zoneinfo` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Zoneinfo, vm.interns);
    module.set_attr(
        StaticStrings::ZoneInfo,
        Value::Builtin(Builtins::Type(BuiltinType::ZoneInfo)),
        vm,
    );
    module.set_attr(
        StaticStrings::AvailableTimezones,
        Value::ModuleFunction(ModuleFunctions::Zoneinfo(ZoneinfoFunctions::AvailableTimezones)),
        vm,
    );
    module.set_attr(
        StaticStrings::ZoneInfoNotFoundError,
        Value::Builtin(Builtins::ExcType(ExcType::KeyError)),
        vm,
    );
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to one of the module's functions.
pub(super) fn call(vm: &mut VM<'_>, function: ZoneinfoFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        ZoneinfoFunctions::AvailableTimezones => {
            args.check_zero_args("available_timezones", vm.heap)?;
            zoneinfo::available_timezones(vm)
        }
    }
}
