//! `ipaddress.IPv4Address`, `IPv6Address`, `IPv4Network` and `IPv6Network`, plus
//! the iterator behind `hosts()`, `subnets()` and iterating a network.
//!
//! One Rust type serves both versions of each class: an address is its version
//! and its bits, a network its version, network bits and prefix length. The
//! algorithms follow CPython's `Lib/ipaddress.py`; [`parse`] holds the text
//! handling and the classification tables.

pub(crate) mod parse;

use std::{
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::{self, Display, Formatter, Write},
    hash::{Hash, Hasher},
};

use num_bigint::BigInt;
use smallvec::smallvec;

use self::parse::ParseError;
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::HashValue,
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    py_value::PyValue,
    types::{
        CmpOrder, LazyHeapSet, List, LongInt, PyTrait, Type, bytes::allocate_bytes, str::allocate_string,
        tuple::allocate_tuple,
    },
    value::{EitherStr, Value},
};

/// 128 bits stored as two halves, since the dump format has no `u128`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
struct Bits {
    hi: u64,
    lo: u64,
}

impl From<u128> for Bits {
    fn from(bits: u128) -> Self {
        Self {
            hi: u64::try_from(bits >> 64).expect("shifted to 64 bits"),
            lo: u64::try_from(bits & u128::from(u64::MAX)).expect("masked to 64 bits"),
        }
    }
}

impl From<Bits> for u128 {
    fn from(bits: Bits) -> Self {
        Self::from(bits.hi) << 64 | Self::from(bits.lo)
    }
}

/// An `IPv4Address` (`v6 == false`) or `IPv6Address`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct IpAddress {
    v6: bool,
    bits: Bits,
    /// The `%scope` of an IPv6 literal, kept verbatim.
    scope_id: Option<String>,
}

/// An `IPv4Network` (`v6 == false`) or `IPv6Network`: the network address and prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct IpNetwork {
    v6: bool,
    network: Bits,
    prefixlen: u32,
}

/// The lazy iterator CPython's generators become: addresses (`hosts()`, iterating
/// a network) or subnets, `remaining` items from `next` in steps of `step`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct IpIterator {
    v6: bool,
    next: Bits,
    remaining: Bits,
    step: Bits,
    /// `Some(prefixlen)` yields networks of that length, `None` yields addresses.
    prefixlen: Option<u32>,
}

impl HeapItem for IpAddress {
    fn py_dec_ref_ids(&mut self, _: &mut Vec<HeapId>) {}
}

impl HeapItem for IpNetwork {
    fn py_dec_ref_ids(&mut self, _: &mut Vec<HeapId>) {}
}

impl HeapItem for IpIterator {
    fn py_dec_ref_ids(&mut self, _: &mut Vec<HeapId>) {}
}

// ============================================================================
// Constructors and module functions
// ============================================================================

/// `IPv4Address(address)` / `IPv6Address(address)`.
pub(crate) fn init_address(vm: &mut VM<'_>, args: ArgValues, v6: bool) -> RunResult<Value> {
    let address = if v6 {
        V6AddressArgs::from_args(args, vm)?.address
    } else {
        V4AddressArgs::from_args(args, vm)?.address
    };
    defer_drop!(address, vm);
    let parsed = IpAddress::from_value(address, v6, vm)?.map_err(parse_error)?;
    Ok(parsed.allocate(vm))
}

/// `IPv4Network(address, strict=True)` / `IPv6Network(address, strict=True)`.
pub(crate) fn init_network(vm: &mut VM<'_>, args: ArgValues, v6: bool) -> RunResult<Value> {
    let NetworkArgs { address, strict } = if v6 {
        V6NetworkArgs::from_args(args, vm)?.into()
    } else {
        V4NetworkArgs::from_args(args, vm)?.into()
    };
    defer_drop!(address, vm);
    defer_drop!(strict, vm);
    let strict = strict.py_bool(vm)?;
    let network = IpNetwork::from_value(address, strict, v6, vm)?.map_err(parse_error)?;
    Ok(network.allocate(vm))
}

/// `ip_address(address)`: an IPv4 address if the text parses as one, else IPv6;
/// parse errors of both become one `ValueError`.
pub(crate) fn ip_address(address: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    if let Ok(parsed) = IpAddress::from_value(address, false, vm)? {
        return Ok(parsed.allocate(vm));
    }
    if let Ok(parsed) = IpAddress::from_value(address, true, vm)? {
        return Ok(parsed.allocate(vm));
    }
    Err(ExcType::value_error(format!(
        "{} does not appear to be an IPv4 or IPv6 address",
        repr_of(address, vm)?
    )))
}

/// `ip_network(address, strict=True)`, like [`ip_address`]; a strict network with
/// host bits set raises its own error rather than falling through to IPv6.
pub(crate) fn ip_network(address: &Value, strict: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    if let Ok(network) = IpNetwork::from_value(address, strict, false, vm)? {
        return Ok(network.allocate(vm));
    }
    if let Ok(network) = IpNetwork::from_value(address, strict, true, vm)? {
        return Ok(network.allocate(vm));
    }
    Err(ExcType::value_error(format!(
        "{} does not appear to be an IPv4 or IPv6 network",
        repr_of(address, vm)?
    )))
}

/// `v4_int_to_packed(address)` / `v6_int_to_packed(address)`: the big-endian bytes of an int.
pub(crate) fn int_to_packed(address: &Value, v6: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let Some(value) = value_to_bigint(address, vm) else {
        return Err(ExcType::attribute_error(address.py_type_name(vm), "to_bytes"));
    };
    let version = if v6 { 6 } else { 4 };
    let bits = u128::try_from(&value)
        .ok()
        .filter(|bits| *bits <= parse::all_ones(v6))
        .ok_or_else(|| ExcType::value_error(format!("Address negative or too large for IPv{version}")))?;
    Ok(allocate_bytes(packed_bytes(v6, bits), vm.heap))
}

/// `summarize_address_range(first, last)`: the fewest networks covering the range,
/// as an iterator.
pub(crate) fn summarize_address_range(first: &Value, last: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let (Some(first), Some(last)) = (IpAddress::read(first, vm), IpAddress::read(last, vm)) else {
        return Err(ExcType::type_error("first and last must be IP addresses, not networks"));
    };
    if first.v6 != last.v6 {
        return Err(not_same_version(&first.to_string(), &last.to_string()));
    }
    if first.bits() > last.bits() {
        return Err(ExcType::value_error("last IP address must be greater than first"));
    }
    let networks: Vec<Value> = summarize(first.v6, first.bits(), last.bits())
        .into_iter()
        .map(|network| network.allocate(vm))
        .collect();
    list_iterator(networks, vm)
}

/// `collapse_addresses(addresses)`: merges addresses and networks into the fewest
/// networks, sorted, as an iterator.
pub(crate) fn collapse_addresses(addresses: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let iter = addresses
        .clone_with_heap(vm)
        .into_py_iter(vm)?
        .require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);
    let mut ips: Vec<IpAddress> = Vec::new();
    let mut nets: Vec<IpNetwork> = Vec::new();
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if let Some(address) = IpAddress::read(item, vm) {
            if let Some(previous) = ips.last()
                && previous.v6 != address.v6
            {
                return Err(not_same_version(&address.to_string(), &previous.to_string()));
            }
            ips.push(address);
        } else if let Some(network) = IpNetwork::read(item, vm) {
            if network.prefixlen == parse::max_prefixlen(network.v6) {
                let address = network.network_address();
                if let Some(previous) = ips.last()
                    && previous.v6 != address.v6
                {
                    return Err(not_same_version(&network.to_string(), &previous.to_string()));
                }
                ips.push(address);
            } else {
                if let Some(previous) = nets.last()
                    && previous.v6 != network.v6
                {
                    return Err(not_same_version(&network.to_string(), &previous.to_string()));
                }
                nets.push(network);
            }
        } else {
            return Err(ExcType::attribute_error(item.py_type_name(vm), "_prefixlen"));
        }
    }
    // Adjacent addresses summarize into networks before the networks merge.
    ips.sort_by_key(IpAddress::bits);
    ips.dedup_by_key(|address| address.bits());
    let mut all = nets;
    let mut run_start = 0;
    for i in 1..=ips.len() {
        if i == ips.len() || ips[i].bits() != ips[i - 1].bits() + 1 {
            if let (Some(first), Some(last)) = (ips.get(run_start), ips.get(i - 1)) {
                all.extend(summarize(first.v6, first.bits(), last.bits()));
            }
            run_start = i;
        }
    }
    let collapsed: Vec<Value> = collapse(all).into_iter().map(|network| network.allocate(vm)).collect();
    list_iterator(collapsed, vm)
}

/// The networks covering `first..=last`, as `summarize_address_range` yields them.
fn summarize(v6: bool, first: u128, last: u128) -> Vec<IpNetwork> {
    let width = parse::max_prefixlen(v6);
    let mut networks = Vec::new();
    let mut current = first;
    while current <= last {
        let span = last - current + 1;
        let righthand_zeros = if current == 0 {
            width
        } else {
            current.trailing_zeros().min(width)
        };
        let nbits = righthand_zeros.min(128 - span.leading_zeros() - 1);
        networks.push(IpNetwork::new(v6, current, width - nbits));
        match current.checked_add(1u128 << nbits) {
            Some(next) if next - 1 != parse::all_ones(v6) => current = next,
            _ => break,
        }
    }
    networks
}

/// CPython's `_collapse_addresses_internal`: merge sibling networks into their
/// supernet until none remain, then drop networks another one covers.
fn collapse(mut to_merge: Vec<IpNetwork>) -> Vec<IpNetwork> {
    let mut subnets: Vec<(IpNetwork, IpNetwork)> = Vec::new();
    while let Some(net) = to_merge.pop() {
        let supernet = net.supernet_by(1);
        match subnets.iter().position(|(key, _)| *key == supernet) {
            None => subnets.push((supernet, net)),
            Some(index) => {
                if subnets[index].1 != net {
                    subnets.remove(index);
                    to_merge.push(supernet);
                }
            }
        }
    }
    let mut merged: Vec<IpNetwork> = subnets.into_iter().map(|(_, net)| net).collect();
    merged.sort();
    let mut result: Vec<IpNetwork> = Vec::new();
    for net in merged {
        if result.last().is_some_and(|last| last.broadcast() >= net.broadcast()) {
            continue;
        }
        result.push(net);
    }
    result
}

/// Separate derives keep each class's own name in binding errors.
macro_rules! address_arguments {
    ($ty:ident, $name:literal) => {
        /// Argument shape for `(address)`.
        #[derive(FromArgs)]
        #[from_args(name = $name, style = def)]
        struct $ty {
            address: Value,
        }
    };
}
address_arguments!(V4AddressArgs, "IPv4Address.__init__");
address_arguments!(V6AddressArgs, "IPv6Address.__init__");

/// The bound `(address, strict=True)` of a network constructor.
struct NetworkArgs {
    address: Value,
    strict: Value,
}

/// Separate derives keep each class's own name in binding errors.
macro_rules! network_arguments {
    ($ty:ident, $name:literal) => {
        /// Argument shape for `(address, strict=True)`.
        #[derive(FromArgs)]
        #[from_args(name = $name, style = def)]
        struct $ty {
            address: Value,
            #[from_args(default = Value::Bool(true))]
            strict: Value,
        }

        impl From<$ty> for NetworkArgs {
            fn from(args: $ty) -> Self {
                Self {
                    address: args.address,
                    strict: args.strict,
                }
            }
        }
    };
}
network_arguments!(V4NetworkArgs, "IPv4Network.__init__");
network_arguments!(V6NetworkArgs, "IPv6Network.__init__");

/// Argument shape for `subnets(prefixlen_diff=1, new_prefix=None)` and `supernet(...)`.
#[derive(FromArgs)]
#[from_args(name = "subnets", style = def)]
struct PrefixArgs {
    #[from_args(default = Value::Int(1))]
    prefixlen_diff: Value,
    #[from_args(default = Value::None)]
    new_prefix: Value,
}

// ============================================================================
// IpAddress
// ============================================================================

impl IpAddress {
    fn new(v6: bool, bits: u128) -> Self {
        Self {
            v6,
            bits: bits.into(),
            scope_id: None,
        }
    }

    fn bits(&self) -> u128 {
        self.bits.into()
    }

    /// Converts a constructor argument: an int, packed `bytes`, or the `str()` of
    /// anything else. `Err` is a parse failure; the outer `Err` an evaluation error.
    fn from_value(address: &Value, v6: bool, vm: &mut VM<'_>) -> RunResult<Result<Self, ParseError>> {
        if let Some(value) = value_to_bigint(address, vm) {
            return Ok(Self::from_bigint(&value, v6));
        }
        if let PyValue::Bytes(bytes) = address.py_value(vm) {
            let bytes = bytes.as_slice(vm);
            let expected = if v6 { 16 } else { 4 };
            if bytes.len() != expected {
                return Ok(Err(ParseError(format!(
                    "{} (len {} != {expected}) is not permitted as an IPv{} address",
                    parse::bytes_repr(bytes),
                    bytes.len(),
                    version(v6)
                ))));
            }
            let mut bits = 0u128;
            for &byte in bytes {
                bits = bits << 8 | u128::from(byte);
            }
            return Ok(Ok(Self::new(v6, bits)));
        }
        let text = address.py_str(vm)?;
        defer_drop!(text, vm);
        let text = text.to_str(vm)?;
        if text.contains('/') {
            return Ok(Err(ParseError(format!("Unexpected '/' in {}", repr_of(address, vm)?))));
        }
        Ok(Self::parse(text, v6))
    }

    /// Parses address text of one version.
    fn parse(text: &str, v6: bool) -> Result<Self, ParseError> {
        if v6 {
            let (text, scope_id) = parse::split_scope_id(text)?;
            let bits = parse::parse_v6(text)?;
            Ok(Self {
                v6: true,
                bits: bits.into(),
                scope_id: scope_id.map(str::to_owned),
            })
        } else {
            parse::parse_v4(text).map(|bits| Self::new(false, u128::from(bits)))
        }
    }

    /// An address from an integer, with CPython's range errors.
    fn from_bigint(value: &BigInt, v6: bool) -> Result<Self, ParseError> {
        if value.sign() == num_bigint::Sign::Minus {
            return Err(ParseError(format!(
                "{value} (< 0) is not permitted as an IPv{} address",
                version(v6)
            )));
        }
        match u128::try_from(value) {
            Ok(bits) if bits <= parse::all_ones(v6) => Ok(Self::new(v6, bits)),
            _ => Err(ParseError(format!(
                "{value} (>= 2**{}) is not permitted as an IPv{} address",
                parse::max_prefixlen(v6),
                version(v6)
            ))),
        }
    }

    /// The address a heap value holds, if it is one.
    fn read(value: &Value, vm: &VM<'_>) -> Option<Self> {
        match value {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::IpAddress(address) => Some(address.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn allocate(self, vm: &mut VM<'_>) -> Value {
        Value::Ref(vm.heap.allocate(HeapData::IpAddress(self)))
    }

    /// The `ipv4_mapped` IPv4 address, which the IPv6 classifications defer to.
    fn ipv4_mapped(&self) -> Option<Self> {
        if self.v6 {
            parse::ipv4_mapped(self.bits()).map(|v4| Self::new(false, u128::from(v4)))
        } else {
            None
        }
    }

    fn is_private(&self) -> bool {
        if let Some(mapped) = self.ipv4_mapped() {
            return mapped.is_private();
        }
        let (networks, exceptions) = if self.v6 {
            (parse::V6_PRIVATE, parse::V6_PRIVATE_EXCEPTIONS)
        } else {
            (parse::V4_PRIVATE, parse::V4_PRIVATE_EXCEPTIONS)
        };
        parse::in_any(self.v6, self.bits(), networks) && !parse::in_any(self.v6, self.bits(), exceptions)
    }

    fn is_global(&self) -> bool {
        if let Some(mapped) = self.ipv4_mapped() {
            return mapped.is_global();
        }
        if self.v6 {
            !self.is_private()
        } else {
            !parse::in_network(false, self.bits(), parse::V4_PUBLIC) && !self.is_private()
        }
    }

    fn is_multicast(&self) -> bool {
        match self.ipv4_mapped() {
            Some(mapped) => mapped.is_multicast(),
            None if self.v6 => parse::in_network(true, self.bits(), parse::V6_MULTICAST),
            None => parse::in_network(false, self.bits(), parse::V4_MULTICAST),
        }
    }

    fn is_reserved(&self) -> bool {
        match self.ipv4_mapped() {
            Some(mapped) => mapped.is_reserved(),
            None if self.v6 => parse::in_any(true, self.bits(), parse::V6_RESERVED),
            None => parse::in_network(false, self.bits(), parse::V4_RESERVED),
        }
    }

    fn is_link_local(&self) -> bool {
        match self.ipv4_mapped() {
            Some(mapped) => mapped.is_link_local(),
            None if self.v6 => parse::in_network(true, self.bits(), parse::V6_LINK_LOCAL),
            None => parse::in_network(false, self.bits(), parse::V4_LINK_LOCAL),
        }
    }

    fn is_site_local(&self) -> bool {
        self.v6 && parse::in_network(true, self.bits(), parse::V6_SITE_LOCAL)
    }

    fn is_unspecified(&self) -> bool {
        match self.ipv4_mapped() {
            Some(mapped) => mapped.is_unspecified(),
            None => self.bits() == 0,
        }
    }

    fn is_loopback(&self) -> bool {
        match self.ipv4_mapped() {
            Some(mapped) => mapped.is_loopback(),
            None if self.v6 => self.bits() == 1,
            None => parse::in_network(false, self.bits(), parse::V4_LOOPBACK),
        }
    }

    /// The compressed text without the scope id.
    fn compressed(&self) -> String {
        if self.v6 {
            parse::format_v6(self.bits())
        } else {
            parse::format_v4(parse::low_u32(self.bits()))
        }
    }

    /// `exploded`: every IPv6 group written out; the dotted form for IPv4.
    fn exploded(&self) -> String {
        if self.v6 {
            let mut exploded = parse::explode_v6(self.bits());
            if let Some(scope) = &self.scope_id {
                exploded.push('%');
                exploded.push_str(scope);
            }
            exploded
        } else {
            self.compressed()
        }
    }

    /// Class name for `repr()` and error messages.
    fn class_name(&self) -> &'static str {
        if self.v6 { "IPv6Address" } else { "IPv4Address" }
    }

    /// The Python type, without heap access.
    pub(crate) fn py_type(&self) -> Type {
        if self.v6 { Type::IPv6Address } else { Type::IPv4Address }
    }
}

impl Display for IpAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.compressed())?;
        if let Some(scope) = &self.scope_id {
            write!(f, "%{scope}")?;
        }
        Ok(())
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, IpAddress> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        if self.get(vm.heap).v6 {
            Type::IPv6Address
        } else {
            Type::IPv4Address
        }
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::IpAddress(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(self.get(vm.heap) == other.get(vm.heap)))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        let (a, b) = (self.get(vm.heap), other.get(vm.heap));
        if a.v6 != b.v6 {
            return Err(not_same_version(&a.to_string(), &b.to_string()));
        }
        Ok(CmpOrder::Ordered(a.bits().cmp(&b.bits())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let address = self.get(vm.heap);
        write!(f, "{}('{address}')", address.class_name())?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(allocate_string(self.get(vm.heap).to_string(), vm.heap))
    }

    fn py_int_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(Some(bits_value(self.get(vm.heap).bits(), vm)))
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.offset(other, false, vm)
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.offset(other, true, vm)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let address = self.get(vm.heap).clone();
        let Some(name) = attr.static_string(vm.interns) else {
            return Ok(None);
        };
        let text = |text: String, vm: &mut VM<'_>| allocate_string(text, vm.heap);
        let value = match name {
            StaticStrings::Version => Value::Int(version(address.v6)),
            StaticStrings::MaxPrefixlen => Value::Int(i64::from(parse::max_prefixlen(address.v6))),
            StaticStrings::Packed => allocate_bytes(packed_bytes(address.v6, address.bits()), vm.heap),
            StaticStrings::Exploded => text(address.exploded(), vm),
            StaticStrings::Compressed => text(address.to_string(), vm),
            StaticStrings::ReversePointer => text(
                parse::reverse_pointer(address.v6, address.bits(), address.scope_id.as_deref()),
                vm,
            ),
            StaticStrings::IsMulticast => Value::Bool(address.is_multicast()),
            StaticStrings::IsPrivate => Value::Bool(address.is_private()),
            StaticStrings::IsGlobal => Value::Bool(address.is_global()),
            StaticStrings::IsReserved => Value::Bool(address.is_reserved()),
            StaticStrings::IsLoopback => Value::Bool(address.is_loopback()),
            StaticStrings::IsLinkLocal => Value::Bool(address.is_link_local()),
            StaticStrings::IsUnspecified => Value::Bool(address.is_unspecified()),
            StaticStrings::Ipv6Mapped if !address.v6 => {
                IpAddress::new(true, 0xFFFF_0000_0000 | address.bits()).allocate(vm)
            }
            StaticStrings::IsSiteLocal if address.v6 => Value::Bool(address.is_site_local()),
            StaticStrings::ScopeId if address.v6 => match &address.scope_id {
                Some(scope) => text(scope.clone(), vm),
                None => Value::None,
            },
            StaticStrings::Ipv4Mapped if address.v6 => match address.ipv4_mapped() {
                Some(mapped) => mapped.allocate(vm),
                None => Value::None,
            },
            StaticStrings::Sixtofour if address.v6 => {
                let bits = address.bits();
                if bits >> 112 == 0x2002 {
                    IpAddress::new(false, (bits >> 80) & 0xFFFF_FFFF).allocate(vm)
                } else {
                    Value::None
                }
            }
            StaticStrings::Teredo if address.v6 => {
                let bits = address.bits();
                if bits >> 96 == 0x2001_0000 {
                    let server = IpAddress::new(false, (bits >> 64) & 0xFFFF_FFFF).allocate(vm);
                    let client = IpAddress::new(false, !bits & 0xFFFF_FFFF).allocate(vm);
                    allocate_tuple(smallvec![server, client], vm.heap)
                } else {
                    Value::None
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }
}

impl<'h> HeapObjectRead<'h, IpAddress> {
    /// `address + n` / `address - n` for an int `n`; anything else is `NotImplemented`.
    fn offset(&self, other: &Value, subtract: bool, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(delta) = value_to_bigint(other, vm) else {
            return Ok(None);
        };
        let address = self.get(vm.heap);
        let base = BigInt::from(address.bits());
        let result = if subtract { base - delta } else { base + delta };
        Ok(Some(
            IpAddress::from_bigint(&result, address.v6)
                .map_err(parse_error)?
                .allocate(vm),
        ))
    }
}

// ============================================================================
// IpNetwork
// ============================================================================

impl IpNetwork {
    fn new(v6: bool, network: u128, prefixlen: u32) -> Self {
        Self {
            v6,
            network: network.into(),
            prefixlen,
        }
    }

    fn network(&self) -> u128 {
        self.network.into()
    }

    fn netmask(&self) -> u128 {
        parse::netmask(self.v6, self.prefixlen)
    }

    fn hostmask(&self) -> u128 {
        self.netmask() ^ parse::all_ones(self.v6)
    }

    fn broadcast(&self) -> u128 {
        self.network() | self.hostmask()
    }

    fn network_address(&self) -> IpAddress {
        IpAddress::new(self.v6, self.network())
    }

    fn broadcast_address(&self) -> IpAddress {
        IpAddress::new(self.v6, self.broadcast())
    }

    /// `num_addresses`, which exceeds `u128` for `::/0`.
    fn num_addresses(&self) -> BigInt {
        BigInt::from(self.broadcast()) - BigInt::from(self.network()) + 1
    }

    /// Whether `bits` (of the same version) lies in this network.
    fn contains_bits(&self, bits: u128) -> bool {
        bits & self.netmask() == self.network()
    }

    /// The network `prefixlen_diff` bits shorter, clamped at `/0`.
    fn supernet_by(&self, prefixlen_diff: u32) -> Self {
        let prefixlen = self.prefixlen.saturating_sub(prefixlen_diff);
        Self::new(self.v6, self.network() & parse::netmask(self.v6, prefixlen), prefixlen)
    }

    /// Converts a constructor argument: `(address, mask)`, an int or `bytes` (a
    /// host network), or text with an optional `/mask`. `Err` is a parse failure;
    /// the outer `Err` an evaluation error, including strict host bits.
    fn from_value(address: &Value, strict: bool, v6: bool, vm: &mut VM<'_>) -> RunResult<Result<Self, ParseError>> {
        let width = parse::max_prefixlen(v6);
        let (host, prefixlen) =
            if value_to_bigint(address, vm).is_some() || matches!(address.py_value(vm), PyValue::Bytes(_)) {
                (IpAddress::from_value(address, v6, vm)?, Ok(width))
            } else if let Value::Ref(id) = address
                && let HeapData::Tuple(tuple) = vm.heap.get(*id)
            {
                let items: Vec<Value> = tuple
                    .as_slice()
                    .iter()
                    .map(|item| item.clone_with_heap(vm.heap))
                    .collect();
                defer_drop!(items, vm);
                let Some(first) = items.first() else {
                    return Err(ExcType::index_error("tuple index out of range"));
                };
                let host = IpAddress::from_value(first, v6, vm)?;
                let prefixlen = match items.get(1) {
                    Some(mask) => Self::prefixlen_from_value(mask, v6, vm)?,
                    None => Ok(width),
                };
                (host, prefixlen)
            } else {
                let text = address.py_str(vm)?;
                defer_drop!(text, vm);
                let text = text.to_str(vm)?;
                let parts: Vec<&str> = text.split('/').collect();
                if parts.len() > 2 {
                    return Ok(Err(ParseError(format!(
                        "Only one '/' permitted in {}",
                        repr_of(address, vm)?
                    ))));
                }
                let host = IpAddress::parse(parts[0], v6);
                let prefixlen = match parts.get(1) {
                    Some(mask) => Self::prefixlen_from_text(mask, v6),
                    None => Ok(width),
                };
                (host, prefixlen)
            };
        let (host, prefixlen) = match (host, prefixlen) {
            (Ok(host), Ok(prefixlen)) => (host, prefixlen),
            (Err(error), _) | (_, Err(error)) => return Ok(Err(error)),
        };
        let network = Self::new(v6, host.bits(), prefixlen);
        if host.bits() & network.netmask() != host.bits() {
            if strict {
                return Err(ExcType::value_error(format!("{network} has host bits set")));
            }
            return Ok(Ok(Self::new(v6, host.bits() & network.netmask(), prefixlen)));
        }
        Ok(Ok(network))
    }

    /// The prefix length of a mask given as an int or text.
    fn prefixlen_from_value(mask: &Value, v6: bool, vm: &mut VM<'_>) -> RunResult<Result<u32, ParseError>> {
        if let Some(value) = value_to_bigint(mask, vm) {
            return Ok(match u32::try_from(&value) {
                Ok(prefixlen) if prefixlen <= parse::max_prefixlen(v6) => Ok(prefixlen),
                _ => Err(ParseError(format!("{value} is not a valid netmask"))),
            });
        }
        if mask.is_str(vm.heap) {
            return Ok(Self::prefixlen_from_text(mask.to_str(vm)?, v6));
        }
        Ok(Err(ParseError(format!(
            "{} is not a valid netmask",
            repr_of(mask, vm)?
        ))))
    }

    /// The prefix length of mask text: a prefix length, or for IPv4 a dotted netmask or hostmask.
    fn prefixlen_from_text(mask: &str, v6: bool) -> Result<u32, ParseError> {
        parse::prefix_from_string(v6, mask).or_else(|error| {
            if v6 {
                Err(error)
            } else {
                parse::prefix_from_v4_mask(mask)
            }
        })
    }

    /// The network a heap value holds, if it is one.
    fn read(value: &Value, vm: &VM<'_>) -> Option<Self> {
        match value {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::IpNetwork(network) => Some(network.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn allocate(self, vm: &mut VM<'_>) -> Value {
        Value::Ref(vm.heap.allocate(HeapData::IpNetwork(self)))
    }

    /// Whether both ends of the network satisfy an address classification.
    fn both_ends(&self, test: impl Fn(&IpAddress) -> bool) -> bool {
        test(&self.network_address()) && test(&self.broadcast_address())
    }

    fn is_private(&self) -> bool {
        let (networks, exceptions) = if self.v6 {
            (parse::V6_PRIVATE, parse::V6_PRIVATE_EXCEPTIONS)
        } else {
            (parse::V4_PRIVATE, parse::V4_PRIVATE_EXCEPTIONS)
        };
        let (network, broadcast) = (self.network(), self.broadcast());
        networks
            .iter()
            .any(|net| parse::in_network(self.v6, network, *net) && parse::in_network(self.v6, broadcast, *net))
            && exceptions
                .iter()
                .all(|net| !parse::in_network(self.v6, network, *net) && !parse::in_network(self.v6, broadcast, *net))
    }

    fn is_global(&self) -> bool {
        if self.v6 {
            !self.is_private()
        } else {
            let shared = |bits| parse::in_network(false, bits, parse::V4_PUBLIC);
            !((shared(self.network()) && shared(self.broadcast())) || self.is_private())
        }
    }

    /// Whether `other` lies within this network, with CPython's version check.
    fn is_subnet_of(sub: &Self, sup: &Self) -> RunResult<bool> {
        if sub.v6 != sup.v6 {
            return Err(not_same_version(&sub.to_string(), &sup.to_string()));
        }
        Ok(sup.network() <= sub.network() && sup.broadcast() >= sub.broadcast())
    }

    /// `address_exclude(other)`: the subnets of this network that do not overlap `other`.
    fn address_exclude(&self, other: &Self) -> RunResult<Vec<Self>> {
        if self.v6 != other.v6 {
            return Err(not_same_version(&self.to_string(), &other.to_string()));
        }
        if !Self::is_subnet_of(other, self)? {
            return Err(ExcType::value_error(format!("{other} not contained in {self}")));
        }
        let mut result = Vec::new();
        if other == self {
            return Ok(result);
        }
        let (mut s1, mut s2) = self.halves();
        while s1 != *other && s2 != *other {
            if Self::is_subnet_of(other, &s1)? {
                result.push(s2);
                (s1, s2) = s1.halves();
            } else {
                result.push(s1);
                (s1, s2) = s2.halves();
            }
        }
        result.push(if s1 == *other { s2 } else { s1 });
        Ok(result)
    }

    /// The two subnets one bit longer.
    fn halves(&self) -> (Self, Self) {
        let prefixlen = self.prefixlen + 1;
        let step = 1u128 << (parse::max_prefixlen(self.v6) - prefixlen);
        (
            Self::new(self.v6, self.network(), prefixlen),
            Self::new(self.v6, self.network() + step, prefixlen),
        )
    }

    fn class_name(&self) -> &'static str {
        if self.v6 { "IPv6Network" } else { "IPv4Network" }
    }

    /// The Python type, without heap access.
    pub(crate) fn py_type(&self) -> Type {
        if self.v6 { Type::IPv6Network } else { Type::IPv4Network }
    }
}

impl PartialOrd for IpNetwork {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IpNetwork {
    /// CPython's order: by network address, then by netmask.
    fn cmp(&self, other: &Self) -> Ordering {
        (self.network(), self.netmask()).cmp(&(other.network(), other.netmask()))
    }
}

impl Display for IpNetwork {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network_address(), self.prefixlen)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, IpNetwork> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        if self.get(vm.heap).v6 {
            Type::IPv6Network
        } else {
            Type::IPv4Network
        }
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::IpNetwork(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(self.get(vm.heap) == other.get(vm.heap)))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        let (a, b) = (self.get(vm.heap), other.get(vm.heap));
        if a.v6 != b.v6 {
            return Err(not_same_version(&a.to_string(), &b.to_string()));
        }
        Ok(CmpOrder::Ordered(a.cmp(b)))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let network = self.get(vm.heap).clone();
        if let Some(address) = IpAddress::read(item, vm) {
            Ok(Some(address.v6 == network.v6 && network.contains_bits(address.bits())))
        } else if IpNetwork::read(item, vm).is_some() {
            Ok(Some(false))
        } else {
            // CPython reads `other.version` and `other._ip` straight off the operand.
            Err(ExcType::attribute_error(item.py_type_name(vm), "version"))
        }
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let network = self.get(vm.heap);
        write!(f, "{}('{network}')", network.class_name())?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(allocate_string(self.get(vm.heap).to_string(), vm.heap))
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let network = self.get(vm.heap).clone();
        let count = BigInt::from(network.broadcast()) - BigInt::from(network.network()) + 1;
        Ok(CallResult::Value(
            IpIterator::addresses(network.v6, network.network(), &count).allocate(vm),
        ))
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let network = self.get(vm.heap).clone();
        let Some(index) = value_to_bigint(key, vm) else {
            return Err(ExcType::type_error(format!(
                "'>=' not supported between instances of '{}' and 'int'",
                key.py_type_name(vm)
            )));
        };
        let (start, end) = (BigInt::from(network.network()), BigInt::from(network.broadcast()));
        let target = if index.sign() == num_bigint::Sign::Minus {
            end + index + 1
        } else {
            start + index
        };
        if target < BigInt::from(network.network()) || target > BigInt::from(network.broadcast()) {
            return Err(ExcType::index_error("address out of range"));
        }
        let bits = u128::try_from(&target).expect("within the network's range");
        Ok(IpAddress::new(network.v6, bits).allocate(vm))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let network = self.get(vm.heap).clone();
        let Some(name) = attr.static_string(vm.interns) else {
            return Ok(None);
        };
        let text = |text: String, vm: &mut VM<'_>| allocate_string(text, vm.heap);
        let value = match name {
            StaticStrings::Version => Value::Int(version(network.v6)),
            StaticStrings::MaxPrefixlen => Value::Int(i64::from(parse::max_prefixlen(network.v6))),
            StaticStrings::NetworkAddress => network.network_address().allocate(vm),
            StaticStrings::BroadcastAddress => network.broadcast_address().allocate(vm),
            StaticStrings::Netmask => IpAddress::new(network.v6, network.netmask()).allocate(vm),
            StaticStrings::Hostmask => IpAddress::new(network.v6, network.hostmask()).allocate(vm),
            StaticStrings::Prefixlen => Value::Int(i64::from(network.prefixlen)),
            StaticStrings::NumAddresses => LongInt::new(network.num_addresses()).into_value(vm.heap),
            StaticStrings::WithPrefixlen | StaticStrings::Compressed => text(network.to_string(), vm),
            StaticStrings::WithNetmask => text(
                format!(
                    "{}/{}",
                    network.network_address(),
                    IpAddress::new(network.v6, network.netmask())
                ),
                vm,
            ),
            StaticStrings::WithHostmask => text(
                format!(
                    "{}/{}",
                    network.network_address(),
                    IpAddress::new(network.v6, network.hostmask())
                ),
                vm,
            ),
            StaticStrings::Exploded => {
                let address = if network.v6 {
                    parse::explode_hex(network.network())
                } else {
                    network.network_address().compressed()
                };
                text(format!("{address}/{}", network.prefixlen), vm)
            }
            StaticStrings::IsMulticast => Value::Bool(network.both_ends(IpAddress::is_multicast)),
            StaticStrings::IsPrivate => Value::Bool(network.is_private()),
            StaticStrings::IsGlobal => Value::Bool(network.is_global()),
            StaticStrings::IsReserved => Value::Bool(network.both_ends(IpAddress::is_reserved)),
            StaticStrings::IsLoopback => Value::Bool(network.both_ends(IpAddress::is_loopback)),
            StaticStrings::IsLinkLocal => Value::Bool(network.both_ends(IpAddress::is_link_local)),
            StaticStrings::IsUnspecified => Value::Bool(network.both_ends(IpAddress::is_unspecified)),
            StaticStrings::IsSiteLocal if network.v6 => Value::Bool(network.both_ends(IpAddress::is_site_local)),
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let network = self.get(vm.heap).clone();
        let value = match attr.static_string(vm.interns) {
            Some(StaticStrings::Hosts) => {
                if args.count() > 0 {
                    let given = args.count() + 1;
                    args.drop_with(vm.heap);
                    return Err(ExcType::type_error(format!(
                        "_BaseNetwork.hosts() takes 1 positional argument but {given} were given"
                    )));
                }
                args.check_zero_args("hosts", vm.heap)?;
                network.hosts().allocate(vm)
            }
            Some(StaticStrings::Subnets) => {
                let (prefixlen_diff, new_prefix) = prefix_arguments(args, vm)?;
                network.subnets(prefixlen_diff, new_prefix)?.allocate(vm)
            }
            Some(StaticStrings::Supernet) => {
                let (prefixlen_diff, new_prefix) = prefix_arguments(args, vm)?;
                network.supernet(prefixlen_diff, new_prefix)?.allocate(vm)
            }
            Some(StaticStrings::Overlaps) => {
                let other = args.get_one_arg("overlaps", vm.heap)?;
                defer_drop!(other, vm);
                let Some(other) = IpNetwork::read(other, vm) else {
                    return Err(ExcType::type_error(format!(
                        "argument of type '{}' is not a container or iterable",
                        other.py_type_name(vm)
                    )));
                };
                let same = network.v6 == other.v6;
                Value::Bool(
                    same && (other.contains_bits(network.network())
                        || other.contains_bits(network.broadcast())
                        || network.contains_bits(other.network())
                        || network.contains_bits(other.broadcast())),
                )
            }
            Some(StaticStrings::SubnetOf | StaticStrings::SupernetOf) => {
                let supernet_of = attr.static_string(vm.interns) == Some(StaticStrings::SupernetOf);
                let name = if supernet_of { "supernet_of" } else { "subnet_of" };
                let other = args.get_one_arg(name, vm.heap)?;
                defer_drop!(other, vm);
                let Some(other) = IpNetwork::read(other, vm) else {
                    let other = other.py_str(vm)?;
                    defer_drop!(other, vm);
                    let other = other.to_str(vm)?.to_owned();
                    let (a, b) = if supernet_of {
                        (other, network.to_string())
                    } else {
                        (network.to_string(), other)
                    };
                    return Err(ExcType::type_error(format!(
                        "Unable to test subnet containment between {a} and {b}"
                    )));
                };
                let (sub, sup) = if supernet_of {
                    (&other, &network)
                } else {
                    (&network, &other)
                };
                Value::Bool(IpNetwork::is_subnet_of(sub, sup)?)
            }
            Some(StaticStrings::AddressExclude) => {
                let other = args.get_one_arg("address_exclude", vm.heap)?;
                defer_drop!(other, vm);
                let Some(other) = IpNetwork::read(other, vm) else {
                    if let Some(address) = IpAddress::read(other, vm) {
                        if address.v6 != network.v6 {
                            return Err(not_same_version(&network.to_string(), &address.to_string()));
                        }
                        return Err(ExcType::type_error(format!("{address} is not a network object")));
                    }
                    return Err(ExcType::attribute_error(other.py_type_name(vm), "version"));
                };
                let excluded: Vec<Value> = network
                    .address_exclude(&other)?
                    .into_iter()
                    .map(|network| network.allocate(vm))
                    .collect();
                return list_iterator(excluded, vm).map(CallResult::Value);
            }
            Some(StaticStrings::CompareNetworks) => {
                let other = args.get_one_arg("compare_networks", vm.heap)?;
                defer_drop!(other, vm);
                let Some(other) = IpNetwork::read(other, vm) else {
                    return Err(ExcType::attribute_error(other.py_type_name(vm), "version"));
                };
                if network.v6 != other.v6 {
                    return Err(ExcType::type_error(format!(
                        "{network} and {other} are not of the same type"
                    )));
                }
                Value::Int(match network.cmp(&other) {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                })
            }
            _ => {
                args.drop_with(vm.heap);
                return Err(ExcType::attribute_error(network.class_name(), attr.as_str(vm.interns)));
            }
        };
        Ok(CallResult::Value(value))
    }
}

impl IpNetwork {
    /// `hosts()`: the usable addresses. A `/31` or `/127` yields both ends, a
    /// host network its one address, and IPv6 includes the last address.
    fn hosts(&self) -> IpIterator {
        let width = parse::max_prefixlen(self.v6);
        let (start, count) = if self.prefixlen >= width - 1 {
            (self.network(), BigInt::from(1u128 << (width - self.prefixlen)))
        } else {
            let last = if self.v6 {
                self.broadcast()
            } else {
                self.broadcast() - 1
            };
            (self.network() + 1, BigInt::from(last - self.network()))
        };
        IpIterator::addresses(self.v6, start, &count)
    }

    /// `subnets(prefixlen_diff=1, new_prefix=None)`.
    fn subnets(&self, prefixlen_diff: i64, new_prefix: Option<i64>) -> RunResult<IpIterator> {
        let width = parse::max_prefixlen(self.v6);
        if self.prefixlen == width {
            return Ok(IpIterator::networks(self.v6, self.network(), 1, self.prefixlen, 1));
        }
        let prefixlen_diff = match new_prefix {
            Some(new_prefix) => {
                if new_prefix < i64::from(self.prefixlen) {
                    return Err(ExcType::value_error("new prefix must be longer"));
                }
                if prefixlen_diff != 1 {
                    return Err(ExcType::value_error("cannot set prefixlen_diff and new_prefix"));
                }
                new_prefix - i64::from(self.prefixlen)
            }
            None => prefixlen_diff,
        };
        if prefixlen_diff < 0 {
            return Err(ExcType::value_error("prefix length diff must be > 0"));
        }
        let new_prefixlen = i64::from(self.prefixlen) + prefixlen_diff;
        if new_prefixlen > i64::from(width) {
            return Err(ExcType::value_error(format!(
                "prefix length diff {new_prefixlen} is invalid for netblock {self}"
            )));
        }
        let new_prefixlen = u32::try_from(new_prefixlen).expect("checked above");
        let shift = u32::try_from(prefixlen_diff).expect("checked above");
        // `::/0` into host networks is 2**128 subnets: saturate, no iteration gets that far.
        let count = 1u128.checked_shl(shift).unwrap_or(u128::MAX);
        let step = 1u128 << (width - new_prefixlen);
        Ok(IpIterator::networks(
            self.v6,
            self.network(),
            count,
            new_prefixlen,
            step,
        ))
    }

    /// `supernet(prefixlen_diff=1, new_prefix=None)`.
    fn supernet(&self, prefixlen_diff: i64, new_prefix: Option<i64>) -> RunResult<Self> {
        if self.prefixlen == 0 {
            return Ok(self.clone());
        }
        let prefixlen_diff = match new_prefix {
            Some(new_prefix) => {
                if new_prefix > i64::from(self.prefixlen) {
                    return Err(ExcType::value_error("new prefix must be shorter"));
                }
                if prefixlen_diff != 1 {
                    return Err(ExcType::value_error("cannot set prefixlen_diff and new_prefix"));
                }
                i64::from(self.prefixlen) - new_prefix
            }
            None => prefixlen_diff,
        };
        let new_prefixlen = i64::from(self.prefixlen) - prefixlen_diff;
        if new_prefixlen < 0 {
            return Err(ExcType::value_error(format!(
                "current prefixlen is {}, cannot have a prefixlen_diff of {prefixlen_diff}",
                self.prefixlen
            )));
        }
        Ok(self.supernet_by(u32::try_from(prefixlen_diff).expect("checked above")))
    }
}

/// Binds `(prefixlen_diff=1, new_prefix=None)` for `subnets` and `supernet`.
fn prefix_arguments(args: ArgValues, vm: &mut VM<'_>) -> RunResult<(i64, Option<i64>)> {
    let PrefixArgs {
        prefixlen_diff,
        new_prefix,
    } = PrefixArgs::from_args(args, vm)?;
    defer_drop!(prefixlen_diff, vm);
    defer_drop!(new_prefix, vm);
    let prefixlen_diff = prefixlen_diff.as_int(vm)?;
    let new_prefix = match new_prefix {
        Value::None => None,
        value => Some(value.as_int(vm)?),
    };
    Ok((prefixlen_diff, new_prefix))
}

// ============================================================================
// IpIterator
// ============================================================================

impl IpIterator {
    /// `count` consecutive addresses from `start`; a count past `u128` saturates,
    /// which no iteration can reach.
    fn addresses(v6: bool, start: u128, count: &BigInt) -> Self {
        Self {
            v6,
            next: start.into(),
            remaining: u128::try_from(count).unwrap_or(u128::MAX).into(),
            step: 1u128.into(),
            prefixlen: None,
        }
    }

    /// `count` networks of `prefixlen` bits from `start`, `step` apart.
    fn networks(v6: bool, start: u128, count: u128, prefixlen: u32, step: u128) -> Self {
        Self {
            v6,
            next: start.into(),
            remaining: count.into(),
            step: step.into(),
            prefixlen: Some(prefixlen),
        }
    }

    fn allocate(self, vm: &mut VM<'_>) -> Value {
        Value::Ref(vm.heap.allocate(HeapData::IpIterator(self)))
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, IpIterator> {
    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::WalkGenerator
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let iter = self.get_mut(vm.heap);
        let remaining: u128 = iter.remaining.into();
        if remaining == 0 {
            return Ok(None);
        }
        let current: u128 = iter.next.into();
        iter.remaining = (remaining - 1).into();
        iter.next = current.wrapping_add(iter.step.into()).into();
        let (v6, prefixlen) = (iter.v6, iter.prefixlen);
        Ok(Some(match prefixlen {
            Some(prefixlen) => IpNetwork::new(v6, current, prefixlen).allocate(vm),
            None => IpAddress::new(v6, current).allocate(vm),
        }))
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

/// The version number as Python reports it.
fn version(v6: bool) -> i64 {
    if v6 { 6 } else { 4 }
}

/// The big-endian bytes of an address: `packed`.
fn packed_bytes(v6: bool, bits: u128) -> Vec<u8> {
    if v6 {
        bits.to_be_bytes().to_vec()
    } else {
        parse::low_u32(bits).to_be_bytes().to_vec()
    }
}

/// An address's integer as a Python int, heap-allocated when it is wide.
fn bits_value(bits: u128, vm: &mut VM<'_>) -> Value {
    match i64::try_from(bits) {
        Ok(small) => Value::Int(small),
        Err(_) => LongInt::new(BigInt::from(bits)).into_value(vm.heap),
    }
}

/// The integer a `bool` or `int` value holds, else `None`.
fn value_to_bigint(value: &Value, vm: &VM<'_>) -> Option<BigInt> {
    match value.py_value(vm) {
        PyValue::Bool(b) => Some(BigInt::from(i64::from(b))),
        PyValue::Int(integer) => Some(integer.with_value(vm, BigInt::from, Clone::clone)),
        _ => None,
    }
}

/// `repr(value)` as text, for the messages that quote their input.
fn repr_of(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let repr = value.py_repr(vm)?;
    defer_drop!(repr, vm);
    Ok(repr.to_str(vm)?.to_owned())
}

/// A `list_iterator` over `items`, standing in for CPython's generators where
/// the elements are computed up front.
fn list_iterator(items: Vec<Value>, vm: &mut VM<'_>) -> RunResult<Value> {
    let list = Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))));
    list.into_py_iter(vm)?.require_synchronous(vm)
}

fn parse_error(error: ParseError) -> RunError {
    ExcType::value_error(error.0)
}

fn not_same_version(a: &str, b: &str) -> RunError {
    ExcType::type_error(format!("{a} and {b} are not of the same version"))
}
