use std::fmt;

use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropGuard, DropWithContext},
    string_builder::StringBuilder,
    types::{PyTrait, str::allocate_string},
    value::{EitherStr, Value},
};

const MAX_FORMAT_RECURSION: u8 = 2;

pub(crate) fn str_format(template: &str, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let FormatCallArgs { positional, keywords } = FormatCallArgs::from_args(args, vm)?;
    let arguments = FormatArguments {
        positional,
        keywords: keywords.into_iter().collect(),
    };
    defer_drop!(arguments, vm);

    let mut numbering = Numbering::default();
    let rendered = render(template, arguments, &mut numbering, MAX_FORMAT_RECURSION, vm)?;
    Ok(allocate_string(rendered, vm.heap))
}

#[derive(FromArgs)]
#[from_args(name = "format")]
struct FormatCallArgs {
    #[from_args(varargs)]
    positional: Vec<Value>,
    #[from_args(varkwargs)]
    keywords: KwargsValues,
}

struct FormatArguments {
    positional: Vec<Value>,
    keywords: Vec<(Value, Value)>,
}

impl<C: ContainsHeap> DropWithContext<C> for FormatArguments {
    fn drop_with(self, ctx: &mut C) {
        self.positional.drop_with(ctx);
        for (key, value) in self.keywords {
            key.drop_with(ctx);
            value.drop_with(ctx);
        }
    }
}

#[derive(Default)]
struct Numbering {
    mode: NumberingMode,
    next_auto: usize,
}

#[derive(Default)]
enum NumberingMode {
    #[default]
    Unknown,
    Automatic,
    Manual,
}

fn render(
    template: &str,
    arguments: &FormatArguments,
    numbering: &mut Numbering,
    recursion_remaining: u8,
    vm: &mut VM<'_>,
) -> RunResult<String> {
    let bytes = template.as_bytes();
    let mut output = String::new();
    let mut literal_start = 0;
    let mut index = 0;

    while index < bytes.len() {
        vm.heap.tracker.check_time_every(index)?;
        match bytes[index] {
            b'{' => {
                output = push_tracked(output, &template[literal_start..index], vm)?;
                if bytes.get(index + 1) == Some(&b'{') {
                    output = push_tracked(output, "{", vm)?;
                    index += 2;
                } else {
                    if index + 1 == bytes.len() {
                        return Err(value_error("Single '{' encountered in format string"));
                    }
                    if recursion_remaining == 0 {
                        return Err(value_error("Max string recursion exceeded"));
                    }
                    let (formatted, next) =
                        render_field(template, index + 1, arguments, numbering, recursion_remaining, vm)?;
                    output = push_tracked(output, &formatted, vm)?;
                    index = next;
                }
                literal_start = index;
            }
            b'}' => {
                output = push_tracked(output, &template[literal_start..index], vm)?;
                if bytes.get(index + 1) == Some(&b'}') {
                    output = push_tracked(output, "}", vm)?;
                    index += 2;
                    literal_start = index;
                } else {
                    return Err(value_error("Single '}' encountered in format string"));
                }
            }
            _ => index += 1,
        }
    }

    push_tracked(output, &template[literal_start..], vm)
}

fn render_field(
    template: &str,
    start: usize,
    arguments: &FormatArguments,
    numbering: &mut Numbering,
    recursion_remaining: u8,
    vm: &mut VM<'_>,
) -> RunResult<(String, usize)> {
    let (field_end, delimiter) = find_field_end(template, start, vm)?;
    let mut cursor = field_end;
    let conversion = if delimiter == b'!' {
        let conversion_start = cursor + 1;
        let Some(conversion) = template[conversion_start..].chars().next() else {
            return Err(value_error("end of string while looking for conversion specifier"));
        };
        cursor = conversion_start + conversion.len_utf8();
        match template.as_bytes().get(cursor) {
            Some(b':' | b'}') => {}
            None => return Err(value_error("unmatched '{' in format spec")),
            _ => return Err(value_error("expected ':' after conversion specifier")),
        }
        Some(conversion)
    } else {
        None
    };

    let delimiter = *template
        .as_bytes()
        .get(cursor)
        .ok_or_else(|| value_error("expected '}' before end of string"))?;
    let spec_range = match delimiter {
        b'}' => None,
        b':' => {
            let spec_start = cursor + 1;
            let spec_end = find_spec_end(template, spec_start, vm)?;
            Some((spec_start, spec_end))
        }
        _ => return Err(value_error("expected '}' before end of string")),
    };

    let value = resolve_field(&template[start..field_end], arguments, numbering, vm)?;
    defer_drop!(value, vm);
    let conversion = match conversion {
        None => None,
        Some('s') => Some(1),
        Some('r') => Some(2),
        Some('a') => Some(3),
        Some(other) => {
            let other = if other.is_ascii_graphic() {
                other.to_string()
            } else {
                format!("\\x{:x}", u32::from(other))
            };
            return Err(value_error(format!("Unknown conversion specifier {other}")));
        }
    };
    let converted = conversion
        .map(|conversion| vm.convert_value(value, conversion))
        .transpose()?;

    if let Some((spec_start, spec_end)) = spec_range {
        let spec = render(
            &template[spec_start..spec_end],
            arguments,
            numbering,
            recursion_remaining - 1,
            vm,
        )?;
        let formatted = if let Some(converted) = &converted {
            vm.format_runtime_string(converted, &spec)?
        } else {
            vm.format_runtime_value(value, 0, Some(&spec))?
        };
        Ok((formatted, spec_end + 1))
    } else if let Some(converted) = converted {
        Ok((converted, cursor + 1))
    } else {
        vm.format_runtime_value(value, 0, None)
            .map(|formatted| (formatted, cursor + 1))
    }
}

fn find_field_end(template: &str, start: usize, vm: &VM<'_>) -> RunResult<(usize, u8)> {
    let bytes = template.as_bytes();
    let mut index = start;
    let mut in_item = false;

    while index < bytes.len() {
        vm.heap.tracker.check_time_every(index)?;
        match bytes[index] {
            b'[' if !in_item => in_item = true,
            b']' if in_item => in_item = false,
            b'{' if !in_item => return Err(value_error("unexpected '{' in field name")),
            delimiter @ (b'!' | b':' | b'}') if !in_item => return Ok((index, delimiter)),
            _ => {}
        }
        index += 1;
    }
    Err(value_error("expected '}' before end of string"))
}

fn find_spec_end(template: &str, start: usize, vm: &VM<'_>) -> RunResult<usize> {
    let bytes = template.as_bytes();
    let mut index = start;
    let mut nesting = 0usize;

    while index < bytes.len() {
        vm.heap.tracker.check_time_every(index)?;
        match bytes[index] {
            b'{' => nesting += 1,
            b'}' if nesting == 0 => return Ok(index),
            b'}' => nesting -= 1,
            _ => {}
        }
        index += 1;
    }
    Err(value_error("unmatched '{' in format spec"))
}

fn resolve_field(
    field: &str,
    arguments: &FormatArguments,
    numbering: &mut Numbering,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let first_end = field
        .as_bytes()
        .iter()
        .position(|byte| matches!(byte, b'.' | b'['))
        .unwrap_or(field.len());
    let value = resolve_first(&field[..first_end], arguments, numbering, vm)?;
    let mut value = DropGuard::new(value, vm);
    let mut cursor = first_end;

    while cursor < field.len() {
        match field.as_bytes()[cursor] {
            b'.' => {
                let start = cursor + 1;
                let end = field.as_bytes()[start..]
                    .iter()
                    .position(|byte| matches!(byte, b'.' | b'['))
                    .map_or(field.len(), |offset| start + offset);
                if start == end {
                    return Err(value_error("Empty attribute in format string"));
                }
                let (current, vm) = value.into_parts();
                let next = resolve_attribute(current, &field[start..end], vm)?;
                value = DropGuard::new(next, vm);
                cursor = end;
            }
            b'[' => {
                let start = cursor + 1;
                let Some(offset) = field[start..].find(']') else {
                    return Err(value_error("expected '}' before end of string"));
                };
                let end = start + offset;
                if start == end {
                    return Err(value_error("Empty attribute in format string"));
                }
                let (current, vm) = value.into_parts();
                let next = resolve_item(current, &field[start..end], vm)?;
                value = DropGuard::new(next, vm);
                cursor = end + 1;
                if cursor < field.len() && !matches!(field.as_bytes()[cursor], b'.' | b'[') {
                    return Err(value_error("Only '.' or '[' may follow ']' in format field specifier"));
                }
            }
            _ => {
                return Err(value_error("expected '}' before end of string"));
            }
        }
    }
    Ok(value.into_inner())
}

fn resolve_first(
    field: &str,
    arguments: &FormatArguments,
    numbering: &mut Numbering,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    if field.is_empty() {
        return resolve_auto(arguments, numbering, vm);
    }

    match decimal_index(field, vm)? {
        Some(index) => {
            if matches!(numbering.mode, NumberingMode::Automatic) {
                return Err(value_error(
                    "cannot switch from automatic field numbering to manual field specification",
                ));
            }
            numbering.mode = NumberingMode::Manual;
            resolve_positional(index, arguments, vm)
        }
        None => resolve_keyword(field, arguments, vm),
    }
}

fn resolve_auto(arguments: &FormatArguments, numbering: &mut Numbering, vm: &VM<'_>) -> RunResult<Value> {
    if matches!(numbering.mode, NumberingMode::Manual) {
        return Err(value_error(
            "cannot switch from manual field specification to automatic field numbering",
        ));
    }
    numbering.mode = NumberingMode::Automatic;
    let index = numbering.next_auto;
    numbering.next_auto = numbering
        .next_auto
        .checked_add(1)
        .ok_or_else(|| value_error("Too many decimal digits in format string"))?;
    resolve_positional(index, arguments, vm)
}

fn resolve_positional(index: usize, arguments: &FormatArguments, vm: &VM<'_>) -> RunResult<Value> {
    arguments
        .positional
        .get(index)
        .map(|value| value.clone_with_heap(vm.heap))
        .ok_or_else(|| {
            SimpleException::new_msg(
                ExcType::IndexError,
                format!("Replacement index {index} out of range for positional args tuple"),
            )
            .into()
        })
}

fn resolve_keyword(name: &str, arguments: &FormatArguments, vm: &mut VM<'_>) -> RunResult<Value> {
    for (index, (key, value)) in arguments.keywords.iter().enumerate() {
        vm.heap.tracker.check_time_every(index)?;
        if key.to_str(vm)? == name {
            return Ok(value.clone_with_heap(vm.heap));
        }
    }

    let key = allocate_string(name, vm.heap);
    defer_drop!(key, vm);
    Err(ExcType::key_error(key, vm))
}

fn resolve_attribute(value: Value, name: &str, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(value, vm);
    match value.py_getattr(&EitherStr::Heap(name.to_owned()), vm)? {
        CallResult::Value(value) => Ok(value),
        other => {
            other.drop_with(vm);
            Err(ExcType::not_implemented("str.format attribute access cannot suspend").into())
        }
    }
}

fn resolve_item(value: Value, item: &str, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(value, vm);
    let key = item_key(item, vm)?;
    defer_drop!(key, vm);
    value.py_getitem(key, vm)
}

fn item_key(item: &str, vm: &VM<'_>) -> RunResult<Value> {
    if let Some(index) = decimal_index(item, vm)? {
        Ok(Value::Int(
            i64::try_from(index).expect("index is bounded by isize::MAX"),
        ))
    } else {
        Ok(allocate_string(item, vm.heap))
    }
}

fn decimal_index(field: &str, vm: &VM<'_>) -> RunResult<Option<usize>> {
    let mut index = 0usize;
    for (offset, character) in field.chars().enumerate() {
        vm.heap.tracker.check_time_every(offset)?;
        let Some(digit) = decimal_digit_value(character) else {
            return Ok(None);
        };
        index = index
            .checked_mul(10)
            .and_then(|value| value.checked_add(digit as usize))
            .filter(|value| isize::try_from(*value).is_ok())
            .ok_or_else(|| value_error("Too many decimal digits in format string"))?;
    }
    Ok(Some(index))
}

fn decimal_digit_value(character: char) -> Option<u32> {
    if get_general_category(character) != GeneralCategory::DecimalNumber {
        return None;
    }

    let code_point = character as u32;
    let mut run_start = code_point;
    while let Some(previous) = run_start.checked_sub(1).and_then(char::from_u32) {
        if get_general_category(previous) != GeneralCategory::DecimalNumber {
            break;
        }
        run_start -= 1;
    }
    // Adjacent Nd runs still repeat their values every ten code points.
    Some((code_point - run_start) % 10)
}

fn push_tracked(output: String, text: &str, vm: &VM<'_>) -> RunResult<String> {
    let mut builder = StringBuilder::from_existing(output, &vm.heap.tracker);
    builder.push_str(text)?;
    builder.finish_raw()
}

fn value_error(message: impl fmt::Display) -> RunError {
    SimpleException::new_msg(ExcType::ValueError, message).into()
}
