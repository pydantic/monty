//! Implementation of Python's `binascii` module.
//!
//! Every name CPython's `binascii` exposes: the `Error` and `Incomplete`
//! classes, the hex pair (`hexlify`/`unhexlify` and their `b2a_hex`/`a2b_hex`
//! aliases), the base64 pair (`b2a_base64`/`a2b_base64`), the uuencode pair
//! (`b2a_uu`/`a2b_uu`), the quoted-printable pair (`b2a_qp`/`a2b_qp`) and the
//! two checksums `crc32` and `crc_hqx`.
//!
//! These are C functions in CPython, not the pure Python of [`super::base64`],
//! so each argument struct names the parser family CPython's own definition
//! uses: `hexlify` is `PyArg_ParseTupleAndKeywords`, the base64 and uuencode
//! pairs are Argument Clinic with keyword-only flags, `crc32` and `crc_hqx`
//! are `PyArg_UnpackTuple`, and `unhexlify` and `a2b_uu` are `METH_O`, which
//! [`ArgValues::get_one_arg`] already words correctly.
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
    #[strum(serialize = "crc_hqx")]
    CrcHqx,
    #[strum(serialize = "b2a_uu")]
    B2aUu,
    #[strum(serialize = "a2b_uu")]
    A2bUu,
    #[strum(serialize = "b2a_qp")]
    B2aQp,
    #[strum(serialize = "a2b_qp")]
    A2bQp,
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
    (StaticStrings::CrcHqx, BinasciiFunctions::CrcHqx),
    (StaticStrings::B2aUu, BinasciiFunctions::B2aUu),
    (StaticStrings::A2bUu, BinasciiFunctions::A2bUu),
    (StaticStrings::B2aQp, BinasciiFunctions::B2aQp),
    (StaticStrings::A2bQp, BinasciiFunctions::A2bQp),
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
    module.set_attr(
        StaticStrings::IncompleteClass,
        Value::Builtin(Builtins::ExcType(ExcType::BinasciiIncomplete)),
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
        BinasciiFunctions::CrcHqx => call_crc_hqx(vm, args),
        BinasciiFunctions::B2aUu => call_b2a_uu(vm, args),
        BinasciiFunctions::A2bUu => call_a2b_uu(vm, args),
        BinasciiFunctions::B2aQp => call_b2a_qp(vm, args),
        BinasciiFunctions::A2bQp => call_a2b_qp(vm, args),
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

/// `binascii.crc_hqx(data, crc)` — the CRC-16 the classic BinHex format used,
/// resumable through `crc` exactly as `crc32` is.
fn call_crc_hqx(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CrcHqxArgs { data, crc } = CrcHqxArgs::from_args(args, vm)?;
    defer_drop!(data, vm);
    defer_drop!(crc, vm);

    // The `I` format takes the seed modulo 2**32, then the body narrows it to
    // the 16 bits the register holds — so `-1` and `0xffff` seed alike.
    let seed = u16::try_from(wrapping_u32(crc, vm)? & 0xffff).expect("masked below 2**16");
    let checksum = crc_hqx(encode_input(data, vm)?.as_ref(), seed);
    Ok(Value::Int(i64::from(checksum)))
}

/// `binascii.b2a_uu(data, *, backtick=False)` — one uuencoded line, newline
/// included, for at most the 45 bytes the line-length byte can describe.
fn call_b2a_uu(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B2aUuArgs { data, backtick } = B2aUuArgs::from_args(args, vm)?;
    defer_drop!(data, vm);
    defer_drop!(backtick, vm);

    let backtick = backtick.py_bool(vm)?;
    let encoded = uu_encode(encode_input(data, vm)?.as_ref(), backtick)?;
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `binascii.a2b_uu(data)` — the inverse, tolerant of either padding
/// convention and of a line truncated before its length byte promised.
fn call_a2b_uu(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let mut positional = args.into_pos_only("binascii.a2b_uu", vm.heap)?;
    let count = positional.len();
    let Some(data) = positional.next().filter(|_| count == 1) else {
        positional.drop_with(vm.heap);
        return Err(ExcType::type_error_arg_count("binascii.a2b_uu", 1, count));
    };
    defer_drop!(data, vm);

    let decoded = uu_decode(decode_input_described(data, vm, "bytes, buffer or ASCII string")?.as_ref())?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `binascii.b2a_qp(data, quotetabs=False, istext=True, header=False)` —
/// quoted-printable encoding, soft-wrapped to 76 columns.
fn call_b2a_qp(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B2aQpArgs {
        data,
        quotetabs,
        istext,
        header,
    } = B2aQpArgs::from_args(args, vm)?;
    defer_drop!(data, vm);
    defer_drop!(quotetabs, vm);
    defer_drop!(istext, vm);
    defer_drop!(header, vm);

    let options = QpOptions {
        quotetabs: quotetabs.py_bool(vm)?,
        istext: istext.py_bool(vm)?,
        header: header.py_bool(vm)?,
    };
    let encoded = qp_encode(encode_input(data, vm)?.as_ref(), options);
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `binascii.a2b_qp(data, header=False)` — quoted-printable decoding, which
/// never fails: anything malformed is copied through verbatim.
fn call_a2b_qp(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let A2bQpArgs { data, header } = A2bQpArgs::from_args(args, vm)?;
    defer_drop!(data, vm);
    defer_drop!(header, vm);

    let header = header.py_bool(vm)?;
    let decoded = qp_decode(
        decode_input_described(data, vm, "bytes, buffer or ASCII string")?.as_ref(),
        header,
    );
    Ok(allocate_bytes(decoded, vm.heap))
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

/// Argument shape for `crc_hqx(data, crc)` — like `crc32`, but with `crc`
/// required rather than defaulted.
#[derive(FromArgs)]
#[from_args(name = "crc_hqx", style = unpack, kwarg_error_name = "binascii.crc_hqx")]
struct CrcHqxArgs {
    #[from_args(pos_only)]
    data: Value,
    #[from_args(pos_only)]
    crc: Value,
}

/// Argument shape for `b2a_uu(data, *, backtick=False)`.
#[derive(FromArgs)]
#[from_args(name = "b2a_uu", style = c_named)]
struct B2aUuArgs {
    #[from_args(pos_only)]
    data: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    backtick: Value,
}

/// Argument shape for `b2a_qp(data, quotetabs=False, istext=True, header=False)`,
/// whose three flags are all positional-or-keyword.
#[derive(FromArgs)]
#[from_args(name = "b2a_qp", style = c_named, at_most_total)]
struct B2aQpArgs {
    data: Value,
    #[from_args(default = Value::Bool(false))]
    quotetabs: Value,
    #[from_args(default = Value::Bool(true))]
    istext: Value,
    #[from_args(default = Value::Bool(false))]
    header: Value,
}

/// Argument shape for `a2b_qp(data, header=False)`.
#[derive(FromArgs)]
#[from_args(name = "a2b_qp", style = c_named, at_most_total)]
struct A2bQpArgs {
    data: Value,
    #[from_args(default = Value::Bool(false))]
    header: Value,
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

/// Computes the CRC-16 of `data`, continuing from `seed`.
///
/// CRC-16/XMODEM: polynomial `0x1021`, unreflected, no final xor — the
/// checksum BinHex 4.0 carried, which `crc_hqx` outlived.
fn crc_hqx(data: &[u8], seed: u16) -> u16 {
    let mut crc = seed;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
    }
    crc
}

/// The most bytes one uuencoded line can hold, since the leading length byte
/// only spans the 6 bits `' '..='`'` encodes.
const UU_MAX_LINE: usize = 45;

/// Encodes one uuencoded line: a length byte, the data in 6-bit groups, and a
/// trailing newline.
///
/// `backtick` picks which of the two conventions spells a zero group — the
/// historic space, or the backtick that survives a mail gateway trimming
/// trailing whitespace.
fn uu_encode(data: &[u8], backtick: bool) -> RunResult<Vec<u8>> {
    if data.len() > UU_MAX_LINE {
        return Err(binascii_error("At most 45 bytes at once"));
    }
    // The length byte is a group like any other, so `backtick` reaches it too:
    // an empty input encodes as a lone backtick rather than a lone space.
    let sextet = |value: u8| match (value & 0x3f, backtick) {
        (0, true) => b'`',
        (value, _) => value + b' ',
    };

    let mut out = Vec::with_capacity(1 + data.len().div_ceil(3) * 4 + 1);
    out.push(sextet(u8::try_from(data.len()).expect("at most 45")));
    for chunk in data.chunks(3) {
        // Short chunks pad with zero bytes, which the length byte tells the
        // decoder to discard.
        let word = u32::from_be_bytes([0, chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)]);
        for shift in [18, 12, 6, 0] {
            out.push(sextet(u8::try_from((word >> shift) & 0x3f).expect("six bits")));
        }
    }
    out.push(b'\n');
    Ok(out)
}

/// Decodes one uuencoded line.
///
/// A line shorter than its length byte claims is zero-padded rather than
/// rejected, matching CPython — only a character outside the alphabet, or
/// non-whitespace left over once the promised bytes are decoded, is an error.
fn uu_decode(data: &[u8]) -> RunResult<Vec<u8>> {
    let Some((length_byte, mut rest)) = data.split_first() else {
        return Err(binascii_error("Missing length byte"));
    };
    let remaining = usize::from(length_byte.wrapping_sub(b' ') & 0x3f);

    let mut out = Vec::with_capacity(remaining);
    let mut leftover: u32 = 0;
    let mut bits = 0_u32;
    while out.len() < remaining {
        // Once the line runs out — or a newline ends it — the rest is zeros.
        let sextet = match rest.split_first() {
            Some((b'\n' | b'\r', _)) | None => 0,
            Some((byte, tail)) => {
                rest = tail;
                match byte {
                    b' ' | b'`' => 0,
                    b'!'..=b'_' => byte - b' ',
                    _ => return Err(binascii_error("Illegal char")),
                }
            }
        };
        leftover = (leftover << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((leftover >> bits) & 0xff).expect("one byte"));
            leftover &= (1 << bits) - 1;
        }
    }

    if rest.iter().all(|byte| matches!(byte, b' ' | b'`' | b'\n' | b'\r')) {
        Ok(out)
    } else {
        Err(binascii_error("Trailing garbage"))
    }
}

/// The column at which `b2a_qp` soft-wraps, CPython's `MAXLINESIZE`.
const QP_MAX_LINE: usize = 76;

/// The three flags `b2a_qp` reads, grouped so the encoder's helpers can take
/// them as one argument.
#[derive(Clone, Copy)]
struct QpOptions {
    /// Quote tabs and spaces wherever they appear, not just at end of line.
    quotetabs: bool,
    /// Treat the input as text, so newlines stay literal line breaks.
    istext: bool,
    /// RFC 2047 header mode: a space becomes `_`, and `_` is itself quoted.
    header: bool,
}

/// Encodes bytes as quoted-printable.
///
/// Soft line breaks (`=` then the newline) keep every line under
/// [`QP_MAX_LINE`]; which newline they use is decided once, up front, by
/// whether the input's first line ends `\r\n` — so mixed endings are
/// normalised to whichever came first, as CPython's own does.
fn qp_encode(data: &[u8], options: QpOptions) -> Vec<u8> {
    let crlf = data
        .iter()
        .position(|byte| *byte == b'\n')
        .is_some_and(|index| index > 0 && data[index - 1] == b'\r');

    let mut out = Vec::with_capacity(data.len());
    let mut linelen = 0;
    let mut index = 0;
    while index < data.len() {
        let byte = data[index];
        if qp_needs_quoting(data, index, linelen, options) {
            // The escape is three columns wide and may not be split, so it
            // moves to the next line whole.
            if linelen + 3 >= QP_MAX_LINE {
                qp_soft_break(&mut out, crlf);
                linelen = 0;
            }
            out.push(b'=');
            out.extend_from_slice(&qp_hex(byte));
            linelen += 3;
            index += 1;
        } else if options.istext && (byte == b'\n' || (byte == b'\r' && data.get(index + 1) == Some(&b'\n'))) {
            // A literal line break, but trailing whitespace before it would be
            // eaten in transit, so the last byte written is quoted after all.
            if let Some(last @ (b' ' | b'\t')) = out.last().copied() {
                out.pop();
                out.push(b'=');
                out.extend_from_slice(&qp_hex(last));
            }
            if crlf {
                out.push(b'\r');
            }
            out.push(b'\n');
            linelen = 0;
            index += if byte == b'\r' { 2 } else { 1 };
        } else {
            // A break right before a newline would be wasted, so it is skipped.
            if index + 1 != data.len() && data[index + 1] != b'\n' && linelen + 1 >= QP_MAX_LINE {
                qp_soft_break(&mut out, crlf);
                linelen = 0;
            }
            out.push(if options.header && byte == b' ' { b'_' } else { byte });
            linelen += 1;
            index += 1;
        }
    }
    out
}

/// Whether the byte at `index` has to be written as an `=XX` escape.
///
/// Beyond the obvious (non-printable, `=`, and `_` in header mode) this covers
/// three positional rules: trailing whitespace, which would not survive
/// transit; whitespace anywhere under `quotetabs`; and a `.` alone on a line,
/// which an SMTP relay would read as the end of the message.
fn qp_needs_quoting(data: &[u8], index: usize, linelen: usize, options: QpOptions) -> bool {
    let byte = data[index];
    let last = index + 1 == data.len();
    let leading_dot = byte == b'.' && linelen == 0 && matches!(data.get(index + 1), None | Some(b'\n' | b'\r' | b'\0'));

    byte > 126
        || byte == b'='
        || (options.header && byte == b'_')
        || leading_dot
        || (!options.istext && matches!(byte, b'\r' | b'\n'))
        || (matches!(byte, b'\t' | b' ') && last)
        || (byte < 33 && byte != b'\r' && byte != b'\n' && (options.quotetabs || !matches!(byte, b'\t' | b' ')))
}

/// Writes a soft line break — an `=` swallowed by the decoder, then the
/// newline the rest of the output uses.
fn qp_soft_break(out: &mut Vec<u8>, crlf: bool) {
    out.push(b'=');
    if crlf {
        out.push(b'\r');
    }
    out.push(b'\n');
}

/// The two uppercase hex digits an `=XX` escape carries.
fn qp_hex(byte: u8) -> [u8; 2] {
    const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";
    [UPPER_HEX[usize::from(byte >> 4)], UPPER_HEX[usize::from(byte & 0x0f)]]
}

/// Decodes quoted-printable.
///
/// Never fails: a malformed escape is copied through as the literal text it
/// was, which is what lets the format survive a mangled message.
fn qp_decode(data: &[u8], header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        match data[index] {
            b'=' => {
                index += 1;
                match data.get(index) {
                    // A trailing `=` is dropped along with the rest of the line.
                    None => break,
                    Some(b'\n' | b'\r') => {
                        // A soft break runs to the newline, so a bare `\r` (or
                        // a `\r\r\n`) swallows everything up to it.
                        while index < data.len() && data[index] != b'\n' {
                            index += 1;
                        }
                        index += usize::from(index < data.len());
                    }
                    // `==` is not an escape, but broken encoders emit it.
                    Some(b'=') => {
                        out.push(b'=');
                        index += 1;
                    }
                    Some(first) => match (hex_digit(*first), data.get(index + 1).and_then(|b| hex_digit(*b))) {
                        (Some(hi), Some(lo)) => {
                            out.push((hi << 4) | lo);
                            index += 2;
                        }
                        // Not a complete escape: the `=` stands for itself and
                        // the following bytes are re-read as ordinary text.
                        _ => out.push(b'='),
                    },
                }
            }
            b'_' if header => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}
