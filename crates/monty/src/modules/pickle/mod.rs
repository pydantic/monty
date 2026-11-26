//! Pickle serialization for built-in data.

mod decode;
mod encode;
mod opcode;

use std::borrow::Cow;

use super::ModuleFunctions;
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    intern::StaticStrings,
    types::{Module, bytes::allocate_bytes},
    value::Value,
};

const DEFAULT_PROTOCOL: u8 = 5;

#[monty_macros::explicit_discriminants(checksum = 0xdc1c5a6e28a22612)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum PickleFunctions {
    Dumps = 0,
    Loads = 1,
}

pub fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Pickle, vm.interns);
    for (name, function) in [
        (StaticStrings::Dumps, PickleFunctions::Dumps),
        (StaticStrings::Loads, PickleFunctions::Loads),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Pickle(function)), vm);
    }
    for name in [StaticStrings::DefaultProtocol, StaticStrings::HighestProtocol] {
        module.set_attr(name, Value::Int(i64::from(DEFAULT_PROTOCOL)), vm);
    }
    module
}

pub(super) fn call(vm: &mut VM<'_>, function: PickleFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        PickleFunctions::Dumps => dumps(vm, args),
        PickleFunctions::Loads => loads(vm, args),
    }
}

#[derive(FromArgs)]
#[from_args(name = "dumps", style = c_named)]
struct DumpsArgs {
    obj: Value,
    #[from_args(static_string = "ProtocolArg", default = Value::None)]
    protocol: Value,
}

fn dumps(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DumpsArgs { obj, protocol } = DumpsArgs::from_args(args, vm)?;
    defer_drop!(obj, vm);
    defer_drop!(protocol, vm);
    let protocol = if matches!(protocol, Value::None) {
        DEFAULT_PROTOCOL
    } else {
        match protocol.as_int_with_overflow(vm, ExcType::overflow_c_int)? {
            n if i32::try_from(n).is_err() => return Err(ExcType::overflow_c_int()),
            n if n < 0 => DEFAULT_PROTOCOL,
            4 => 4,
            5 => 5,
            _ => return Err(ExcType::value_error("pickle supports protocols 4 and 5")),
        }
    };
    let bytes = encode::encode(obj, protocol, vm)?;
    Ok(allocate_bytes(bytes, vm.heap))
}

#[derive(FromArgs)]
#[from_args(name = "loads", style = c_named)]
struct LoadsArgs {
    #[from_args(pos_only)]
    data: Value,
}

fn loads(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let LoadsArgs { data } = LoadsArgs::from_args(args, vm)?;
    defer_drop!(data, vm);
    let input = match data {
        Value::InternBytes(id) => Cow::Borrowed(vm.interns.get_bytes(*id)),
        Value::Ref(id) if let HeapData::Bytes(bytes) = vm.heap.get(*id) => {
            let bytes = bytes.as_slice();
            vm.heap.tracker.check_allocation(bytes.len())?;
            Cow::Owned(bytes.to_vec())
        }
        _ => {
            return Err(ExcType::type_error(format_args!(
                "a bytes-like object is required, not '{}'",
                data.py_type_name(vm)
            )));
        }
    };
    decode::decode(&input, vm)
}
