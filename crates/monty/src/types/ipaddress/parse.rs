//! Parsing, formatting and classification for `ipaddress`, ported from CPython's
//! `Lib/ipaddress.py` and working on plain integers and strings; `mod.rs` binds
//! them to Python values.
//!
//! Parse failures carry CPython's `AddressValueError` / `NetmaskValueError`
//! text; `ip_address()` and `ip_network()` swallow them for their own message.

use std::fmt::Write;

use crate::types::str::StringRepr;

/// The number of 16-bit groups in an IPv6 address.
const HEXTETS: usize = 8;

/// A rejected address or netmask, with the message CPython raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError(pub String);

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// The address width of a version, as `max_prefixlen` reports it.
pub(crate) fn max_prefixlen(v6: bool) -> u32 {
    if v6 { 128 } else { 32 }
}

/// Every bit set for a version: CPython's `_ALL_ONES`.
pub(crate) fn all_ones(v6: bool) -> u128 {
    if v6 { u128::MAX } else { u128::from(u32::MAX) }
}

/// The netmask of a prefix length.
pub(crate) fn netmask(v6: bool, prefixlen: u32) -> u128 {
    let ones = all_ones(v6);
    ones ^ ones.checked_shr(prefixlen).unwrap_or(0)
}

/// Parses dotted-quad text with CPython's `_BaseV4._ip_int_from_string` errors.
pub(crate) fn parse_v4(text: &str) -> Result<u32, ParseError> {
    if text.is_empty() {
        return Err(ParseError::new("Address cannot be empty"));
    }
    let octets: Vec<&str> = text.split('.').collect();
    if octets.len() != 4 {
        return Err(ParseError::new(format!("Expected 4 octets in {}", py_repr(text))));
    }
    let mut value = 0u32;
    for octet in octets {
        let byte = parse_octet(octet).map_err(|message| ParseError::new(format!("{message} in {}", py_repr(text))))?;
        value = value << 8 | u32::from(byte);
    }
    Ok(value)
}

/// One decimal octet: no sign, no leading zero, at most three digits, below 256.
fn parse_octet(octet: &str) -> Result<u8, String> {
    if octet.is_empty() {
        return Err("Empty octet not permitted".to_owned());
    }
    if !octet.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("Only decimal digits permitted in {}", py_repr(octet)));
    }
    if octet.len() > 3 {
        return Err(format!("At most 3 characters permitted in {}", py_repr(octet)));
    }
    if octet != "0" && octet.starts_with('0') {
        return Err(format!("Leading zeros are not permitted in {}", py_repr(octet)));
    }
    let value: u32 = octet.parse().expect("ascii digits");
    u8::try_from(value).map_err(|_| format!("Octet {value} (> 255) not permitted"))
}

/// Splits an IPv6 literal's scope id (`fe80::1%eth0`), as `_split_scope_id` does.
pub(crate) fn split_scope_id(text: &str) -> Result<(&str, Option<&str>), ParseError> {
    match text.split_once('%') {
        None => Ok((text, None)),
        Some((_, scope)) if scope.is_empty() || scope.contains('%') => {
            Err(ParseError::new(format!("Invalid IPv6 address: \"{}\"", py_repr(text))))
        }
        Some((address, scope)) => Ok((address, Some(scope))),
    }
}

/// Parses IPv6 text (scope id already removed) with CPython's
/// `_BaseV6._ip_int_from_string` errors.
pub(crate) fn parse_v6(text: &str) -> Result<u128, ParseError> {
    if text.is_empty() {
        return Err(ParseError::new("Address cannot be empty"));
    }
    if text.chars().count() > 45 {
        let count = text.chars().count();
        let shown = if count > 100 {
            let head: String = text.chars().take(45).collect();
            let tail: String = text.chars().skip(count - 45).collect();
            format!("{head}({} chars elided){tail}", count - 90)
        } else {
            text.to_owned()
        };
        return Err(ParseError::new(format!(
            "At most 45 characters expected in {}",
            py_repr(&shown)
        )));
    }
    let max_parts = HEXTETS + 1;
    let mut parts: Vec<String> = text.splitn(max_parts + 1, ':').map(str::to_owned).collect();
    if parts.len() < 3 {
        return Err(ParseError::new(format!(
            "At least 3 parts expected in {}",
            py_repr(text)
        )));
    }
    if let Some(last) = parts.pop_if(|last| last.contains('.')) {
        let v4 = parse_v4(&last).map_err(|e| ParseError::new(format!("{} in {}", e.0, py_repr(text))))?;
        parts.push(format!("{:x}", (v4 >> 16) & 0xFFFF));
        parts.push(format!("{:x}", v4 & 0xFFFF));
    }
    if parts.len() > max_parts {
        return Err(ParseError::new(format!(
            "At most {} colons permitted in {}",
            max_parts - 1,
            py_repr(text)
        )));
    }

    let mut skip_index = None;
    for (i, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip_index.is_some() {
                return Err(ParseError::new(format!(
                    "At most one '::' permitted in {}",
                    py_repr(text)
                )));
            }
            skip_index = Some(i);
        }
    }
    let leading = || {
        ParseError::new(format!(
            "Leading ':' only permitted as part of '::' in {}",
            py_repr(text)
        ))
    };
    let trailing = || {
        ParseError::new(format!(
            "Trailing ':' only permitted as part of '::' in {}",
            py_repr(text)
        ))
    };
    let (parts_hi, parts_lo, parts_skipped) = if let Some(skip) = skip_index {
        let mut hi = skip;
        let mut lo = parts.len() - skip - 1;
        if parts[0].is_empty() {
            hi -= 1;
            if hi > 0 {
                return Err(leading());
            }
        }
        if parts[parts.len() - 1].is_empty() {
            lo -= 1;
            if lo > 0 {
                return Err(trailing());
            }
        }
        if hi + lo >= HEXTETS {
            return Err(ParseError::new(format!(
                "Expected at most {} other parts with '::' in {}",
                HEXTETS - 1,
                py_repr(text)
            )));
        }
        (hi, lo, HEXTETS - (hi + lo))
    } else {
        if parts.len() != HEXTETS {
            return Err(ParseError::new(format!(
                "Exactly {HEXTETS} parts expected without '::' in {}",
                py_repr(text)
            )));
        }
        if parts[0].is_empty() {
            return Err(leading());
        }
        if parts[parts.len() - 1].is_empty() {
            return Err(trailing());
        }
        (parts.len(), 0, 0)
    };

    let hextet =
        |part: &str| parse_hextet(part).map_err(|message| ParseError::new(format!("{message} in {}", py_repr(text))));
    let mut value = 0u128;
    for part in &parts[..parts_hi] {
        value = value << 16 | u128::from(hextet(part)?);
    }
    // `::` alone skips every group, a shift by the full width.
    value = value
        .checked_shl(u32::try_from(16 * parts_skipped).expect("at most 128"))
        .unwrap_or(0);
    for part in &parts[parts.len() - parts_lo..] {
        value = value << 16 | u128::from(hextet(part)?);
    }
    Ok(value)
}

/// One hex group of at most four digits.
fn parse_hextet(hextet: &str) -> Result<u16, String> {
    if !hextet.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("Only hex digits permitted in {}", py_repr(hextet)));
    }
    if hextet.len() > 4 {
        return Err(format!("At most 4 characters permitted in {}", py_repr(hextet)));
    }
    Ok(u16::from_str_radix(hextet, 16).expect("at most four hex digits"))
}

/// The dotted-quad form of an IPv4 address.
pub(crate) fn format_v4(bits: u32) -> String {
    let [a, b, c, d] = bits.to_be_bytes();
    format!("{a}.{b}.{c}.{d}")
}

/// The compressed form of an IPv6 address: the longest run of zero groups
/// becomes `::`, and an IPv4-mapped address shows its IPv4 part dotted.
pub(crate) fn format_v6(bits: u128) -> String {
    if let Some(v4) = ipv4_mapped(bits) {
        return format!("{}:{}", compress_hextets(bits >> 32, 6), format_v4(v4));
    }
    compress_hextets(bits, 8)
}

/// Joins the low `count` 16-bit groups of `bits`, compressing the longest zero run.
fn compress_hextets(bits: u128, count: usize) -> String {
    let hextets: Vec<String> = (0..count)
        .rev()
        .map(|i| format!("{:x}", (bits >> (16 * i)) & 0xFFFF))
        .collect();
    let (mut best_start, mut best_len) = (None, 0);
    let (mut start, mut len) = (None, 0);
    for (index, hextet) in hextets.iter().enumerate() {
        if hextet == "0" {
            len += 1;
            start.get_or_insert(index);
            if len > best_len {
                best_len = len;
                best_start = start;
            }
        } else {
            len = 0;
            start = None;
        }
    }
    match best_start {
        Some(best_start) if best_len > 1 => {
            let head = hextets[..best_start].join(":");
            let tail = hextets[best_start + best_len..].join(":");
            format!("{head}::{tail}")
        }
        _ => hextets.join(":"),
    }
}

/// The exploded form of an IPv6 address: eight zero-padded groups, or the
/// dotted IPv4 part for an IPv4-mapped address.
pub(crate) fn explode_v6(bits: u128) -> String {
    let full = explode_hex(bits);
    match ipv4_mapped(bits) {
        Some(v4) => format!("{}{}", &full[..30], format_v4(v4)),
        None => full,
    }
}

/// Eight zero-padded hex groups: the exploded form of a network address, which
/// never shows an embedded IPv4 part.
pub(crate) fn explode_hex(bits: u128) -> String {
    (0..8)
        .rev()
        .map(|i| format!("{:04x}", (bits >> (16 * i)) & 0xFFFF))
        .collect::<Vec<_>>()
        .join(":")
}

/// The IPv4 address embedded in an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`).
pub(crate) fn ipv4_mapped(bits: u128) -> Option<u32> {
    (bits >> 32 == 0xFFFF).then_some(low_u32(bits))
}

/// The low 32 bits, which is all an IPv4 address occupies.
pub(crate) fn low_u32(bits: u128) -> u32 {
    u32::try_from(bits & 0xFFFF_FFFF).expect("masked to 32 bits")
}

/// `reverse_pointer` for either version. A scope id is reversed along with the
/// exploded text, as CPython reverses it, except on an IPv4-mapped address.
pub(crate) fn reverse_pointer(v6: bool, bits: u128, scope_id: Option<&str>) -> String {
    if v6 {
        // An IPv4-mapped address reverses the hex form of its IPv4 part.
        let exploded = match (ipv4_mapped(bits), scope_id) {
            (Some(v4), _) => format!("{}{v4:08x}", &explode_hex(bits)[..30]),
            (None, Some(scope)) => format!("{}%{scope}", explode_hex(bits)),
            (None, None) => explode_hex(bits),
        };
        let mut out = String::with_capacity(72);
        for c in exploded.chars().rev().filter(|c| *c != ':') {
            out.push(c);
            out.push('.');
        }
        out.push_str("ip6.arpa");
        out
    } else {
        let mut octets = format_v4(low_u32(bits))
            .split('.')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        octets.reverse();
        format!("{}.in-addr.arpa", octets.join("."))
    }
}

/// A prefix length from its decimal text, with `NetmaskValueError`'s wording.
pub(crate) fn prefix_from_string(v6: bool, text: &str) -> Result<u32, ParseError> {
    let invalid = || ParseError::new(format!("{} is not a valid netmask", py_repr(text)));
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    match text.parse::<u32>() {
        Ok(prefixlen) if prefixlen <= max_prefixlen(v6) => Ok(prefixlen),
        _ => Err(invalid()),
    }
}

/// An IPv4 prefix length from a dotted netmask or hostmask, which CPython tries
/// when the text is not a prefix length.
pub(crate) fn prefix_from_v4_mask(text: &str) -> Result<u32, ParseError> {
    let invalid = || ParseError::new(format!("{} is not a valid netmask", py_repr(text)));
    let bits = u128::from(parse_v4(text).map_err(|_| invalid())?);
    prefix_from_mask_bits(false, bits)
        .or_else(|| prefix_from_mask_bits(false, bits ^ all_ones(false)))
        .ok_or_else(invalid)
}

/// The prefix length of a netmask's bits, `None` unless they are leading ones.
fn prefix_from_mask_bits(v6: bool, bits: u128) -> Option<u32> {
    let width = max_prefixlen(v6);
    let trailing = if bits == 0 {
        width
    } else {
        bits.trailing_zeros().min(width)
    };
    let prefixlen = width - trailing;
    let leading_ones = bits.checked_shr(trailing).unwrap_or(0);
    let all = (1u128 << prefixlen) - 1;
    (leading_ones == all).then_some(prefixlen)
}

/// A table of networks used by the `is_*` classifications.
pub(crate) type Networks = &'static [(u128, u32)];

/// Whether `bits` lies in the network `(address, prefixlen)` of version `v6`.
pub(crate) fn in_network(v6: bool, bits: u128, network: (u128, u32)) -> bool {
    bits & netmask(v6, network.1) == network.0
}

/// Whether `bits` lies in any of `networks`.
pub(crate) fn in_any(v6: bool, bits: u128, networks: Networks) -> bool {
    networks.iter().any(|network| in_network(v6, bits, *network))
}

const fn v4(a: u8, b: u8, c: u8, d: u8, prefixlen: u32) -> (u128, u32) {
    (u32::from_be_bytes([a, b, c, d]) as u128, prefixlen)
}

/// `_IPv4Constants._private_networks`.
pub(crate) const V4_PRIVATE: Networks = &[
    v4(0, 0, 0, 0, 8),
    v4(10, 0, 0, 0, 8),
    v4(127, 0, 0, 0, 8),
    v4(169, 254, 0, 0, 16),
    v4(172, 16, 0, 0, 12),
    v4(192, 0, 0, 0, 24),
    v4(192, 0, 0, 170, 31),
    v4(192, 0, 2, 0, 24),
    v4(192, 168, 0, 0, 16),
    v4(198, 18, 0, 0, 15),
    v4(198, 51, 100, 0, 24),
    v4(203, 0, 113, 0, 24),
    v4(240, 0, 0, 0, 4),
    v4(255, 255, 255, 255, 32),
];
/// `_IPv4Constants._private_networks_exceptions`.
pub(crate) const V4_PRIVATE_EXCEPTIONS: Networks = &[v4(192, 0, 0, 9, 32), v4(192, 0, 0, 10, 32)];
/// `_IPv4Constants._public_network`, the shared address space.
pub(crate) const V4_PUBLIC: (u128, u32) = v4(100, 64, 0, 0, 10);
pub(crate) const V4_LINK_LOCAL: (u128, u32) = v4(169, 254, 0, 0, 16);
pub(crate) const V4_LOOPBACK: (u128, u32) = v4(127, 0, 0, 0, 8);
pub(crate) const V4_MULTICAST: (u128, u32) = v4(224, 0, 0, 0, 4);
pub(crate) const V4_RESERVED: (u128, u32) = v4(240, 0, 0, 0, 4);

const fn v6(hextets: [u16; 8], prefixlen: u32) -> (u128, u32) {
    let mut bits = 0u128;
    let mut i = 0;
    while i < 8 {
        bits = bits << 16 | hextets[i] as u128;
        i += 1;
    }
    (bits, prefixlen)
}

/// `_IPv6Constants._private_networks`.
pub(crate) const V6_PRIVATE: Networks = &[
    v6([0, 0, 0, 0, 0, 0, 0, 1], 128),
    v6([0, 0, 0, 0, 0, 0, 0, 0], 128),
    v6([0, 0, 0, 0, 0, 0xffff, 0, 0], 96),
    v6([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
    v6([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
    v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
    v6([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
    v6([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),
    v6([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
    v6([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
];
/// `_IPv6Constants._private_networks_exceptions`.
pub(crate) const V6_PRIVATE_EXCEPTIONS: Networks = &[
    v6([0x2001, 1, 0, 0, 0, 0, 0, 1], 128),
    v6([0x2001, 1, 0, 0, 0, 0, 0, 2], 128),
    v6([0x2001, 3, 0, 0, 0, 0, 0, 0], 32),
    v6([0x2001, 4, 0x112, 0, 0, 0, 0, 0], 48),
    v6([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28),
    v6([0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28),
];
/// `_IPv6Constants._reserved_networks`.
pub(crate) const V6_RESERVED: Networks = &[
    v6([0, 0, 0, 0, 0, 0, 0, 0], 8),
    v6([0x100, 0, 0, 0, 0, 0, 0, 0], 8),
    v6([0x200, 0, 0, 0, 0, 0, 0, 0], 7),
    v6([0x400, 0, 0, 0, 0, 0, 0, 0], 6),
    v6([0x800, 0, 0, 0, 0, 0, 0, 0], 5),
    v6([0x1000, 0, 0, 0, 0, 0, 0, 0], 4),
    v6([0x4000, 0, 0, 0, 0, 0, 0, 0], 3),
    v6([0x6000, 0, 0, 0, 0, 0, 0, 0], 3),
    v6([0x8000, 0, 0, 0, 0, 0, 0, 0], 3),
    v6([0xa000, 0, 0, 0, 0, 0, 0, 0], 3),
    v6([0xc000, 0, 0, 0, 0, 0, 0, 0], 3),
    v6([0xe000, 0, 0, 0, 0, 0, 0, 0], 4),
    v6([0xf000, 0, 0, 0, 0, 0, 0, 0], 5),
    v6([0xf800, 0, 0, 0, 0, 0, 0, 0], 6),
    v6([0xfe00, 0, 0, 0, 0, 0, 0, 0], 9),
];
pub(crate) const V6_LINK_LOCAL: (u128, u32) = v6([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10);
pub(crate) const V6_MULTICAST: (u128, u32) = v6([0xff00, 0, 0, 0, 0, 0, 0, 0], 8);
pub(crate) const V6_SITE_LOCAL: (u128, u32) = v6([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10);

/// `repr()` of a Python `str`, for the error messages that quote their input.
pub(crate) fn py_repr(text: &str) -> String {
    StringRepr(text).to_string()
}

/// `repr()` of Python `bytes`, for the packed-address error.
pub(crate) fn bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from("b");
    out.push(quote);
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b if b == quote as u8 => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(b)),
            // Writing to a `String` cannot fail.
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out.push(quote);
    out
}
