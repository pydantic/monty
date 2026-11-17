//! `str.translate` and `str.maketrans`.
//!
//! Translation follows CPython's `_PyUnicode_TranslateCharmap` step for step, including its
//! ASCII lookup cache and its look-ahead over deleted runs, so a table whose `__getitem__` has
//! side effects is called with the same keys in the same order.

use super::{PyStr, allocate_string};
use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapReadOutput},
    intern::StaticStrings,
    py_value::PyValue,
    resource_checks::check_estimated_size,
    str_format::push_tracked,
    types::{Dict, PyTrait, Type, hash_table::TableCursor},
    value::{EitherStr, Value},
};

/// ASCII cache slot not yet looked up.
const UNCACHED: u8 = 0xff;
/// ASCII cache slot whose character the table deletes.
const DELETED: u8 = 0xfe;

/// What a translation table maps one character to.
enum Mapping {
    /// The lookup raised a `LookupError`: the character is kept.
    Unmapped,
    /// The table returned `None`.
    Delete,
    /// The table returned an int code point.
    Char(char),
    /// The table returned a str, owned here.
    Text(Value),
}

/// Implements `str.translate(table)`.
///
/// Looks each character up as `table[ord(c)]`, through `__getitem__` for any table type.
/// Output beyond the ASCII fast path grows through `push_tracked`, so a table of long
/// replacement strings hits the memory limit gracefully.
pub(super) fn str_translate<'h>(s: &PyStr<'h>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let table = args
        .reject_kwargs("str.translate", vm.heap)?
        .get_one_arg("str.translate", vm.heap)?;
    defer_drop!(table, vm);
    let len = s.as_str(vm).len();
    let (mut output, mut pos) = if s.is_ascii(vm) {
        translate_ascii(s, table, vm)?
    } else {
        (String::new(), 0)
    };

    let mut steps = 0;
    while pos < len {
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let c = char_at(s.as_str(vm), pos);
        pos += c.len_utf8();
        output = match lookup(table, c, vm)? {
            Mapping::Unmapped => push_tracked(output, c.encode_utf8(&mut [0; 4]), vm)?,
            Mapping::Char(replacement) => push_tracked(output, replacement.encode_utf8(&mut [0; 4]), vm)?,
            Mapping::Text(text) => {
                defer_drop!(text, vm);
                push_tracked(output, text.as_str(vm).expect("lookup returns str values"), vm)?
            }
            Mapping::Delete => {
                // CPython looks up the rest of a deleted run before skipping it, then looks up
                // the character that ended the run a second time on the next iteration.
                while pos < len {
                    vm.heap.tracker.check_time_every(steps)?;
                    steps += 1;
                    let c = char_at(s.as_str(vm), pos);
                    if !is_deleted(table, c, vm)? {
                        break;
                    }
                    pos += c.len_utf8();
                }
                output
            }
        };
    }
    Ok(allocate_string(output, vm.heap))
}

/// CPython's fast path for ASCII input, returning the output and the byte position it stopped at.
///
/// Results that are a single ASCII character or a deletion are cached, so each distinct
/// character is looked up once. Any other result stops the fast path at that character, which
/// the general loop then looks up again. The initial reservation is preflighted against the
/// resource tracker before allocating.
fn translate_ascii<'h>(s: &PyStr<'h>, table: &Value, vm: &mut VM<'h>) -> RunResult<(String, usize)> {
    let len = s.as_str(vm).len();
    check_estimated_size(len, &vm.heap.tracker)?;
    let mut output = String::with_capacity(len);
    let mut cache = [UNCACHED; 128];
    for pos in 0..len {
        vm.heap.tracker.check_time_every(pos)?;
        let byte = s.as_str(vm).as_bytes()[pos];
        let slot = usize::from(byte);
        if cache[slot] == UNCACHED {
            match ascii_cache_entry(table, byte, vm)? {
                Some(entry) => cache[slot] = entry,
                None => return Ok((output, pos)),
            }
        }
        if cache[slot] != DELETED {
            output.push(char::from(cache[slot]));
        }
    }
    Ok((output, len))
}

/// Looks up an ASCII character for the fast path's cache; `None` when the result is uncacheable.
fn ascii_cache_entry(table: &Value, byte: u8, vm: &mut VM<'_>) -> RunResult<Option<u8>> {
    Ok(match lookup(table, char::from(byte), vm)? {
        Mapping::Unmapped => Some(byte),
        Mapping::Delete => Some(DELETED),
        Mapping::Char(c) => ascii_byte(c),
        Mapping::Text(text) => {
            defer_drop!(text, vm);
            let mut chars = text.as_str(vm).expect("lookup returns str values").chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => ascii_byte(c),
                _ => None,
            }
        }
    })
}

/// The byte for an ASCII character, or `None` for anything wider.
fn ascii_byte(c: char) -> Option<u8> {
    u8::try_from(c).ok().filter(u8::is_ascii)
}

/// Whether the table deletes `c`, for the look-ahead over a deleted run.
fn is_deleted(table: &Value, c: char, vm: &mut VM<'_>) -> RunResult<bool> {
    Ok(match lookup(table, c, vm)? {
        Mapping::Delete => true,
        Mapping::Text(text) => {
            text.drop_with(vm);
            false
        }
        Mapping::Unmapped | Mapping::Char(_) => false,
    })
}

/// Looks `c` up in the table and classifies the result as CPython's `charmaptranslate_lookup` does.
fn lookup(table: &Value, c: char, vm: &mut VM<'_>) -> RunResult<Mapping> {
    let Some(item) = get_item(table, c, vm)? else {
        return Ok(Mapping::Unmapped);
    };
    let mapping = match item.py_value(vm) {
        PyValue::None => Ok(Mapping::Delete),
        PyValue::Bool(b) => Ok(Mapping::Char(char::from(u8::from(b)))),
        PyValue::Int(int) => int
            .to_i64(vm)
            .and_then(|n| u32::try_from(n).ok())
            .and_then(char::from_u32)
            .map(Mapping::Char)
            .ok_or_else(ExcType::value_error_character_mapping_range),
        PyValue::Str(_) => return Ok(Mapping::Text(item)),
        _ => Err(ExcType::type_error_character_mapping_type()),
    };
    item.drop_with(vm);
    mapping
}

/// Returns `table[ord(c)]`, or `None` when the lookup raises a `LookupError`.
///
/// A plain dict is probed directly, skipping the `KeyError` a miss would otherwise build.
fn get_item(table: &Value, c: char, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    let key = Value::Int(i64::from(u32::from(c)));
    let result = if let Value::Ref(id) = table
        && let HeapReadOutput::Dict(dict) = vm.heap.read(*id)
        && dict.get(vm.heap).kind_type() == Type::Dict
    {
        dict.dict_get(&key, vm)
    } else {
        table.py_getitem(&key, vm).map(Some)
    };
    match result {
        Ok(item) => Ok(item),
        Err(RunError::Exc(raise)) if Type::from(raise.exc.exc_type()).is_subclass_of(Type::LookupError) => Ok(None),
        Err(err) => Err(err),
    }
}

/// The character starting at byte offset `pos`, which must be a char boundary below the length.
fn char_at(text: &str, pos: usize) -> char {
    text[pos..].chars().next().expect("position is inside the string")
}

/// `str.maketrans(...)` called on the type.
pub(crate) fn str_class_method(
    attr: &EitherStr,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> Result<RunResult<CallResult>, ArgValues> {
    if attr.static_string(vm.interns) == Some(StaticStrings::Maketrans) {
        Ok(str_maketrans(args, vm).map(CallResult::Value))
    } else {
        Err(args)
    }
}

/// Argument shape for `str.maketrans(x, y=..., z=..., /)`.
///
/// CPython checks the arity with `_PyArg_CheckPositional` (`maketrans expected at most 3
/// arguments, got 4`) and converts `y` and `z` before the body runs, so their type errors
/// come before any check on `x`.
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

/// Implements the `str.maketrans` static method, building a new dict with int keys.
///
/// - `maketrans(dict)` converts each 1-character str key to its code point and keeps int keys.
/// - `maketrans(x, y[, z])` maps each character of `x` to the one at the same index of `y`,
///   and each character of `z` to `None`.
pub(super) fn str_maketrans(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let MaketransArgs { x, y, z } = MaketransArgs::from_args(args, vm)?;
    defer_drop!(x, vm);
    defer_drop!(y, vm);
    defer_drop!(z, vm);
    let mut table_guard = DropGuard::new(Dict::new(), vm);
    let (table, vm) = table_guard.as_parts_mut();
    match y {
        Some(y) => {
            let Some(x) = PyStr::from_value(x, vm) else {
                return Err(ExcType::type_error_maketrans_first_arg());
            };
            let y = PyStr::from_value(y.as_value(), vm).expect("StrArg always holds a str");
            maketrans_from_strings(table, &x, &y, z.as_ref(), vm)?;
        }
        None => maketrans_from_dict(table, x, vm)?,
    }
    let (table, vm) = table_guard.into_parts();
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(table))))
}

/// Fills `table` from the two- and three-argument forms of `str.maketrans`.
///
/// Walks the strings by byte offset rather than with iterators, since inserting needs the VM mutably.
fn maketrans_from_strings<'h>(
    table: &mut Dict,
    x: &PyStr<'h>,
    y: &PyStr<'h>,
    z: Option<&StrArg>,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    if x.checked_char_len(vm)? != y.checked_char_len(vm)? {
        return Err(ExcType::value_error_maketrans_unequal_length());
    }
    let (mut x_pos, mut y_pos, mut step) = (0, 0, 0);
    while x_pos < x.as_str(vm).len() {
        vm.heap.tracker.check_time_every(step)?;
        step += 1;
        let from = char_at(x.as_str(vm), x_pos);
        let to = char_at(y.as_str(vm), y_pos);
        x_pos += from.len_utf8();
        y_pos += to.len_utf8();
        table.set(code_point(from), code_point(to), vm)?.drop_with(vm);
    }
    if let Some(z) = z {
        let (mut pos, mut step) = (0, 0);
        while pos < z.as_str(vm).len() {
            vm.heap.tracker.check_time_every(step)?;
            step += 1;
            let deleted = char_at(z.as_str(vm), pos);
            pos += deleted.len_utf8();
            table.set(code_point(deleted), Value::None, vm)?.drop_with(vm);
        }
    }
    Ok(())
}

/// The int key or value `str.maketrans` stores for `c`.
fn code_point(c: char) -> Value {
    Value::Int(i64::from(u32::from(c)))
}

/// Fills `table` from the one-argument form, which only accepts an exact `dict`.
fn maketrans_from_dict(table: &mut Dict, x: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let source = match x {
        Value::Ref(id) => match vm.heap.read(*id) {
            HeapReadOutput::Dict(source) if source.get(vm.heap).kind_type() == Type::Dict => source,
            _ => return Err(ExcType::type_error_maketrans_needs_dict()),
        },
        _ => return Err(ExcType::type_error_maketrans_needs_dict()),
    };
    let mut cursor = TableCursor::new(source.get(vm.heap).len());
    for index in 0.. {
        vm.heap.tracker.check_time_every(index)?;
        let Some((key, value)) = source.get(vm.heap).clone_next_pair(&mut cursor, vm.heap)? else {
            break;
        };
        defer_drop!(key, vm);
        defer_drop!(value, vm);
        let new_key = maketrans_key(key, vm)?;
        table.set(new_key, value.clone_with_heap(vm), vm)?.drop_with(vm);
    }
    Ok(())
}

/// The key `str.maketrans` stores for a dict key: a 1-character str becomes its code point
/// and an int is kept as is.
fn maketrans_key(key: &Value, vm: &VM<'_>) -> RunResult<Value> {
    if let Some(text) = key.as_str(vm) {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(code_point(c)),
            _ => Err(ExcType::value_error_maketrans_key_length()),
        }
    } else if matches!(key.py_value(vm), PyValue::Int(_) | PyValue::Bool(_)) {
        Ok(key.clone_with_heap(vm.heap))
    } else {
        Err(ExcType::type_error_maketrans_key_type())
    }
}
