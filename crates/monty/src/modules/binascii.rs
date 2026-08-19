//! Implementation of Python's `binascii` module.
//!
//! The `Error` class every codec failure raises, the hex pair
//! (`hexlify`/`unhexlify` and their `b2a_hex`/`a2b_hex` aliases), the base64
//! pair (`b2a_base64`/`a2b_base64`) and `crc32`. The remaining conversions
//! (`a2b_uu`, `b2a_qp`, …) are absent — see `limitations/base64.md`.
//!
//! These are C functions in CPython, not the pure Python of [`super::base64`],
//! so each argument struct names the parser family CPython's own definition
//! uses: `hexlify` is `PyArg_ParseTupleAndKeywords`, the base64 pair is
//! Argument Clinic with keyword-only flags, `crc32` is `PyArg_UnpackTuple`,
//! and `unhexlify` is `METH_O`, which [`ArgValues::get_one_arg`] already
//! words correctly.
//!
//! The byte-level codecs live in [`super::base64`], where they were written
//! first; `binascii` re-exposes them under the names CPython puts them at.

use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::{
        ModuleFunctions,
        base64::{allocate_bytes, b64_decode, b64_encode, binascii_error, decode_input_described, encode_input},
    },
    types::{Module, PyTrait},
    value::Value,
};

/// Lowercase hex digits, `hexlify`'s output alphabet.
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// `binascii` module functions, one variant per Python-visible function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
pub(crate) enum BinasciiFunctions {
    #[strum(serialize = "hexlify")]
    Hexlify,
    #[strum(serialize = "unhexlify")]
    Unhexlify,
    #[strum(serialize = "b2a_hex")]
    B2aHex,
    #[strum(serialize = "a2b_hex")]
    A2bHex,
    #[strum(serialize = "b2a_base64")]
    B2aBase64,
    #[strum(serialize = "a2b_base64")]
    A2bBase64,
    #[strum(serialize = "crc32")]
    Crc32,
}

/// Static mapping of attribute names to functions for module creation.
const BINASCII_FUNCTIONS: &[(StaticStrings, BinasciiFunctions)] = &[
    (StaticStrings::Hexlify, BinasciiFunctions::Hexlify),
    (StaticStrings::Unhexlify, BinasciiFunctions::Unhexlify),
    (StaticStrings::B2aHex, BinasciiFunctions::B2aHex),
    (StaticStrings::A2bHex, BinasciiFunctions::A2bHex),
    (StaticStrings::B2aBase64, BinasciiFunctions::B2aBase64),
    (StaticStrings::A2bBase64, BinasciiFunctions::A2bBase64),
    (StaticStrings::Crc32, BinasciiFunctions::Crc32),
];

/// Creates the `binascii` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Binascii);

    module.set_attr(
        StaticStrings::ErrorClass,
        Value::Builtin(Builtins::ExcType(ExcType::BinasciiError)),
        vm,
    );
    for (name, func) in BINASCII_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Binascii(*func)), vm);
    }

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `binascii` module function.
///
/// All functions are pure computations and return `Value` directly.
pub(super) fn call(vm: &mut VM<'_>, function: BinasciiFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        BinasciiFunctions::Hexlify => call_hexlify(vm, args, "hexlify"),
        BinasciiFunctions::B2aHex => call_hexlify(vm, args, "b2a_hex"),
        BinasciiFunctions::Unhexlify => call_unhexlify(vm, args, "binascii.unhexlify"),
        BinasciiFunctions::A2bHex => call_unhexlify(vm, args, "binascii.a2b_hex"),
        BinasciiFunctions::B2aBase64 => call_b2a_base64(vm, args),
        BinasciiFunctions::A2bBase64 => call_a2b_base64(vm, args),
        BinasciiFunctions::Crc32 => call_crc32(vm, args),
    }
}

/// `binascii.hexlify(data, sep, bytes_per_sep=1)` and its `b2a_hex` alias,
/// which differ only in the name their signature errors carry.
fn call_hexlify(vm: &mut VM<'_>, args: ArgValues, name: &str) -> RunResult<Value> {
    let (data, sep, bytes_per_sep) = if name == "hexlify" {
        let HexlifyArgs {
            data,
            sep,
            bytes_per_sep,
        } = HexlifyArgs::from_args(args, vm)?;
        (data, sep, bytes_per_sep)
    } else {
        let B2aHexArgs {
            data,
            sep,
            bytes_per_sep,
        } = B2aHexArgs::from_args(args, vm)?;
        (data, sep, bytes_per_sep)
    };
    defer_drop!(data, vm);
    defer_drop!(sep, vm);

    let separator = hex_separator(sep.as_ref(), vm)?;
    let encoded = hex_encode(encode_input(data, vm)?.as_ref(), separator, bytes_per_sep);
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `binascii.unhexlify(hexstr)` and its `a2b_hex` alias — `METH_O`, so
/// keywords are rejected wholesale before the arity is even counted, both
/// under the module-qualified `name`.
fn call_unhexlify(vm: &mut VM<'_>, args: ArgValues, name: &str) -> RunResult<Value> {
    let mut positional = args.into_pos_only(name, vm.heap)?;
    let count = positional.len();
    let Some(hexstr) = positional.next().filter(|_| count == 1) else {
        positional.drop_with(vm.heap);
        return Err(ExcType::type_error_arg_count(name, 1, count));
    };
    defer_drop!(hexstr, vm);

    let decoded = hex_decode(decode_input_described(hexstr, vm, "bytes, buffer or ASCII string")?.as_ref())?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `binascii.b2a_base64(data, *, newline=True)` — base64 with the trailing
/// newline `encodebytes` relies on.
fn call_b2a_base64(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B2aBase64Args { data, newline } = B2aBase64Args::from_args(args, vm)?;
    defer_drop!(data, vm);
    defer_drop!(newline, vm);

    let mut encoded = b64_encode(encode_input(data, vm)?.as_ref());
    if newline.py_bool(vm)? {
        encoded.push(b'\n');
    }
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `binascii.a2b_base64(data, *, strict_mode=False)` — the decoder every
/// `base64` decode path funnels into.
fn call_a2b_base64(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let A2bBase64Args { data, strict_mode } = A2bBase64Args::from_args(args, vm)?;
    defer_drop!(data, vm);
    defer_drop!(strict_mode, vm);

    let strict = strict_mode.py_bool(vm)?;
    let decoded = b64_decode(
        decode_input_described(data, vm, "bytes, buffer or ASCII string")?.as_ref(),
        strict,
    )?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `binascii.crc32(data, crc=0)` — CRC-32 as used by zip and png, resumable
/// by feeding the previous result back in as `crc`.
fn call_crc32(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let Crc32Args { data, crc } = Crc32Args::from_args(args, vm)?;
    defer_drop!(data, vm);
    defer_drop!(crc, vm);

    // CPython converts `crc` with the `I` format, which takes any int modulo
    // 2**32 rather than rejecting one out of range.
    let seed = match crc {
        None => 0,
        Some(value) => wrapping_u32(value, vm)?,
    };
    let checksum = crc32(encode_input(data, vm)?.as_ref(), seed);
    Ok(Value::Int(i64::from(checksum)))
}

/// Argument shape for `hexlify(data, sep, bytes_per_sep=1)`.
///
/// `PyArg_ParseTupleAndKeywords` with a `:name`, plus the up-front count that
/// reports `takes at most 3 arguments` rather than an unexpected keyword.
#[derive(FromArgs)]
#[from_args(name = "hexlify", style = c_named, at_most_total)]
struct HexlifyArgs {
    data: Value,
    #[from_args(default)]
    sep: Option<Value>,
    #[from_args(default = 1)]
    bytes_per_sep: i32,
}

/// The same shape under the `b2a_hex` name, which its signature errors report.
#[derive(FromArgs)]
#[from_args(name = "b2a_hex", style = c_named, at_most_total)]
struct B2aHexArgs {
    data: Value,
    #[from_args(default)]
    sep: Option<Value>,
    #[from_args(default = 1)]
    bytes_per_sep: i32,
}

/// Argument shape for `b2a_base64(data, *, newline=True)`.
#[derive(FromArgs)]
#[from_args(name = "b2a_base64", style = c_named)]
struct B2aBase64Args {
    #[from_args(pos_only)]
    data: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    newline: Value,
}

/// Argument shape for `a2b_base64(data, *, strict_mode=False)`.
#[derive(FromArgs)]
#[from_args(name = "a2b_base64", style = c_named)]
struct A2bBase64Args {
    #[from_args(pos_only)]
    data: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    strict_mode: Value,
}

/// Argument shape for `crc32(data, crc=0)` — positional-only, so keywords are
/// rejected outright with the module-qualified name.
#[derive(FromArgs)]
#[from_args(name = "crc32", style = unpack, kwarg_error_name = "binascii.crc32")]
struct Crc32Args {
    #[from_args(pos_only)]
    data: Value,
    // Absent rather than `None`: CPython's `crc` defaults to unset, so an
    // explicit `None` reaches the integer conversion and is rejected there.
    #[from_args(pos_only, default)]
    crc: Option<Value>,
}

/// Reduces an `int` to the `unsigned int` CPython's `I` format takes, which
/// wraps modulo 2**32 rather than rejecting a value out of range — so a
/// negative or huge `crc` seed is accepted, as CPython accepts it.
fn wrapping_u32(value: &Value, vm: &VM<'_>) -> RunResult<u32> {
    const MODULUS: i64 = 1 << 32;

    match value {
        Value::Int(int) => Ok(u32::try_from(int.rem_euclid(MODULUS)).expect("reduced below 2**32")),
        Value::Bool(flag) => Ok(u32::from(*flag)),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::LongInt(long) => {
                // `&` on a `BigInt` is two's-complement, so this is Python's
                // own `value & 0xffffffff` for negatives as well.
                Ok((&long.0 & BigInt::from(u32::MAX)).to_u32().expect("masked below 2**32"))
            }
            _ => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
        },
        _ => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
    }
}

/// Encodes bytes as lowercase hex, inserting `sep` every `bytes_per_sep` bytes.
///
/// A positive `bytes_per_sep` groups from the right, so any short group leads;
/// a negative one groups from the left. Zero, or no separator, means none.
fn hex_encode(data: &[u8], sep: Option<u8>, bytes_per_sep: i32) -> Vec<u8> {
    let group = match sep {
        Some(_) if bytes_per_sep != 0 && !data.is_empty() => usize::try_from(bytes_per_sep.unsigned_abs())
            .expect("u32 fits usize on supported targets")
            .min(data.len()),
        // Without a separator, or with an empty input, emit one flat run.
        _ => data.len().max(1),
    };
    // Groups are measured from the right when positive, so the leading group
    // absorbs the remainder.
    let lead = if bytes_per_sep > 0 {
        match data.len() % group {
            0 => group,
            remainder => remainder,
        }
    } else {
        group
    };

    let mut out = Vec::with_capacity(data.len() * 2 + data.len() / group);
    for (index, byte) in data.iter().enumerate() {
        if let Some(sep) = sep
            && (index == lead || (index > lead && (index - lead) % group == 0))
        {
            out.push(sep);
        }
        out.push(HEX_DIGITS[usize::from(byte >> 4)]);
        out.push(HEX_DIGITS[usize::from(byte & 0x0f)]);
    }
    out
}

/// Decodes hex of either case, CPython's `unhexlify`.
fn hex_decode(data: &[u8]) -> RunResult<Vec<u8>> {
    if data.len().is_multiple_of(2) {
        data.chunks(2)
            .map(|pair| match (hex_digit(pair[0]), hex_digit(pair[1])) {
                (Some(hi), Some(lo)) => Ok((hi << 4) | lo),
                _ => Err(binascii_error("Non-hexadecimal digit found")),
            })
            .collect()
    } else {
        Err(binascii_error("Odd-length string"))
    }
}

/// Maps a hex digit of either case to its value, `None` for anything else.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Extracts `hexlify`'s separator: one byte, from a `bytes` or a `str` whose
/// single character is Latin-1, which is what CPython's `sep` accepts.
///
/// CPython measures the length before it looks at the type, so an unsized
/// separator reports "has no len()" and a sized one of the wrong length is a
/// `ValueError` — "sep must be str or bytes." only applies to a sized object
/// of length one. `None` is not "no separator": omitting `sep` leaves it
/// unset, so an explicit `None` fails the length check as any other object.
fn hex_separator(sep: Option<&Value>, vm: &VM<'_>) -> RunResult<Option<u8>> {
    let Some(sep) = sep else {
        return Ok(None);
    };
    let length = sep
        .py_len(vm)
        .ok_or_else(|| ExcType::type_error(format!("object of type '{}' has no len()", sep.py_type_name(vm))))?;

    if length != 1 {
        Err(value_error("sep must be length 1."))
    } else if sep.is_str(vm.heap) {
        // Latin-1, not ASCII: `hexlify` returns bytes, so CPython only rejects
        // a character that does not fit a byte, under an "ASCII" message.
        u8::try_from(u32::from(sep.to_str(vm)?.chars().next().expect("one character")))
            .map(Some)
            .map_err(|_| value_error("sep must be ASCII."))
    } else if is_bytes(sep, vm) {
        Ok(Some(encode_input(sep, vm)?[0]))
    } else {
        Err(ExcType::type_error("sep must be str or bytes."))
    }
}

/// Whether a value is `bytes`, which is all `hexlify` accepts as a separator
/// besides `str` — a buffer that `encode_input` would take is still rejected.
fn is_bytes(value: &Value, vm: &VM<'_>) -> bool {
    match value {
        Value::InternBytes(_) => true,
        Value::Ref(heap_id) => matches!(vm.heap.get(*heap_id), HeapData::Bytes(_)),
        _ => false,
    }
}

/// The `ValueError`s `hexlify` raises for a malformed separator, whose
/// messages end in a full stop as CPython's do.
fn value_error(message: &'static str) -> RunError {
    SimpleException::new_msg(ExcType::ValueError, message).into()
}

/// Computes the CRC-32 of `data`, continuing from `seed`.
///
/// The standard reflected polynomial (`0xedb88320`), computed a nibble at a
/// time so the table stays 16 entries rather than 256.
fn crc32(data: &[u8], seed: u32) -> u32 {
    const NIBBLE_TABLE: [u32; 16] = [
        0x0000_0000,
        0x1db7_1064,
        0x3b6e_20c8,
        0x26d9_30ac,
        0x76dc_4190,
        0x6b6b_51f4,
        0x4db2_6158,
        0x5005_713c,
        0xedb8_8320,
        0xf00f_9344,
        0xd6d6_a3e8,
        0xcb61_b38c,
        0x9b64_c2b0,
        0x86d3_d2d4,
        0xa00a_e278,
        0xbdbd_f21c,
    ];

    let mut crc = !seed;
    for byte in data {
        crc ^= u32::from(*byte);
        crc = (crc >> 4) ^ NIBBLE_TABLE[usize::try_from(crc & 0x0f).expect("nibble fits usize")];
        crc = (crc >> 4) ^ NIBBLE_TABLE[usize::try_from(crc & 0x0f).expect("nibble fits usize")];
    }
    !crc
}
