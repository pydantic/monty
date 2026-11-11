//! `str.translate()` and `str.maketrans()`.
//!
//! Split out of `str.rs`, which is already large. Both take generic Python
//! objects as translation tables, so unlike most `str` methods they re-enter
//! the interpreter (`__getitem__`, dict iteration) while building a result.

use std::mem;

use super::{Dict, PyTrait, str::allocate_string};
use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapReadOutput},
    py_value::PyValue,
    string_builder::StringBuilder,
    value::Value,
};

/// Argument shape for `str.maketrans(x, y=None, z=None, /)`.
///
/// CPython parses it with `PyArg_UnpackTuple` (the arity wording) after clinic-converting `y` and `z` to `str`
/// (the `bad_arg` wording). It is a static method, so `kwarg_error_name` reports `str.maketrans()`.
#[derive(FromArgs)]
#[from_args(name = "maketrans", style = unpack, bad_arg, kwarg_error_name = "str.maketrans")]
struct MaketransArgs {
    #[from_args(pos_only)]
    x: Value,
    #[from_args(pos_only, default)]
    y: Option<StrArg>,
    #[from_args(pos_only, default)]
    z: Option<StrArg>,
}

/// Implements `str.translate(table)`.
///
/// Each character `c` is looked up as `table[ord(c)]`: a `LookupError` keeps `c`, `None` deletes it,
/// an int is replaced by that code point and a str is inserted. A str mapping can amplify the input,
/// so every append goes through a [`StringBuilder`].
///
/// `text` is the caller's resource-checked copy of the receiver: the lookups can run user code that re-enters
/// the heap, so the receiver cannot stay borrowed from it.
pub(super) fn str_translate(text: &str, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    // A `METH_O` function in CPython: keywords are rejected before the argument count is checked.
    if args.has_kwargs() {
        args.drop_with(vm);
        return Err(ExcType::type_error_no_kwargs("str.translate"));
    }
    let table = args.get_one_arg("str.translate", vm.heap)?;
    defer_drop!(table, vm);
    // The builder borrows the tracker, which the lookups need `vm` for, so it only lives within one append.
    let mut out = StringBuilder::with_capacity(text.len(), &vm.heap.tracker)?.finish_raw()?;
    for (i, c) in text.chars().enumerate() {
        vm.heap.tracker.check_time_every(i)?;
        let key = Value::Int(i64::from(u32::from(c)));
        let mapped = match table.py_getitem(&key, vm) {
            Ok(mapped) => mapped,
            Err(RunError::Exc(raise)) if raise.exc.exc_type().is_subclass_of(ExcType::LookupError) => {
                out.push(c);
                continue;
            }
            Err(error) => return Err(error),
        };
        defer_drop!(mapped, vm);
        let mut builder = StringBuilder::from_existing(out, &vm.heap.tracker);
        push_mapped(&mut builder, mapped, vm)?;
        out = builder.finish_raw()?;
    }
    Ok(allocate_string(out, vm.heap))
}

/// Appends the translation of one character: nothing for `None`, a code point for an int, the text for a str.
fn push_mapped(builder: &mut StringBuilder<'_>, mapped: &Value, vm: &VM<'_>) -> RunResult<()> {
    match mapped.py_value(vm) {
        PyValue::None => {}
        PyValue::Str(text) => builder.push_str(text.as_str(vm))?,
        PyValue::Bool(flag) => builder.push(char::from(u8::from(flag)))?,
        PyValue::Int(int) => {
            // Surrogates are not `char`s, so they are rejected like out-of-range values.
            let code = int
                .to_i64(vm)
                .and_then(|n| u32::try_from(n).ok())
                .and_then(char::from_u32);
            builder.push(code.ok_or_else(ExcType::value_error_translate_range)?)?;
        }
        _ => return Err(ExcType::type_error_translate_value()),
    }
    Ok(())
}

/// Implements `str.maketrans(x[, y[, z]])`: builds a table mapping code points to code points, strings or `None`.
///
/// With one argument `x` must be a dict whose str keys (of length 1) become code points.
/// With two or three, `x` and `y` are equal-length strings mapped pairwise and `z` lists characters to delete.
pub(crate) fn str_maketrans(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let MaketransArgs { x, y, z } = MaketransArgs::from_args(args, vm)?;
    defer_drop!(x, vm);
    defer_drop!(y, vm);
    defer_drop!(z, vm);
    let pairs = match y {
        Some(y) => {
            let from = x.as_str(vm).ok_or_else(ExcType::type_error_maketrans_first_str)?;
            let to = y.as_str(vm);
            let deleted = z.as_ref().map_or("", |z| z.as_str(vm));
            code_point_pairs(from, to, deleted)?
        }
        None => dict_pairs(x, vm)?,
    };
    let dict = Dict::from_pairs(pairs, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
}

/// Pairs `from[i]` with `to[i]` as code points, then maps every character of `deleted` to `None`.
fn code_point_pairs(from: &str, to: &str, deleted: &str) -> RunResult<Vec<(Value, Value)>> {
    if from.chars().count() != to.chars().count() {
        return Err(ExcType::value_error_maketrans_lengths());
    }
    let code = |c: char| Value::Int(i64::from(u32::from(c)));
    Ok(from
        .chars()
        .zip(to.chars())
        .map(|(f, t)| (code(f), code(t)))
        .chain(deleted.chars().map(|c| (code(c), Value::None)))
        .collect())
}

/// Copies the pairs of a one-argument `maketrans` dict, turning single-character str keys into code points.
fn dict_pairs(x: &Value, vm: &mut VM<'_>) -> RunResult<Vec<(Value, Value)>> {
    let Some(HeapReadOutput::Dict(dict)) = x.read_heap(vm) else {
        return Err(ExcType::type_error_maketrans_dict());
    };
    let pairs = dict.clone_all_pairs(vm)?;
    let mut guard = DropGuard::new(pairs, vm);
    let (pairs, vm) = guard.as_parts_mut();
    for (key, _) in pairs.iter_mut() {
        let code = match key.as_str(vm) {
            Some(text) => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Some(Value::Int(i64::from(u32::from(c)))),
                    _ => return Err(ExcType::value_error_maketrans_key_length()),
                }
            }
            None if matches!(key.py_value(vm), PyValue::Int(_) | PyValue::Bool(_)) => None,
            None => return Err(ExcType::type_error_maketrans_key_type()),
        };
        if let Some(code) = code {
            mem::replace(key, code).drop_with(vm);
        }
    }
    Ok(guard.into_inner())
}
