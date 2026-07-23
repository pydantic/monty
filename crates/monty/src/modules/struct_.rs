//! Illustrative `struct` module for the module-registry spike.
//!
//! Deliberately minimal: just enough of `calcsize` / `pack` / `unpack` (a few
//! standard-size numeric codes with a `<`/`>` byte-order prefix) to prove the
//! registry seam flows end to end and produces/consumes the existing
//! `bytes`/`tuple` heap types. This is NOT a production `struct` — errors map to
//! `ValueError` and most of CPython's format language is absent. See
//! `limitations/struct.md`.

use std::mem;

use monty_types::ResourceTracker;

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropWithContext, HeapData},
    modules::registry::{ModuleDescriptor, ModuleFuncId},
    resource_checks::check_repeat_size,
    types::{Bytes, allocate_tuple},
    value::Value,
};

/// Registry descriptor for `struct`. Ids match the dispatch arms in
/// [`super::registry::call`] and are append-only.
pub(crate) const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    name: "struct",
    functions: &[
        ("calcsize", ModuleFuncId(0)),
        ("pack", ModuleFuncId(1)),
        ("unpack", ModuleFuncId(2)),
    ],
};

/// `struct.calcsize(format)` — packed byte size of `format`.
pub(crate) fn calcsize(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CalcsizeArgs { format } = CalcsizeArgs::from_args(args, vm)?;
    let parsed = parse(format.as_str(vm), vm.heap.tracker());
    format.drop_with(vm);
    let size = size_of(&parsed?.fields);
    Ok(Value::Int(
        i64::try_from(size).map_err(|_| ExcType::value_error("struct size too large"))?,
    ))
}

/// `struct.pack(format, *values)` — pack `values` into `bytes`.
pub(crate) fn pack(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PackArgs { format, values } = PackArgs::from_args(args, vm)?;
    let parsed = parse(format.as_str(vm), vm.heap.tracker());
    format.drop_with(vm);
    let result = parsed.and_then(|fmt| pack_values(&fmt, &values, vm));
    for value in values {
        value.drop_with(vm);
    }
    result
}

/// `struct.unpack(format, buffer)` — unpack `buffer` into a tuple.
pub(crate) fn unpack(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let UnpackArgs { format, buffer } = UnpackArgs::from_args(args, vm)?;
    let parsed = parse(format.as_str(vm), vm.heap.tracker());
    format.drop_with(vm);
    let result = parsed.and_then(|fmt| {
        let bytes = buffer_bytes(&buffer, vm)?;
        let size = size_of(&fmt.fields);
        if bytes.len() != size {
            return Err(ExcType::value_error(format!(
                "unpack requires a buffer of {size} bytes"
            )));
        }
        unpack_fields(&fmt, bytes, vm)
    });
    buffer.drop_with(vm);
    result
}

/// `calcsize(format, /)`.
#[derive(FromArgs)]
#[from_args(name = "calcsize", style = unpack)]
struct CalcsizeArgs {
    #[from_args(pos_only)]
    format: StrArg,
}

/// `pack(format, /, *values)`.
#[derive(FromArgs)]
#[from_args(name = "pack")]
struct PackArgs {
    #[from_args(pos_only)]
    format: StrArg,
    #[from_args(varargs)]
    values: Vec<Value>,
}

/// `unpack(format, buffer, /)`.
#[derive(FromArgs)]
#[from_args(name = "unpack", style = unpack)]
struct UnpackArgs {
    #[from_args(pos_only)]
    format: StrArg,
    #[from_args(pos_only)]
    buffer: Value,
}

/// A parsed format: byte order plus a sequence of `(code, repeat)` fields.
struct Format {
    big_endian: bool,
    fields: Vec<Field>,
}

/// One format field: a code and its repeat count.
#[derive(Clone, Copy)]
struct Field {
    code: u8,
    count: usize,
}

/// Parses a `[<>=!]` prefix followed by `[count]code` items. Only the numeric
/// codes `b B h H i I l L q f d` are supported; anything else is rejected.
fn parse(fmt: &str, tracker: &ResourceTracker) -> RunResult<Format> {
    // Each field consumes at least one code byte, so the field vector is bounded
    // by the format length; charge for it up front, since a large (already
    // tracked) format string would otherwise amplify ~size_of::<Field>()× into an
    // untracked Vec.
    check_repeat_size(mem::size_of::<Field>(), fmt.len(), tracker)?;
    let bytes = fmt.as_bytes();
    let native_big = cfg!(target_endian = "big");
    let (big_endian, mut i) = match bytes.first() {
        Some(b'<') => (false, 1),
        Some(b'>' | b'!') => (true, 1),
        Some(b'=') => (native_big, 1),
        _ => (native_big, 0),
    };
    let mut fields = Vec::new();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let mut count = 0usize;
        let mut has_count = false;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            has_count = true;
            count = count
                .checked_mul(10)
                .and_then(|c| c.checked_add(usize::from(bytes[i] - b'0')))
                .ok_or_else(|| ExcType::value_error("overflow in item count"))?;
            i += 1;
        }
        let Some(&code) = bytes.get(i) else {
            return Err(ExcType::value_error("repeat count given without format specifier"));
        };
        i += 1;
        if code_size(code).is_none() {
            return Err(ExcType::value_error("bad char in struct format"));
        }
        fields.push(Field {
            code,
            count: if has_count { count } else { 1 },
        });
    }
    Ok(Format { big_endian, fields })
}

/// Total packed size of the fields.
fn size_of(fields: &[Field]) -> usize {
    fields
        .iter()
        .map(|f| code_size(f.code).unwrap_or(0).saturating_mul(f.count))
        .fold(0, usize::saturating_add)
}

/// Total number of packed items across all fields. Saturates so a format with
/// huge repeat counts (`'9999999999999999999b9999999999999999999b'`) yields a
/// rejected size/arity rather than an arithmetic-overflow panic.
fn total_count(fields: &[Field]) -> usize {
    fields.iter().map(|f| f.count).fold(0, usize::saturating_add)
}

/// Byte width of a code, or `None` if the code is unsupported.
fn code_size(code: u8) -> Option<usize> {
    match code {
        b'b' | b'B' => Some(1),
        b'h' | b'H' => Some(2),
        b'i' | b'I' | b'l' | b'L' | b'f' => Some(4),
        b'q' | b'd' => Some(8),
        _ => None,
    }
}

/// Packs `values` per `fmt` into a `bytes` object.
fn pack_values(fmt: &Format, values: &[Value], vm: &mut VM<'_>) -> RunResult<Value> {
    let expected = total_count(&fmt.fields);
    if values.len() != expected {
        return Err(ExcType::value_error(format!(
            "pack expected {expected} items for packing (got {})",
            values.len()
        )));
    }
    let size = size_of(&fmt.fields);
    // The output size comes from the format string, which can amplify a tiny
    // input; charge the tracker before allocating the buffer.
    check_repeat_size(size, 1, vm.heap.tracker())?;
    let mut out = Vec::with_capacity(size);
    let mut index = 0;
    for field in &fmt.fields {
        for _ in 0..field.count {
            pack_one(&mut out, field.code, &values[index], fmt.big_endian)?;
            index += 1;
        }
    }
    let heap_id = vm.heap.allocate(HeapData::Bytes(Bytes::new(out)))?;
    Ok(Value::Ref(heap_id))
}

/// Packs a single value for `code` into `out`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the `f` code narrows f64 to f32 by design"
)]
fn pack_one(out: &mut Vec<u8>, code: u8, value: &Value, big_endian: bool) -> RunResult<()> {
    if code == b'f' || code == b'd' {
        let float = value_as_f64(value)?;
        match code {
            b'f' => put(out, &(float as f32).to_le_bytes(), big_endian),
            _ => put(out, &float.to_le_bytes(), big_endian),
        }
    } else {
        let int = value_as_i64(value)?;
        let (min, max) = int_range(code);
        if int < min || int > max {
            return Err(ExcType::value_error(format!(
                "'{}' format requires {min} <= number <= {max}",
                char::from(code)
            )));
        }
        let size = code_size(code).expect("validated code");
        put(out, &int.to_le_bytes()[..size], big_endian);
    }
    Ok(())
}

/// Inclusive `[min, max]` for an integer code (all fit `i64` in this subset).
fn int_range(code: u8) -> (i64, i64) {
    match code {
        b'b' => (-128, 127),
        b'B' => (0, 255),
        b'h' => (-32768, 32767),
        b'H' => (0, 65535),
        b'i' | b'l' => (-2_147_483_648, 2_147_483_647),
        b'I' | b'L' => (0, 4_294_967_295),
        _ => (i64::MIN, i64::MAX), // 'q'
    }
}

/// Unpacks `buffer` (exactly `size_of(fields)` bytes) into a tuple.
fn unpack_fields(fmt: &Format, buffer: &[u8], vm: &VM<'_>) -> RunResult<Value> {
    // Building the output materialises one `Value` per unpacked field on the Rust
    // heap before `allocate_tuple` charges anything; a buffer of 1-byte codes
    // amplifies ~size_of::<Value>()× per input byte. Charge the tracker up front
    // so the transient can't outrun the memory limit (mirrors `pack_values`).
    let count = total_count(&fmt.fields);
    check_repeat_size(mem::size_of::<Value>(), count, vm.heap.tracker())?;
    let mut items = Vec::with_capacity(count);
    let mut cursor = 0;
    for field in &fmt.fields {
        let size = code_size(field.code).expect("validated code");
        for _ in 0..field.count {
            items.push(read_one(&buffer[cursor..cursor + size], field.code, fmt.big_endian));
            cursor += size;
        }
    }
    allocate_tuple(items.into(), vm.heap).map_err(RunError::from)
}

/// Reads a single value of `code` from `bytes`.
fn read_one(bytes: &[u8], code: u8, big_endian: bool) -> Value {
    match code {
        b'f' => {
            let mut buf = [0u8; 4];
            order_into(bytes, &mut buf, big_endian);
            Value::Float(f64::from(f32::from_le_bytes(buf)))
        }
        b'd' => {
            let mut buf = [0u8; 8];
            order_into(bytes, &mut buf, big_endian);
            Value::Float(f64::from_le_bytes(buf))
        }
        _ => {
            let signed = matches!(code, b'b' | b'h' | b'i' | b'l' | b'q');
            let size = bytes.len();
            let mut buf = [0u8; 8];
            order_into(bytes, &mut buf[..size], big_endian);
            if signed && buf[size - 1] & 0x80 != 0 {
                for byte in &mut buf[size..] {
                    *byte = 0xff;
                }
            }
            Value::Int(i64::from_le_bytes(buf))
        }
    }
}

/// Writes `le` (little-endian bytes) to `out`, reversing for big-endian.
fn put(out: &mut Vec<u8>, le: &[u8], big_endian: bool) {
    if big_endian {
        out.extend(le.iter().rev());
    } else {
        out.extend_from_slice(le);
    }
}

/// Copies `src` into `dst` in little-endian order (reversing a big-endian source).
fn order_into(src: &[u8], dst: &mut [u8], big_endian: bool) {
    if big_endian {
        for (d, s) in dst.iter_mut().zip(src.iter().rev()) {
            *d = *s;
        }
    } else {
        dst.copy_from_slice(src);
    }
}

/// Reads a Python `int`/`bool` as `i64`; errors otherwise (large ints outside
/// `i64` are unsupported in this subset).
fn value_as_i64(value: &Value) -> RunResult<i64> {
    match value {
        Value::Int(i) => Ok(*i),
        Value::Bool(b) => Ok(i64::from(*b)),
        _ => Err(ExcType::value_error("required argument is not an integer")),
    }
}

/// Reads a Python `float`/`int`/`bool` as `f64`; errors otherwise.
#[expect(
    clippy::cast_precision_loss,
    reason = "int-to-float widening matches CPython's struct"
)]
fn value_as_f64(value: &Value) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        Value::Int(i) => Ok(*i as f64),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        _ => Err(ExcType::value_error("required argument is not a float")),
    }
}

/// Borrows a `bytes` buffer, erroring on any other type.
fn buffer_bytes<'a>(value: &'a Value, vm: &'a VM<'_>) -> RunResult<&'a [u8]> {
    match value {
        Value::InternBytes(id) => Ok(vm.interns.get_bytes(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(b) => Ok(b.as_slice()),
            _ => Err(ExcType::type_error("a bytes object is required")),
        },
        _ => Err(ExcType::type_error("a bytes object is required")),
    }
}
