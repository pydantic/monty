//! The `ipaddress` module: the four address and network classes, the factory
//! functions, the range helpers and the two exception names.
//!
//! `AddressValueError` and `NetmaskValueError` are `ValueError` itself here,
//! since Monty's exception types are a fixed set (see `limitations/ipaddress.md`).

use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{BuiltinType, Module, PyTrait, ipaddress},
    value::Value,
};

/// The module's functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum IpaddressFunctions {
    IpAddress,
    IpNetwork,
    SummarizeAddressRange,
    CollapseAddresses,
    V4IntToPacked,
    V6IntToPacked,
}

/// Creates the `ipaddress` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Ipaddress, vm.interns);
    for (name, function) in [
        (StaticStrings::IpAddress, IpaddressFunctions::IpAddress),
        (StaticStrings::IpNetwork, IpaddressFunctions::IpNetwork),
        (
            StaticStrings::SummarizeAddressRange,
            IpaddressFunctions::SummarizeAddressRange,
        ),
        (StaticStrings::CollapseAddresses, IpaddressFunctions::CollapseAddresses),
        (StaticStrings::V4IntToPacked, IpaddressFunctions::V4IntToPacked),
        (StaticStrings::V6IntToPacked, IpaddressFunctions::V6IntToPacked),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Ipaddress(function)), vm);
    }
    for (name, class) in [
        (StaticStrings::IPv4Address, BuiltinType::IPv4Address),
        (StaticStrings::IPv6Address, BuiltinType::IPv6Address),
        (StaticStrings::IPv4Network, BuiltinType::IPv4Network),
        (StaticStrings::IPv6Network, BuiltinType::IPv6Network),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::Type(class)), vm);
    }
    for name in [StaticStrings::AddressValueError, StaticStrings::NetmaskValueError] {
        module.set_attr(name, Value::Builtin(Builtins::ExcType(ExcType::ValueError)), vm);
    }
    module.set_attr(StaticStrings::Ipv4length, Value::Int(32), vm);
    module.set_attr(StaticStrings::Ipv6length, Value::Int(128), vm);
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to one of the module's functions.
pub(super) fn call(vm: &mut VM<'_>, function: IpaddressFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        IpaddressFunctions::IpAddress => {
            let AddressArgs { address } = AddressArgs::from_args(args, vm)?;
            defer_drop!(address, vm);
            ipaddress::ip_address(address, vm)
        }
        IpaddressFunctions::IpNetwork => {
            let NetworkArgs { address, strict } = NetworkArgs::from_args(args, vm)?;
            defer_drop!(address, vm);
            defer_drop!(strict, vm);
            let strict = strict.py_bool(vm)?;
            ipaddress::ip_network(address, strict, vm)
        }
        IpaddressFunctions::SummarizeAddressRange => {
            let RangeArgs { first, last } = RangeArgs::from_args(args, vm)?;
            defer_drop!(first, vm);
            defer_drop!(last, vm);
            ipaddress::summarize_address_range(first, last, vm)
        }
        IpaddressFunctions::CollapseAddresses => {
            let CollapseArgs { addresses } = CollapseArgs::from_args(args, vm)?;
            defer_drop!(addresses, vm);
            ipaddress::collapse_addresses(addresses, vm)
        }
        IpaddressFunctions::V4IntToPacked => {
            let V4PackedArgs { address } = V4PackedArgs::from_args(args, vm)?;
            defer_drop!(address, vm);
            ipaddress::int_to_packed(address, false, vm)
        }
        IpaddressFunctions::V6IntToPacked => {
            let V6PackedArgs { address } = V6PackedArgs::from_args(args, vm)?;
            defer_drop!(address, vm);
            ipaddress::int_to_packed(address, true, vm)
        }
    }
}

/// Argument shape for `ip_address(address)`.
#[derive(FromArgs)]
#[from_args(name = "ip_address", style = def)]
struct AddressArgs {
    address: Value,
}

/// Argument shape for `ip_network(address, strict=True)`.
#[derive(FromArgs)]
#[from_args(name = "ip_network", style = def)]
struct NetworkArgs {
    address: Value,
    #[from_args(default = Value::Bool(true))]
    strict: Value,
}

/// Argument shape for `summarize_address_range(first, last)`.
#[derive(FromArgs)]
#[from_args(name = "summarize_address_range", style = def)]
struct RangeArgs {
    first: Value,
    last: Value,
}

/// Argument shape for `collapse_addresses(addresses)`.
#[derive(FromArgs)]
#[from_args(name = "collapse_addresses", style = def)]
struct CollapseArgs {
    addresses: Value,
}

/// Argument shape for `v4_int_to_packed(address)`.
#[derive(FromArgs)]
#[from_args(name = "v4_int_to_packed", style = def)]
struct V4PackedArgs {
    address: Value,
}

/// Argument shape for `v6_int_to_packed(address)`.
#[derive(FromArgs)]
#[from_args(name = "v6_int_to_packed", style = def)]
struct V6PackedArgs {
    address: Value,
}
