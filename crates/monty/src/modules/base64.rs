//! Implementation of Python's `base64` module.
//!
//! Covers the base64/base32/base16/base85 codecs, the ascii85 pair and the
//! MIME helpers `encodebytes`/`decodebytes`; the file-object `encode`/`decode`
//! pair is not — see `limitations/base64.md`.
//!
//! CPython's `base64` is pure Python delegating to `binascii`, so every
//! function here uses `#[from_args(style = def)]` and coerces in the body,
//! reproducing both CPython's signature errors and the order its bodies reject
//! bad values in. Decode failures raise [`ExcType::BinasciiError`]
//! (`binascii.Error`, a `ValueError` subclass) with `binascii`'s wording.
//!
//! Encoders take bytes only; decoders also take ASCII `str` (CPython's
//! `_bytes_from_decode_data`).

use std::{borrow::Cow, cmp::Ordering};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{CmpOrder, Module, PyTrait, bytes::bytes_repr},
    value::Value,
};

/// Standard base64 alphabet (RFC 4648 §4).
const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// Standard base32 alphabet (RFC 4648 §6).
const B32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Base85 alphabet used by `b85encode` (RFC 1924 order, as CPython spells it).
const B85_ALPHABET: &[u8; 85] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
/// Z85 alphabet (ZeroMQ RFC 32), the same scheme over a shell-safe character set.
const Z85_ALPHABET: &[u8; 85] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";

/// Lowest Ascii85 digit: value `v` encodes as `A85_FIRST_DIGIT + v`, so the
/// alphabet is the 85 characters `!` through `u`.
const A85_FIRST_DIGIT: u8 = b'!';
/// Highest Ascii85 digit, and the character CPython pads a trailing partial
/// group with.
const A85_LAST_DIGIT: u8 = b'u';
/// Opening marker of the Adobe framing, optional on decode.
const A85_ADOBE_START: &[u8; 2] = b"<~";
/// Closing marker of the Adobe framing, required on decode when `adobe` is set.
const A85_ADOBE_END: &[u8; 2] = b"~>";

/// Extended-hex base32 alphabet (RFC 4648 §7), used by `b32hexencode`.
const B32HEX_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";

/// Bytes per `encodebytes` output line: 57 input bytes encode to 76 base64
/// characters, CPython's `MAXBINSIZE`.
const MAX_BIN_SIZE: u8 = 57;
/// Characters per `encodebytes` output line, CPython's `MAXLINESIZE`. Both are
/// exposed as module attributes, as CPython does.
const MAX_LINE_SIZE: u8 = 76;

/// `base64` module functions, one variant per Python-visible function.
///
/// Serialized into dumps by discriminant, so new functions are appended here
/// rather than slotted in beside the codec they belong with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
pub(crate) enum Base64Functions {
    #[strum(serialize = "b64encode")]
    B64Encode,
    #[strum(serialize = "b64decode")]
    B64Decode,
    #[strum(serialize = "standard_b64encode")]
    StandardB64Encode,
    #[strum(serialize = "standard_b64decode")]
    StandardB64Decode,
    #[strum(serialize = "urlsafe_b64encode")]
    UrlsafeB64Encode,
    #[strum(serialize = "urlsafe_b64decode")]
    UrlsafeB64Decode,
    #[strum(serialize = "b32encode")]
    B32Encode,
    #[strum(serialize = "b32decode")]
    B32Decode,
    #[strum(serialize = "b32hexencode")]
    B32HexEncode,
    #[strum(serialize = "b32hexdecode")]
    B32HexDecode,
    #[strum(serialize = "b16encode")]
    B16Encode,
    #[strum(serialize = "b16decode")]
    B16Decode,
    #[strum(serialize = "encodebytes")]
    Encodebytes,
    #[strum(serialize = "decodebytes")]
    Decodebytes,
    #[strum(serialize = "b85encode")]
    B85Encode,
    #[strum(serialize = "b85decode")]
    B85Decode,
    #[strum(serialize = "z85encode")]
    Z85Encode,
    #[strum(serialize = "z85decode")]
    Z85Decode,
    #[strum(serialize = "a85encode")]
    A85Encode,
    #[strum(serialize = "a85decode")]
    A85Decode,
}

/// Static mapping of attribute names to functions for module creation.
const BASE64_FUNCTIONS: &[(StaticStrings, Base64Functions)] = &[
    (StaticStrings::B64Encode, Base64Functions::B64Encode),
    (StaticStrings::B64Decode, Base64Functions::B64Decode),
    (StaticStrings::StandardB64Encode, Base64Functions::StandardB64Encode),
    (StaticStrings::StandardB64Decode, Base64Functions::StandardB64Decode),
    (StaticStrings::UrlsafeB64Encode, Base64Functions::UrlsafeB64Encode),
    (StaticStrings::UrlsafeB64Decode, Base64Functions::UrlsafeB64Decode),
    (StaticStrings::B32Encode, Base64Functions::B32Encode),
    (StaticStrings::B32Decode, Base64Functions::B32Decode),
    (StaticStrings::B32HexEncode, Base64Functions::B32HexEncode),
    (StaticStrings::B32HexDecode, Base64Functions::B32HexDecode),
    (StaticStrings::B16Encode, Base64Functions::B16Encode),
    (StaticStrings::B16Decode, Base64Functions::B16Decode),
    (StaticStrings::Encodebytes, Base64Functions::Encodebytes),
    (StaticStrings::Decodebytes, Base64Functions::Decodebytes),
    (StaticStrings::B85Encode, Base64Functions::B85Encode),
    (StaticStrings::B85Decode, Base64Functions::B85Decode),
    (StaticStrings::Z85Encode, Base64Functions::Z85Encode),
    (StaticStrings::Z85Decode, Base64Functions::Z85Decode),
    (StaticStrings::A85Encode, Base64Functions::A85Encode),
    (StaticStrings::A85Decode, Base64Functions::A85Decode),
];

/// Creates the `base64` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Base64);

    for (name, func) in BASE64_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Base64(*func)), vm);
    }

    module.set_attr(StaticStrings::MaxBinSize, Value::Int(i64::from(MAX_BIN_SIZE)), vm);
    module.set_attr(StaticStrings::MaxLineSize, Value::Int(i64::from(MAX_LINE_SIZE)), vm);

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `base64` module function.
///
/// All functions are pure computations and return `Value` directly.
pub(super) fn call(vm: &mut VM<'_>, function: Base64Functions, args: ArgValues) -> RunResult<Value> {
    match function {
        Base64Functions::B64Encode => call_b64encode(vm, args),
        Base64Functions::B64Decode => call_b64decode(vm, args),
        Base64Functions::StandardB64Encode => call_standard_b64encode(vm, args),
        Base64Functions::StandardB64Decode => call_standard_b64decode(vm, args),
        Base64Functions::UrlsafeB64Encode => call_urlsafe_b64encode(vm, args),
        Base64Functions::UrlsafeB64Decode => call_urlsafe_b64decode(vm, args),
        Base64Functions::B32Encode => call_b32encode(vm, args),
        Base64Functions::B32Decode => call_b32decode(vm, args),
        Base64Functions::B32HexEncode => call_b32hexencode(vm, args),
        Base64Functions::B32HexDecode => call_b32hexdecode(vm, args),
        Base64Functions::B16Encode => call_b16encode(vm, args),
        Base64Functions::B16Decode => call_b16decode(vm, args),
        Base64Functions::Encodebytes => call_encodebytes(vm, args),
        Base64Functions::Decodebytes => call_decodebytes(vm, args),
        Base64Functions::B85Encode => call_b85encode(vm, args),
        Base64Functions::B85Decode => call_b85decode(vm, args),
        Base64Functions::Z85Encode => call_z85encode(vm, args),
        Base64Functions::Z85Decode => call_z85decode(vm, args),
        Base64Functions::A85Encode => call_a85encode(vm, args),
        Base64Functions::A85Decode => call_a85decode(vm, args),
    }
}

/// `base64.b64encode(s, altchars=None)` — standard base64 with optional
/// substitutes for the `+` and `/` characters.
fn call_b64encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B64EncodeArgs { s, altchars } = B64EncodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    defer_drop!(altchars, vm);

    let mut encoded = b64_encode(encode_input(s, vm)?.as_ref());
    if !matches!(altchars, Value::None) {
        // CPython asserts on the raw object's length before `bytes.maketrans`
        // type-checks it, so a 1-byte `str` is an AssertionError, not a TypeError.
        assert_len(altchars, 2, vm)?;
        let alt = encode_input(altchars, vm)?.into_owned();
        translate(&mut encoded, b"+/", &alt);
    }
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.b64decode(s, altchars=None, validate=False)`.
///
/// With `validate=False` (the default) anything outside the alphabet is
/// discarded before the quads are assembled; `validate=True` rejects it.
fn call_b64decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B64DecodeArgs { s, altchars, validate } = B64DecodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    defer_drop!(altchars, vm);
    defer_drop!(validate, vm);

    let mut data = decode_input(s, vm)?.into_owned();
    if !matches!(altchars, Value::None) {
        // Unlike the encode path, CPython coerces before asserting the length.
        let alt = decode_input(altchars, vm)?.into_owned();
        assert_len_bytes(&alt, 2)?;
        translate(&mut data, &alt, b"+/");
    }
    let strict = validate.py_bool(vm)?;
    let decoded = b64_decode(&data, strict)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.standard_b64encode(s)` — `b64encode` without `altchars`.
fn call_standard_b64encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let StandardB64EncodeArgs { s } = StandardB64EncodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let encoded = b64_encode(encode_input(s, vm)?.as_ref());
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.standard_b64decode(s)` — `b64decode` without `altchars`.
fn call_standard_b64decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let StandardB64DecodeArgs { s } = StandardB64DecodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let decoded = b64_decode(decode_input(s, vm)?.as_ref(), false)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.urlsafe_b64encode(s)` — standard base64 with `-` and `_` in place
/// of `+` and `/`.
fn call_urlsafe_b64encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let UrlsafeB64EncodeArgs { s } = UrlsafeB64EncodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let mut encoded = b64_encode(encode_input(s, vm)?.as_ref());
    translate(&mut encoded, b"+/", b"-_");
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.urlsafe_b64decode(s)` — the inverse of [`call_urlsafe_b64encode`].
fn call_urlsafe_b64decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let UrlsafeB64DecodeArgs { s } = UrlsafeB64DecodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let mut data = decode_input(s, vm)?.into_owned();
    translate(&mut data, b"-_", b"+/");
    let decoded = b64_decode(&data, false)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.b32encode(s)` — RFC 4648 base32.
fn call_b32encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B32EncodeArgs { s } = B32EncodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let encoded = b32_encode(memoryview_input(s, vm)?.as_ref(), B32_ALPHABET);
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.b32decode(s, casefold=False, map01=None)`.
///
/// `map01` names the letter that the digit `1` maps to; `0` always maps to
/// `O`. Both translations happen before the optional case folding.
fn call_b32decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B32DecodeArgs { s, casefold, map01 } = B32DecodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    defer_drop!(casefold, vm);
    defer_drop!(map01, vm);

    let mut data = decode_input(s, vm)?.into_owned();
    if !data.len().is_multiple_of(8) {
        return Err(incorrect_padding());
    }
    if !matches!(map01, Value::None) {
        let map = decode_input(map01, vm)?.into_owned();
        assert_len_bytes(&map, 1)?;
        translate(&mut data, b"01", &[b'O', map[0]]);
    }
    let decoded = b32_decode(&data, B32_ALPHABET, casefold.py_bool(vm)?)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.b32hexencode(s)` — base32 over the extended-hex alphabet.
fn call_b32hexencode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B32HexEncodeArgs { s } = B32HexEncodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let encoded = b32_encode(memoryview_input(s, vm)?.as_ref(), B32HEX_ALPHABET);
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.b32hexdecode(s, casefold=False)` — no `map01`, since `0` and `1`
/// are themselves digits in the extended-hex alphabet.
fn call_b32hexdecode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B32HexDecodeArgs { s, casefold } = B32HexDecodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    defer_drop!(casefold, vm);

    let data = decode_input(s, vm)?.into_owned();
    if !data.len().is_multiple_of(8) {
        return Err(incorrect_padding());
    }
    let decoded = b32_decode(&data, B32HEX_ALPHABET, casefold.py_bool(vm)?)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.b16encode(s)` — uppercase hex.
fn call_b16encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B16EncodeArgs { s } = B16EncodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let encoded = b16_encode(encode_input(s, vm)?.as_ref());
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.b16decode(s, casefold=False)` — rejects lowercase hex unless
/// `casefold` is set, as CPython does.
fn call_b16decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B16DecodeArgs { s, casefold } = B16DecodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    defer_drop!(casefold, vm);

    let data = decode_input(s, vm)?.into_owned();
    let decoded = b16_decode(&data, casefold.py_bool(vm)?)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.encodebytes(s)` — base64 split into 76-character lines, each
/// terminated by a newline (the MIME convention).
fn call_encodebytes(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let EncodebytesArgs { s } = EncodebytesArgs::from_args(args, vm)?;
    defer_drop!(s, vm);

    let data = input_type_check(s, vm)?;
    let mut encoded = Vec::new();
    for chunk in data.chunks(usize::from(MAX_BIN_SIZE)) {
        encoded.extend_from_slice(&b64_encode(chunk));
        encoded.push(b'\n');
    }
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.decodebytes(s)` — the inverse of [`call_encodebytes`]. Unlike
/// `b64decode` it takes bytes only, never a `str`.
fn call_decodebytes(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DecodebytesArgs { s } = DecodebytesArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let decoded = b64_decode(input_type_check(s, vm)?.as_ref(), false)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.b85encode(b, pad=False)` — RFC 1924 base85.
///
/// `pad` keeps the characters that encode the zero-padding of a final short
/// group, so the output length is always a multiple of five.
fn call_b85encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B85EncodeArgs { b, pad } = B85EncodeArgs::from_args(args, vm)?;
    defer_drop!(b, vm);
    defer_drop!(pad, vm);

    let pad = pad.py_bool(vm)?;
    let encoded = b85_encode(memoryview_input(b, vm)?.as_ref(), B85_ALPHABET, pad);
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.b85decode(b)` — the inverse of [`call_b85encode`].
fn call_b85decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let B85DecodeArgs { b } = B85DecodeArgs::from_args(args, vm)?;
    defer_drop!(b, vm);

    let decoded = b85_decode(decode_input(b, vm)?.as_ref(), B85_ALPHABET, "base85")?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.z85encode(s)` — base85 over the shell-safe Z85 alphabet.
fn call_z85encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let Z85EncodeArgs { s } = Z85EncodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);

    let encoded = b85_encode(memoryview_input(s, vm)?.as_ref(), Z85_ALPHABET, false);
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.z85decode(s)` — the inverse of [`call_z85encode`].
fn call_z85decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let Z85DecodeArgs { s } = Z85DecodeArgs::from_args(args, vm)?;
    defer_drop!(s, vm);

    let decoded = b85_decode(decode_input(s, vm)?.as_ref(), Z85_ALPHABET, "z85")?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// `base64.a85encode(b, *, foldspaces=False, wrapcol=0, pad=False, adobe=False)`
/// — Ascii85, the btoa/PostScript dialect of base85.
///
/// Zero folding to `z` is always on and `foldspaces` adds `y` for four spaces;
/// `adobe` frames the result in `<~`/`~>`, and `wrapcol` breaks it into lines.
fn call_a85encode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let A85EncodeArgs {
        b,
        foldspaces,
        wrapcol,
        pad,
        adobe,
    } = A85EncodeArgs::from_args(args, vm)?;
    defer_drop!(b, vm);
    defer_drop!(foldspaces, vm);
    defer_drop!(wrapcol, vm);
    defer_drop!(pad, vm);
    defer_drop!(adobe, vm);

    // CPython coerces the input before any flag and reaches `pad` only when the
    // length needs padding. Which flag raises first is unobservable while
    // `__bool__` goes undispatched (`limitations/classes.md`), but this order
    // holds once it lands. Owned as `tobytes()` is: a dispatched `__bool__`
    // re-enters the interpreter and could mutate a `bytearray`.
    let data = memoryview_input(b, vm)?.into_owned();
    let fold = foldspaces.py_bool(vm)?;
    let keep_padding = data.len() % 4 != 0 && pad.py_bool(vm)?;
    let adobe = adobe.py_bool(vm)?;
    let mut encoded = a85_encode(&data, keep_padding, fold);

    if adobe {
        encoded.splice(0..0, *A85_ADOBE_START);
    }
    // CPython only consults the width when it is truthy, and wraps the framed
    // result — so the opening marker counts towards the first line.
    if wrapcol.py_bool(vm)? {
        let width = a85_wrapcol(wrapcol, if adobe { 2 } else { 1 }, vm)?;
        encoded = a85_wrap(&encoded, width, adobe);
    }
    if adobe {
        encoded.extend_from_slice(A85_ADOBE_END);
    }
    Ok(allocate_bytes(encoded, vm.heap))
}

/// `base64.a85decode(b, *, foldspaces=False, adobe=False, ignorechars=b' \t\n\r\v')`
/// — the inverse of [`call_a85encode`].
///
/// `foldspaces` must be set to the same value the encoder used, since `y` is
/// otherwise not a digit at all.
fn call_a85decode(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let A85DecodeArgs {
        b,
        foldspaces,
        adobe,
        ignorechars,
    } = A85DecodeArgs::from_args(args, vm)?;
    defer_drop!(b, vm);
    defer_drop!(foldspaces, vm);
    defer_drop!(adobe, vm);
    defer_drop!(ignorechars, vm);

    // Owned so the decode loop can re-enter the interpreter for a caller-given
    // `ignorechars`, which the borrowed input would rule out.
    let data = decode_input(b, vm)?.into_owned();
    let framed = a85_strip_adobe(&data, adobe.py_bool(vm)?)?;
    let fold = foldspaces.py_bool(vm)?;
    let ignore = match ignorechars.as_ref() {
        Some(value) => IgnoreChars::Given(value),
        None => IgnoreChars::Default,
    };

    let decoded = a85_decode(framed, fold, &ignore, vm)?;
    Ok(allocate_bytes(decoded, vm.heap))
}

/// Argument shape for `b64encode(s, altchars=None)`.
///
/// Fields stay raw `Value` throughout this module: CPython's `def` binding
/// never type-checks, so the body's coercion order is what produces the
/// observable errors.
#[derive(FromArgs)]
#[from_args(name = "b64encode", style = def)]
struct B64EncodeArgs {
    s: Value,
    #[from_args(default = Value::None)]
    altchars: Value,
}

/// Argument shape for `b64decode(s, altchars=None, validate=False)`.
///
/// `validate` is a raw `Value` truth-tested in the body — CPython forwards it
/// to `binascii.a2b_base64(strict_mode=...)`, which accepts any object.
#[derive(FromArgs)]
#[from_args(name = "b64decode", style = def)]
struct B64DecodeArgs {
    s: Value,
    #[from_args(default = Value::None)]
    altchars: Value,
    #[from_args(default = Value::Bool(false))]
    validate: Value,
}

/// Argument shape for `b32decode(s, casefold=False, map01=None)` — see
/// [`B64DecodeArgs`] for why the flags are raw `Value`s.
#[derive(FromArgs)]
#[from_args(name = "b32decode", style = def)]
struct B32DecodeArgs {
    s: Value,
    #[from_args(default = Value::Bool(false))]
    casefold: Value,
    #[from_args(default = Value::None)]
    map01: Value,
}

/// Argument shape for `b32hexdecode(s, casefold=False)`.
#[derive(FromArgs)]
#[from_args(name = "b32hexdecode", style = def)]
struct B32HexDecodeArgs {
    s: Value,
    #[from_args(default = Value::Bool(false))]
    casefold: Value,
}

/// Argument shape for `b16decode(s, casefold=False)`.
#[derive(FromArgs)]
#[from_args(name = "b16decode", style = def)]
struct B16DecodeArgs {
    s: Value,
    #[from_args(default = Value::Bool(false))]
    casefold: Value,
}

/// Argument shape for `b85encode(b, pad=False)` — note the parameter is `b`,
/// not the `s` the rest of the module uses, which signature errors report.
#[derive(FromArgs)]
#[from_args(name = "b85encode", style = def)]
struct B85EncodeArgs {
    b: Value,
    #[from_args(default = Value::Bool(false))]
    pad: Value,
}

/// Argument shape for `b85decode(b)`.
#[derive(FromArgs)]
#[from_args(name = "b85decode", style = def)]
struct B85DecodeArgs {
    b: Value,
}

/// Argument shape for
/// `a85encode(b, *, foldspaces=False, wrapcol=0, pad=False, adobe=False)`.
#[derive(FromArgs)]
#[from_args(name = "a85encode", style = def)]
struct A85EncodeArgs {
    b: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    foldspaces: Value,
    #[from_args(kw_only, default = Value::Int(0))]
    wrapcol: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    pad: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    adobe: Value,
}

/// Argument shape for
/// `a85decode(b, *, foldspaces=False, adobe=False, ignorechars=b' \t\n\r\v')`.
///
/// `ignorechars` is absent rather than defaulted: CPython tests membership in
/// whatever object was passed, so an explicit argument takes a different path
/// from the built-in set — see [`IgnoreChars`].
#[derive(FromArgs)]
#[from_args(name = "a85decode", style = def)]
struct A85DecodeArgs {
    b: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    foldspaces: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    adobe: Value,
    #[from_args(kw_only, default)]
    ignorechars: Option<Value>,
}

/// Declares the argument struct for one of the module's `f(s)` functions.
///
/// These differ only in the name reported by signature errors
/// (`b32encode() missing 1 required positional argument: 's'`), which the
/// derive bakes in, so a shared struct cannot serve them.
macro_rules! single_arg_struct {
    ($struct_name:ident, $py_name:literal) => {
        #[derive(FromArgs)]
        #[from_args(name = $py_name, style = def)]
        struct $struct_name {
            s: Value,
        }
    };
}

single_arg_struct!(StandardB64EncodeArgs, "standard_b64encode");
single_arg_struct!(StandardB64DecodeArgs, "standard_b64decode");
single_arg_struct!(UrlsafeB64EncodeArgs, "urlsafe_b64encode");
single_arg_struct!(UrlsafeB64DecodeArgs, "urlsafe_b64decode");
single_arg_struct!(B32EncodeArgs, "b32encode");
single_arg_struct!(B32HexEncodeArgs, "b32hexencode");
single_arg_struct!(B16EncodeArgs, "b16encode");
single_arg_struct!(EncodebytesArgs, "encodebytes");
single_arg_struct!(DecodebytesArgs, "decodebytes");
single_arg_struct!(Z85EncodeArgs, "z85encode");
single_arg_struct!(Z85DecodeArgs, "z85decode");

/// Encodes bytes as standard base64 with `=` padding.
pub(super) fn b64_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = usize::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |b| usize::from(*b));
        let b2 = chunk.get(2).map_or(0, |b| usize::from(*b));
        out.push(B64_ALPHABET[b0 >> 2]);
        out.push(B64_ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)]);
        out.push(if chunk.len() > 1 {
            B64_ALPHABET[((b1 & 0x0f) << 2) | (b2 >> 6)]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 { B64_ALPHABET[b2 & 0x3f] } else { b'=' });
    }
    out
}

/// Decodes base64, mirroring `binascii.a2b_base64`'s two modes.
///
/// Non-strict decoding skips bytes outside the alphabet, and padding only
/// *permits* a quad to end rather than stopping the scan, so
/// `b64decode(b'YQ==YQ==')` is three bytes, not two `a`s. The state machine
/// transcribes CPython's, since which error an input produces depends on
/// exactly where CPython gives up.
pub(super) fn b64_decode(data: &[u8], strict: bool) -> RunResult<Vec<u8>> {
    if strict && data.first() == Some(&b'=') {
        return Err(binascii_error("Leading padding not allowed"));
    }

    let mut out: Vec<u8> = Vec::with_capacity(data.len().div_ceil(4) * 3);
    let mut quad_pos = 0u8;
    let mut leftchar = 0u8;
    let mut pads = 0u8;
    let mut padding_started = false;
    // Whether padding has closed the quad currently in progress, which makes a
    // non-zero `quad_pos` legal at end of input.
    let mut quad_closed = false;

    for byte in data {
        if *byte == b'=' {
            if strict {
                // A pad is only ever legal two or three characters into an
                // open quad. One character in, CPython reports the same
                // "1 more than a multiple of 4" error it would at end of input.
                if quad_closed || quad_pos == 0 {
                    return Err(binascii_error("Excess padding not allowed"));
                } else if quad_pos == 1 {
                    return Err(invalid_data_characters(out.len()));
                }
            }
            padding_started = true;
            // Only count pads while the quad is open: once closed, further `=`
            // bytes are inert, and counting them would overflow these `u8`s.
            if quad_pos >= 2 && !quad_closed {
                pads += 1;
                if quad_pos + pads >= 4 {
                    quad_closed = true;
                }
            }
            continue;
        }

        let Some(sextet) = b64_value(*byte) else {
            if strict {
                return Err(binascii_error("Only base64 data is allowed"));
            }
            continue;
        };
        if strict {
            if quad_closed {
                return Err(binascii_error("Excess data after padding"));
            } else if padding_started {
                return Err(binascii_error("Discontinuous padding not allowed"));
            }
        }
        pads = 0;
        quad_closed = false;

        match quad_pos {
            0 => {
                quad_pos = 1;
                leftchar = sextet;
            }
            1 => {
                quad_pos = 2;
                out.push((leftchar << 2) | (sextet >> 4));
                leftchar = sextet & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((leftchar << 4) | (sextet >> 2));
                leftchar = sextet & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((leftchar << 6) | sextet);
                leftchar = 0;
            }
        }
    }

    if quad_pos == 0 || quad_closed {
        Ok(out)
    } else if quad_pos == 1 {
        Err(invalid_data_characters(out.len()))
    } else {
        Err(incorrect_padding())
    }
}

/// The error for a quad holding a single character, which no encoder can
/// produce. The count is of decoded characters, derived from the output so far,
/// not of input bytes — junk and padding never reach it.
fn invalid_data_characters(decoded_len: usize) -> RunError {
    let data_chars = decoded_len / 3 * 4 + 1;
    binascii_error(format!(
        "Invalid base64-encoded string: number of data characters ({data_chars}) cannot be 1 more than a multiple of 4"
    ))
}

/// Maps a base64 character to its 6-bit value.
fn b64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Encodes bytes as base32 over `alphabet`, padding the final group to eight
/// characters with `=`.
fn b32_encode(data: &[u8], alphabet: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().div_ceil(5) * 8);
    for chunk in data.chunks(5) {
        let mut acc: u64 = 0;
        for i in 0..5 {
            acc = (acc << 8) | u64::from(chunk.get(i).copied().unwrap_or(0));
        }
        // Characters carrying data; the remaining 8 - encoded_len are pads.
        let encoded_len = match chunk.len() {
            1 => 2,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => 8,
        };
        for i in 0..8 {
            out.push(if i < encoded_len {
                let shift = 35 - i * 5;
                alphabet[usize::try_from((acc >> shift) & 0x1f).expect("5 bits fit usize")]
            } else {
                b'='
            });
        }
    }
    out
}

/// Decodes base32 over `alphabet`.
///
/// Assumes the caller has already checked that the length is a multiple of
/// eight and applied any `map01` translation — CPython performs both before
/// this point and the resulting error ordering is observable.
fn b32_decode(data: &[u8], alphabet: &[u8; 32], casefold: bool) -> RunResult<Vec<u8>> {
    let folded: Cow<'_, [u8]> = if casefold {
        Cow::Owned(data.to_ascii_uppercase())
    } else {
        Cow::Borrowed(data)
    };
    let stripped = {
        let mut end = folded.len();
        while end > 0 && folded[end - 1] == b'=' {
            end -= 1;
        }
        &folded[..end]
    };
    let pad_chars = folded.len() - stripped.len();

    let mut out = Vec::with_capacity(stripped.len() / 8 * 5 + 5);
    let mut last_acc: u64 = 0;
    for group in stripped.chunks(8) {
        let mut acc: u64 = 0;
        for byte in group {
            let Some(value) = alphabet.iter().position(|c| c == byte) else {
                return Err(binascii_error("Non-base32 digit found"));
            };
            acc = (acc << 5) | u64::try_from(value).expect("alphabet index fits u64");
        }
        last_acc = acc;
        // A short final group is still written as five bytes here and trimmed
        // below, exactly as CPython's `decoded[-5:] = last[:leftover]` does.
        out.extend_from_slice(&acc.to_be_bytes()[3..]);
    }

    // 0, 1, 3, 4 and 6 are the pad counts a base32 encoder can emit.
    if !matches!(pad_chars, 0 | 1 | 3 | 4 | 6) {
        return Err(incorrect_padding());
    }
    if pad_chars > 0 && !out.is_empty() {
        let shifted = last_acc << (5 * pad_chars);
        let leftover = (43 - 5 * pad_chars) / 8;
        let tail = shifted.to_be_bytes();
        out.truncate(out.len() - 5);
        out.extend_from_slice(&tail[3..3 + leftover]);
    }
    Ok(out)
}

/// Encodes bytes as uppercase hex (`b16encode`).
fn b16_encode(data: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(data.len() * 2);
    for byte in data {
        out.push(HEX[usize::from(byte >> 4)]);
        out.push(HEX[usize::from(byte & 0x0f)]);
    }
    out
}

/// Decodes uppercase hex, folding case first when asked.
///
/// CPython screens the whole input for non-`[0-9A-F]` bytes before
/// `binascii.unhexlify` sees it, so a bad digit outranks an odd length.
fn b16_decode(data: &[u8], casefold: bool) -> RunResult<Vec<u8>> {
    let folded: Cow<'_, [u8]> = if casefold {
        Cow::Owned(data.to_ascii_uppercase())
    } else {
        Cow::Borrowed(data)
    };
    if folded.iter().any(|b| !matches!(b, b'0'..=b'9' | b'A'..=b'F')) {
        return Err(binascii_error("Non-base16 digit found"));
    }
    if !folded.len().is_multiple_of(2) {
        return Err(binascii_error("Odd-length string"));
    }
    Ok(folded
        .chunks(2)
        .map(|pair| (hex_value(pair[0]) << 4) | hex_value(pair[1]))
        .collect())
}

/// Maps a validated uppercase hex digit to its value.
fn hex_value(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        byte - b'0'
    } else {
        byte - b'A' + 10
    }
}

/// Encodes bytes as base85 over `alphabet`, five characters per four bytes.
///
/// A short final group is zero-padded to four bytes; unless `pad` is set, the
/// characters those padding bytes produced are dropped again, so the output
/// length tracks the input's.
fn b85_encode(data: &[u8], alphabet: &[u8; 85], pad: bool) -> Vec<u8> {
    let padding = (4 - data.len() % 4) % 4;
    let mut out = Vec::with_capacity(data.len().div_ceil(4) * 5);

    for chunk in data.chunks(4) {
        let mut word: u32 = 0;
        for i in 0..4 {
            word = (word << 8) | u32::from(chunk.get(i).copied().unwrap_or(0));
        }
        // Five base-85 digits, most significant first.
        let mut digits = [0u8; 5];
        for slot in digits.iter_mut().rev() {
            *slot = alphabet[usize::try_from(word % 85).expect("remainder below 85")];
            word /= 85;
        }
        out.extend_from_slice(&digits);
    }

    if !pad {
        out.truncate(out.len() - padding);
    }
    out
}

/// Decodes base85 over `alphabet`, with `codec` naming the scheme in errors.
///
/// A trailing partial group is completed with the alphabet's highest character
/// and the extra bytes trimmed off, so any input length decodes. Unlike the
/// other decoders here, failures are plain `ValueError`s, as CPython's are.
fn b85_decode(data: &[u8], alphabet: &[u8; 85], codec: &str) -> RunResult<Vec<u8>> {
    let table = base85_table(alphabet);
    let padding = (5 - data.len() % 5) % 5;
    let mut out = Vec::with_capacity(data.len().div_ceil(5) * 4);

    for (chunk_index, chunk) in data.chunks(5).enumerate() {
        let start = chunk_index * 5;
        let mut acc: u64 = 0;
        for offset in 0..5 {
            // Positions past the end are the virtual padding: the top digit.
            let value = match chunk.get(offset) {
                Some(byte) => table[usize::from(*byte)].ok_or_else(|| {
                    codec_value_error(format!("bad {codec} character at position {}", start + offset))
                })?,
                None => 84,
            };
            acc = acc * 85 + u64::from(value);
        }
        let word = u32::try_from(acc)
            .map_err(|_| codec_value_error(format!("{codec} overflow in hunk starting at byte {start}")))?;
        out.extend_from_slice(&word.to_be_bytes());
    }

    out.truncate(out.len() - padding);
    Ok(out)
}

/// Reverse lookup for a base85 alphabet: index by byte, `None` outside it.
fn base85_table(alphabet: &[u8; 85]) -> [Option<u8>; 256] {
    let mut table = [None; 256];
    for (value, byte) in alphabet.iter().enumerate() {
        table[usize::from(*byte)] = Some(u8::try_from(value).expect("index bounded by alphabet length"));
    }
    table
}

/// Encodes bytes as Ascii85, five digits per four-byte word.
///
/// An all-zero word folds to `z` and, with `foldspaces`, four spaces fold to
/// `y`. A short final group is zero-padded to a full word and the digits that
/// padding produced are dropped again unless `pad` is set.
fn a85_encode(data: &[u8], pad: bool, foldspaces: bool) -> Vec<u8> {
    let padding = (4 - data.len() % 4) % 4;
    let mut out: Vec<u8> = Vec::with_capacity(data.len().div_ceil(4) * 5);
    // Where the final word's digits start, so the padding trim below can
    // rewrite them the way CPython rewrites `chunks[-1]`.
    let mut last_start = 0;

    for chunk in data.chunks(4) {
        last_start = out.len();
        let mut word: u32 = 0;
        for i in 0..4 {
            word = (word << 8) | u32::from(chunk.get(i).copied().unwrap_or(0));
        }
        if word == 0 {
            out.push(b'z');
        } else if foldspaces && word == 0x2020_2020 {
            out.push(b'y');
        } else {
            // Five base-85 digits, most significant first.
            let mut digits = [0u8; 5];
            for slot in digits.iter_mut().rev() {
                *slot = A85_FIRST_DIGIT + u8::try_from(word % 85).expect("remainder below 85");
                word /= 85;
            }
            out.extend_from_slice(&digits);
        }
    }

    if padding != 0 && !pad {
        // Padding cannot make a word four spaces, but it can make one zero —
        // and a folded `z` has no digits to trim, so it is expanded first.
        if out[last_start..] == *b"z" {
            out.truncate(last_start);
            out.extend_from_slice(&[A85_FIRST_DIGIT; 5]);
        }
        out.truncate(out.len() - padding);
    }
    out
}

/// Breaks encoded output into lines of at most `width` characters.
///
/// With the Adobe framing an extra newline is added when `~>` would not fit on
/// the last line, so no line ever exceeds `width`.
fn a85_wrap(result: &[u8], width: usize, adobe: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(result.len() + result.len() / width + 1);
    let mut last_len = 0;
    for (index, line) in result.chunks(width).enumerate() {
        if index > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(line);
        last_len = line.len();
    }
    if adobe && last_len + A85_ADOBE_END.len() > width {
        out.push(b'\n');
    }
    out
}

/// Resolves `max(2 if adobe else 1, wrapcol)` and the index `range` then needs.
///
/// Both of CPython's failure modes live here: a `wrapcol` that cannot be
/// ordered against an `int` fails in `max`, and one that wins the comparison
/// but is no index — a `float` of one or more — fails in `range`.
fn a85_wrapcol(wrapcol: &Value, floor: u8, vm: &mut VM<'_>) -> RunResult<usize> {
    let wider = match wrapcol.py_cmp(&Value::Int(i64::from(floor)), vm)? {
        CmpOrder::Ordered(Ordering::Greater) => true,
        // `NaN` is neither larger nor smaller, so the floor stands, as in `max`.
        CmpOrder::Ordered(_) | CmpOrder::Unordered => false,
        CmpOrder::Incomparable => {
            return Err(ExcType::type_error_ordering(">", &wrapcol.py_type_name(vm), "int"));
        }
    };
    if wider {
        a85_width(wrapcol, vm)
    } else {
        Ok(usize::from(floor))
    }
}

/// Converts a `wrapcol` that won the `max` into a line length.
///
/// CPython feeds it to `range`, which takes an arbitrary `int`, so a width
/// past the address space is not an error — everything lands on one line.
/// Having beaten a floor of 1, such a width can only be large and positive.
fn a85_width(wrapcol: &Value, vm: &mut VM<'_>) -> RunResult<usize> {
    let long = match wrapcol {
        Value::InternLongInt(_) => true,
        Value::Ref(heap_id) => matches!(vm.heap.get(*heap_id), HeapData::LongInt(_)),
        _ => false,
    };
    if long {
        Ok(usize::MAX)
    } else {
        Ok(usize::try_from(wrapcol.as_int(vm)?).unwrap_or(usize::MAX))
    }
}

/// Strips the `<~` / `~>` framing when `adobe` is set, leaving the digits.
///
/// Only the terminator is required — PDF streams carry it without the opening
/// marker — and the two overlap in `b'<~>'`, where Python's slicing yields an
/// empty body rather than failing.
fn a85_strip_adobe(data: &[u8], adobe: bool) -> RunResult<&[u8]> {
    if !adobe {
        Ok(data)
    } else if data.ends_with(A85_ADOBE_END) {
        let end = data.len() - A85_ADOBE_END.len();
        let start = if data.starts_with(A85_ADOBE_START) {
            A85_ADOBE_START.len()
        } else {
            0
        };
        Ok(data.get(start..end).unwrap_or_default())
    } else {
        Err(codec_value_error("Ascii85 encoded byte sequences must end with b'~>'"))
    }
}

/// Decodes Ascii85, transcribing CPython's byte-at-a-time loop.
///
/// The four `u`s appended to the input flush a trailing partial group, exactly
/// as CPython's `b + b'u' * 4` does; the bytes they contributed are trimmed
/// off again at the end, so a group left one digit short decodes to nothing.
fn a85_decode(data: &[u8], foldspaces: bool, ignore: &IgnoreChars<'_>, vm: &mut VM<'_>) -> RunResult<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(data.len().div_ceil(5) * 4);
    let mut acc: u64 = 0;
    let mut digits = 0u8;
    // Counts only the bytes reaching `skips`, the one arm whose cost grows with
    // a caller's `ignorechars`. Indexing by position instead would let input
    // that lands those bytes off the poll's stride skip the clock entirely.
    let mut ignored = 0usize;

    for byte in data.iter().copied().chain([A85_LAST_DIGIT; 4]) {
        if (A85_FIRST_DIGIT..=A85_LAST_DIGIT).contains(&byte) {
            acc = acc * 85 + u64::from(byte - A85_FIRST_DIGIT);
            digits += 1;
            if digits == 5 {
                // Five digits reach 85**5 - 1, half again as much as a word holds.
                let word = u32::try_from(acc).map_err(|_| codec_value_error("Ascii85 overflow"))?;
                out.extend_from_slice(&word.to_be_bytes());
                acc = 0;
                digits = 0;
            }
        } else if byte == b'z' {
            // The short forms stand for a whole word, so they cannot appear
            // part-way through one.
            if digits != 0 {
                return Err(codec_value_error("z inside Ascii85 5-tuple"));
            }
            out.extend_from_slice(&[0; 4]);
        } else if foldspaces && byte == b'y' {
            if digits != 0 {
                return Err(codec_value_error("y inside Ascii85 5-tuple"));
            }
            out.extend_from_slice(b"    ");
        } else {
            // `ignorechars` is a Python container, so this is a `py_contains`
            // per byte — linear for `bytes`. Nothing here returns to the VM's
            // dispatch checkpoint, so the loop polls the clock itself.
            vm.heap.tracker.check_time_every(ignored)?;
            ignored += 1;
            if !ignore.skips(byte, vm)? {
                return Err(codec_value_error(format!(
                    "Non-Ascii85 digit found: {}",
                    char::from(byte)
                )));
            }
        }
    }

    // Each digit still held stood for a byte the input never carried. The flush
    // guarantees a whole word was written, so there is always that much to trim.
    out.truncate(out.len() - usize::from(4 - digits));
    Ok(out)
}

/// Which bytes `a85decode` skips rather than decoding.
///
/// Splitting the default out keeps the common case a byte comparison: an
/// explicit argument is tested with Python's `in`, which re-enters the
/// interpreter and is where a `str` argument raises the way CPython's does.
enum IgnoreChars<'a> {
    /// `ignorechars` left at its `b' \t\n\r\v'` default.
    Default,
    /// The object the caller passed, whatever its type.
    Given(&'a Value),
}

impl IgnoreChars<'_> {
    /// Answers CPython's `x in ignorechars` for a byte no digit rule matched.
    fn skips(&self, byte: u8, vm: &mut VM<'_>) -> RunResult<bool> {
        match self {
            Self::Default => Ok(matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b)),
            Self::Given(value) => value.py_contains(&Value::Int(i64::from(byte)), vm),
        }
    }
}

/// Builds the plain `ValueError` the ascii85 and base85 codecs raise — not
/// `binascii.Error`, since none of them reaches `binascii`.
fn codec_value_error(message: impl Into<String>) -> RunError {
    SimpleException::new_msg(ExcType::ValueError, message.into()).into()
}

/// Rewrites `data` in place, mapping each byte of `from` to the byte at the
/// same index of `to` — CPython's `bytes.translate(bytes.maketrans(...))`.
///
/// Later entries win, matching `maketrans` when `from` repeats a byte.
fn translate(data: &mut [u8], from: &[u8], to: &[u8]) {
    let mut table: [u8; 256] = [0; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        *slot = u8::try_from(i).expect("index bounded by table length");
    }
    for (src, dst) in from.iter().zip(to) {
        table[usize::from(*src)] = *dst;
    }
    for byte in data {
        *byte = table[usize::from(*byte)];
    }
}

/// Borrows the bytes of an encoder input.
///
/// Encoders reach `binascii` without a coercion step, so a `str` fails the
/// buffer protocol with CPython's `a bytes-like object is required` wording.
pub(super) fn encode_input<'a>(value: &'a Value, vm: &'a VM<'_>) -> RunResult<Cow<'a, [u8]>> {
    encode_input_prefixed(value, vm, "")
}

/// Borrows the bytes of an encoder that coerces with `memoryview(s).tobytes()`
/// first — the base32 and base85 pair — whose `TypeError` is prefixed as a
/// result. The base64 and base16 encoders reach `binascii` directly instead.
fn memoryview_input<'a>(value: &'a Value, vm: &'a VM<'_>) -> RunResult<Cow<'a, [u8]>> {
    encode_input_prefixed(value, vm, "memoryview: ")
}

/// Shared body of the encoder coercions, where `prefix` selects the wording.
fn encode_input_prefixed<'a>(value: &'a Value, vm: &'a VM<'_>, prefix: &str) -> RunResult<Cow<'a, [u8]>> {
    value_as_bytes(value, vm).ok_or_else(|| {
        ExcType::type_error(format!(
            "{prefix}a bytes-like object is required, not '{}'",
            value.py_type_name(vm)
        ))
    })
}

/// Borrows the bytes of a decoder input — CPython's `_bytes_from_decode_data`.
///
/// Decoders additionally accept `str`, provided it is pure ASCII.
pub(super) fn decode_input<'a>(value: &'a Value, vm: &'a VM<'_>) -> RunResult<Cow<'a, [u8]>> {
    decode_input_described(value, vm, "a bytes-like object or ASCII string")
}

/// The same coercion with `expected` naming the accepted types, which
/// `binascii`'s own functions word differently from `base64`'s.
pub(super) fn decode_input_described<'a>(value: &'a Value, vm: &'a VM<'_>, expected: &str) -> RunResult<Cow<'a, [u8]>> {
    if value.is_str(vm.heap) {
        let text = value.to_str(vm)?;
        if text.is_ascii() {
            Ok(Cow::Borrowed(text.as_bytes()))
        } else {
            Err(SimpleException::new_msg(
                ExcType::ValueError,
                "string argument should contain only ASCII characters",
            )
            .into())
        }
    } else {
        value_as_bytes(value, vm).ok_or_else(|| {
            ExcType::type_error(format!(
                "argument should be {expected}, not '{}'",
                value.py_type_name(vm)
            ))
        })
    }
}

/// Borrows the bytes of an `encodebytes` / `decodebytes` argument.
///
/// These two go through `base64._input_type_check`, whose message differs from
/// both other paths.
fn input_type_check<'a>(value: &'a Value, vm: &'a VM<'_>) -> RunResult<Cow<'a, [u8]>> {
    value_as_bytes(value, vm)
        .ok_or_else(|| ExcType::type_error(format!("expected bytes-like object, not {}", value.py_type_name(vm))))
}

/// Borrows the bytes behind a `bytes` value, interned or heap-allocated.
///
/// `None` for anything else — each caller supplies its own error message.
fn value_as_bytes<'a>(value: &'a Value, vm: &'a VM<'_>) -> Option<Cow<'a, [u8]>> {
    match value {
        Value::InternBytes(bytes_id) => Some(Cow::Borrowed(vm.interns.get_bytes(*bytes_id))),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Bytes(bytes) => Some(Cow::Borrowed(bytes.as_slice())),
            _ => None,
        },
        _ => None,
    }
}

/// Reproduces `assert len(value) == expected, repr(value)` on a raw argument.
fn assert_len(value: &Value, expected: usize, vm: &mut VM<'_>) -> RunResult<()> {
    let Some(len) = value.py_len(vm) else {
        return Err(ExcType::type_error(format!(
            "object of type '{}' has no len()",
            value.py_type_name(vm)
        )));
    };
    if len == expected {
        Ok(())
    } else {
        Err(assertion_error(value, vm)?)
    }
}

/// The same assertion, but against already-coerced bytes.
///
/// The decoders coerce before asserting, so both the length and the `repr` in
/// the message come from the bytes — `b64decode(s, altchars='-')` reports
/// `b'-'`, not the `str` the caller passed.
fn assert_len_bytes(bytes: &[u8], expected: usize) -> RunResult<()> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(SimpleException::new_msg(ExcType::AssertionError, bytes_repr(bytes)).into())
    }
}

/// Builds the `AssertionError` whose message is `repr(value)`.
fn assertion_error(value: &Value, vm: &mut VM<'_>) -> RunResult<RunError> {
    let repr = value.py_repr(vm)?;
    defer_drop!(repr, vm);
    let message = repr.to_str(vm)?.to_owned();
    Ok(SimpleException::new_msg(ExcType::AssertionError, message).into())
}

/// Allocates a decoded/encoded byte string on the heap.
///
/// No resource preflight: every result here is bounded by a small constant
/// multiple of an input that the heap already accounts for.
pub(super) fn allocate_bytes(data: Vec<u8>, heap: &Heap) -> Value {
    Value::Ref(heap.allocate(HeapData::Bytes(data.into())))
}

/// `binascii.Error("Incorrect padding")` — shared by the base64 and base32
/// padding checks.
fn incorrect_padding() -> RunError {
    binascii_error("Incorrect padding")
}

/// Builds a `binascii.Error`, the `ValueError` subclass every codec failure uses.
pub(super) fn binascii_error(message: impl Into<String>) -> RunError {
    SimpleException::new_msg(ExcType::BinasciiError, message.into()).into()
}
