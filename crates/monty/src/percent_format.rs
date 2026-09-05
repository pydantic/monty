//! Printf-style `str % args` formatting (`'%5.2f' % x`, `'%(key)s' % mapping`).
//!
//! Directive parsing, argument consumption and error precedence follow
//! CPython's `PyUnicode_Format`. Float digit text comes from the f-string
//! formatters in [`crate::fstring`]; padding happens here because printf pads
//! differently from the format mini-language (zero fill also covers `nan` and
//! `inf`, text right-aligns, integer precision zero-extends the digits).

use monty_types::ResourceTracker;

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    fstring::{ParsedFormatSpec, Sign, TypeChar, format_float_e, format_float_f, format_float_g},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData},
    resource_checks::check_repeat_size,
    str_format::{push_tracked, value_error},
    string_builder::StringBuilder,
    types::{LongInt, PyTrait, Type, long_int::check_bits_str_digits_limit, str::allocate_string},
    value::Value,
};

/// Renders `template % args`: `args` is a tuple of positional values, a
/// mapping for `%(key)` directives, or a single value.
pub(crate) fn percent_format(template: &str, args: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let arguments = Arguments::new(args, vm);
    defer_drop!(arguments, vm);
    let mut cursor = DropGuard::new(Cursor::default(), vm);
    let (cursor, vm) = cursor.as_parts_mut();

    let mut output = String::new();
    let mut index = 0;
    let mut steps = 0;
    while index < template.len() {
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let Some(offset) = template[index..].find('%') else {
            output = push_tracked(output, &template[index..], vm)?;
            break;
        };
        output = push_tracked(output, &template[index..index + offset], vm)?;
        index += offset + 1;
        // Only an immediate `%%` is an escape: `%5%` is a directive whose conversion is `%`.
        if template.as_bytes().get(index) == Some(&b'%') {
            output = push_tracked(output, "%", vm)?;
            index += 1;
        } else {
            let (rendered, next) = render_directive(template, index, arguments, cursor, vm)?;
            output = push_tracked(output, &rendered, vm)?;
            index = next;
        }
    }

    // A mapping operand never reports leftovers, so `'abc' % {}` is fine.
    if arguments.mapping.is_none() && cursor.next < arguments.positional.len() {
        Err(ExcType::type_error(
            "not all arguments converted during string formatting",
        ))
    } else {
        Ok(allocate_string(output, vm.heap))
    }
}

/// The right-hand operand, split the way CPython's `PyUnicode_Format` sees it.
struct Arguments {
    /// The tuple's items, or the lone operand itself.
    positional: Vec<Value>,
    /// The operand again when `%(key)` may index it: the types CPython's
    /// mapping check accepts (see `limitations/format.md`).
    mapping: Option<Value>,
}

impl Arguments {
    fn new(args: &Value, vm: &VM<'_>) -> Self {
        let positional = match args {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Tuple(tuple) => clone_items(tuple.as_slice(), vm),
                HeapData::NamedTuple(tuple) => clone_items(tuple.as_vec(), vm),
                _ => vec![args.clone_with_heap(vm.heap)],
            },
            _ => vec![args.clone_with_heap(vm.heap)],
        };
        let mapping = matches!(
            args.py_type(vm),
            Type::Dict | Type::DefaultDict | Type::Counter | Type::List | Type::Bytes | Type::Range
        )
        .then(|| args.clone_with_heap(vm.heap));
        Self { positional, mapping }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Arguments {
    fn drop_with(self, ctx: &mut C) {
        self.positional.drop_with(ctx);
        self.mapping.drop_with(ctx);
    }
}

impl<C: ContainsHeap> DropWithContext<C> for Cursor {
    fn drop_with(self, ctx: &mut C) {
        self.pending.drop_with(ctx);
    }
}

/// Clones tuple items so the operand can be released before rendering ends.
fn clone_items(items: &[Value], vm: &VM<'_>) -> Vec<Value> {
    items.iter().map(|item| item.clone_with_heap(vm.heap)).collect()
}

/// Argument consumption state; lives inside a `DropGuard` in the render loop
/// so a pending keyed value is released on every error path.
#[derive(Default)]
struct Cursor {
    /// Index of the next unconsumed positional argument.
    next: usize,
    /// Set once a `%(key)` directive has run: from then on the only argument
    /// left is the looked-up value itself, which `pending` carries until a `*`
    /// or the conversion takes it.
    keyed: bool,
    /// The value a `%(key)` lookup produced, not yet consumed.
    pending: Option<Value>,
}

/// The flags, width and precision of one `%` directive.
#[derive(Default)]
struct Directive {
    /// `-`: left-justify within the width.
    left: bool,
    /// `+` always signs numbers; ` ` puts a space where their sign would go.
    /// `+` wins when both are given.
    sign: Option<Sign>,
    /// `#`: `0x`/`0o` prefixes and a forced decimal point.
    alternate: bool,
    /// `0`: pad numbers with zeros after the sign; ignored for text and under `-`.
    zero: bool,
    /// Minimum field width in characters.
    width: usize,
    /// Digits after the point for floats, minimum digits for integers,
    /// maximum characters for text.
    precision: Option<usize>,
}

/// A directive read up to its conversion character.
struct ParsedDirective {
    spec: Directive,
    conversion: char,
    /// Index just past the conversion character.
    end: usize,
}

/// Parses and renders the directive starting just after its `%`.
fn render_directive(
    template: &str,
    start: usize,
    arguments: &Arguments,
    cursor: &mut Cursor,
    vm: &mut VM<'_>,
) -> RunResult<(String, usize)> {
    let mut index = start;
    // `%(key)` is looked up as soon as it is read, so a missing key reports
    // before a malformed spec does.
    if template.as_bytes().get(index) == Some(&b'(') {
        let key_end = find_key_end(template, index + 1, vm)?;
        lookup_key(&template[index + 1..key_end], arguments, cursor, vm)?;
        index = key_end + 1;
    }
    let parsed = parse_directive(template, index, arguments, cursor, vm)?;

    // The argument is taken before the conversion is checked, so `'%5%' % ()`
    // reports the missing argument rather than the bad conversion.
    let value = next_argument(arguments, cursor, vm)?;
    defer_drop!(value, vm);
    let spec = &parsed.spec;
    let rendered = match parsed.conversion {
        's' => pad_text(&vm.convert_value(value, 1)?, spec.precision, spec, &vm.heap.tracker),
        'r' => pad_text(&vm.convert_value(value, 2)?, spec.precision, spec, &vm.heap.tracker),
        'a' => pad_text(&vm.convert_value(value, 3)?, spec.precision, spec, &vm.heap.tracker),
        // `%c` ignores the precision.
        'c' => pad_text(&char_text(value, vm)?, None, spec, &vm.heap.tracker),
        'd' | 'i' | 'u' => format_integer(value, 10, parsed.conversion, spec, vm),
        'o' => format_integer(value, 8, 'o', spec, vm),
        'x' => format_integer(value, 16, 'x', spec, vm),
        'X' => format_integer(value, 16, 'X', spec, vm),
        'e' => format_float(value, TypeChar::E, spec, vm),
        'E' => format_float(value, TypeChar::EUpper, spec, vm),
        'f' => format_float(value, TypeChar::F, spec, vm),
        'F' => format_float(value, TypeChar::FUpper, spec, vm),
        'g' => format_float(value, TypeChar::G, spec, vm),
        'G' => format_float(value, TypeChar::GUpper, spec, vm),
        other => Err(unsupported(other, template, parsed.end - other.len_utf8())),
    }?;
    Ok((rendered, parsed.end))
}

/// Reads the flags, width, precision, optional C length modifier and
/// conversion character, consuming the arguments a `*` width or precision names.
fn parse_directive(
    template: &str,
    start: usize,
    arguments: &Arguments,
    cursor: &mut Cursor,
    vm: &mut VM<'_>,
) -> RunResult<ParsedDirective> {
    let bytes = template.as_bytes();
    let mut spec = Directive::default();
    let mut index = start;
    while let Some(flag) = bytes.get(index) {
        match flag {
            b'-' => spec.left = true,
            b'+' => spec.sign = Some(Sign::Plus),
            b' ' => spec.sign = spec.sign.or(Some(Sign::Space)),
            b'#' => spec.alternate = true,
            b'0' => spec.zero = true,
            _ => break,
        }
        index += 1;
    }

    if bytes.get(index) == Some(&b'*') {
        let width = star_operand(arguments, cursor, vm, ExcType::overflow_c_ssize_t)?;
        // A negative `*` width left-justifies, like the `-` flag.
        spec.left |= width < 0;
        spec.width = usize::try_from(width.unsigned_abs()).map_err(|_| ExcType::overflow_c_ssize_t())?;
        index += 1;
    } else {
        (spec.width, index) = parse_number(template, index, "width too big")?;
    }

    if bytes.get(index) == Some(&b'.') {
        index += 1;
        if bytes.get(index) == Some(&b'*') {
            let precision = star_operand(arguments, cursor, vm, ExcType::overflow_c_int)?;
            // A negative `*` precision means none was given.
            spec.precision = usize::try_from(precision).ok();
            index += 1;
        } else {
            let (precision, next) = parse_number(template, index, "precision too big")?;
            spec.precision = Some(precision);
            index = next;
        }
    }

    // C length modifiers are accepted and ignored.
    if matches!(bytes.get(index), Some(b'h' | b'l' | b'L')) {
        index += 1;
    }

    match template[index..].chars().next() {
        Some(conversion) => Ok(ParsedDirective {
            spec,
            conversion,
            end: index + conversion.len_utf8(),
        }),
        None => Err(value_error("incomplete format")),
    }
}

/// Finds the `)` closing a `%(key)`, allowing nested parentheses in the key.
fn find_key_end(template: &str, start: usize, vm: &VM<'_>) -> RunResult<usize> {
    let mut depth = 0usize;
    for (offset, byte) in template.as_bytes()[start..].iter().enumerate() {
        vm.heap.tracker.check_time_every(offset)?;
        match byte {
            b'(' => depth += 1,
            b')' if depth == 0 => return Ok(start + offset),
            b')' => depth -= 1,
            _ => {}
        }
    }
    Err(value_error("incomplete format key"))
}

/// Parses a run of decimal digits, raising `message` past the signed size range.
fn parse_number(template: &str, start: usize, message: &str) -> RunResult<(usize, usize)> {
    let bytes = template.as_bytes();
    let mut index = start;
    let mut number = 0usize;
    while let Some(digit) = bytes.get(index).filter(|byte| byte.is_ascii_digit()) {
        number = number
            .checked_mul(10)
            .and_then(|number| number.checked_add(usize::from(digit - b'0')))
            .filter(|number| isize::try_from(*number).is_ok())
            .ok_or_else(|| value_error(message))?;
        index += 1;
    }
    Ok((number, index))
}

/// Looks `key` up in the mapping operand and makes the result the one
/// remaining argument, as CPython does.
fn lookup_key(key: &str, arguments: &Arguments, cursor: &mut Cursor, vm: &mut VM<'_>) -> RunResult<()> {
    let Some(mapping) = &arguments.mapping else {
        return Err(ExcType::type_error("format requires a mapping"));
    };
    let key = allocate_string(key, vm.heap);
    defer_drop!(key, vm);
    let value = mapping.py_getitem(key, vm)?;
    cursor.keyed = true;
    cursor.pending.replace(value).drop_with(vm);
    Ok(())
}

/// Takes the next argument: the pending keyed value if there is one,
/// otherwise the next positional value.
fn next_argument(arguments: &Arguments, cursor: &mut Cursor, vm: &VM<'_>) -> RunResult<Value> {
    let positional = if cursor.keyed {
        None
    } else {
        arguments.positional.get(cursor.next)
    };
    match (cursor.pending.take(), positional) {
        (Some(value), _) => Ok(value),
        (None, Some(value)) => {
            cursor.next += 1;
            Ok(value.clone_with_heap(vm.heap))
        }
        (None, None) => Err(ExcType::type_error("not enough arguments for format string")),
    }
}

/// Consumes the positional argument a `*` width or precision reads.
fn star_operand(
    arguments: &Arguments,
    cursor: &mut Cursor,
    vm: &mut VM<'_>,
    on_overflow: fn() -> RunError,
) -> RunResult<i64> {
    let value = next_argument(arguments, cursor, vm)?;
    defer_drop!(value, vm);
    match value {
        Value::Int(n) => Ok(*n),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => Err(on_overflow()),
        _ => Err(ExcType::type_error("* wants int")),
    }
}

/// Truncates `text` to `precision` characters and pads it to the width:
/// text right-aligns unless `-` was given, and the `0` flag is ignored.
fn pad_text(text: &str, precision: Option<usize>, spec: &Directive, tracker: &ResourceTracker) -> RunResult<String> {
    let mut length = 0;
    let mut end = text.len();
    for (index, (offset, _)) in text.char_indices().enumerate() {
        tracker.check_time_every(index)?;
        if precision == Some(length) {
            end = offset;
            break;
        }
        length += 1;
    }
    pad("", "", &text[..end], length, false, spec, tracker)
}

/// Renders `%d`/`%i`/`%u` (base 10), `%o`, `%x` and `%X`: the magnitude in
/// `base`, zero-extended to the precision, then signed, prefixed and padded.
fn format_integer(value: &Value, base: u32, conversion: char, spec: &Directive, vm: &mut VM<'_>) -> RunResult<String> {
    let operand = integer_operand(value, conversion, vm)?;
    defer_drop!(operand, vm);
    let uppercase = conversion == 'X';
    let (negative, digits) = magnitude_digits(operand, base, uppercase, vm)?;
    let digits = zero_extend(digits, spec.precision, &vm.heap.tracker)?;
    let prefix = match (spec.alternate, base) {
        (true, 8) => "0o",
        (true, 16) if uppercase => "0X",
        (true, 16) => "0x",
        _ => "",
    };
    pad(
        number_sign(negative, spec),
        prefix,
        &digits,
        digits.len(),
        spec.zero,
        spec,
        &vm.heap.tracker,
    )
}

/// Coerces an integer directive's operand: ints and bools as they are, floats
/// truncated for the decimal directives only, anything else through `__index__`.
fn integer_operand(value: &Value, conversion: char, vm: &mut VM<'_>) -> RunResult<Value> {
    let decimal = matches!(conversion, 'd' | 'i' | 'u');
    match value {
        Value::Int(_) | Value::Bool(_) => Ok(value.clone_with_heap(vm.heap)),
        Value::Float(f) if decimal => LongInt::value_from_f64(*f, vm.heap),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => Ok(value.clone_with_heap(vm.heap)),
        _ => {
            let requirement = if decimal { "a real number" } else { "an integer" };
            value.py_index_impl(vm)?.ok_or_else(|| {
                ExcType::type_error(format!(
                    "%{conversion} format: {requirement} is required, not {}",
                    value.py_type_name(vm)
                ))
            })
        }
    }
}

/// Renders an integer operand's magnitude in `base`, reporting whether it was negative.
fn magnitude_digits(operand: &Value, base: u32, uppercase: bool, vm: &VM<'_>) -> RunResult<(bool, String)> {
    match operand {
        Value::Int(n) => Ok((*n < 0, small_digits(n.unsigned_abs(), base, uppercase))),
        Value::Bool(b) => Ok((false, u64::from(*b).to_string())),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
            long_digits(li, base, uppercase, &vm.heap.tracker)
        }
        _ => Err(ExcType::type_error(format!(
            "__index__ returned non-int (type {})",
            operand.py_type_name(vm)
        ))),
    }
}

/// Digits of a machine-word magnitude in base 8, 10 or 16.
fn small_digits(magnitude: u64, base: u32, uppercase: bool) -> String {
    match (base, uppercase) {
        (8, _) => format!("{magnitude:o}"),
        (16, true) => format!("{magnitude:X}"),
        (16, false) => format!("{magnitude:x}"),
        _ => magnitude.to_string(),
    }
}

/// Digits of a big integer's magnitude, budgeted before the render like
/// `format_long_int`; only decimal is bounded by CPython's digit limit.
fn long_digits(li: &LongInt, base: u32, uppercase: bool, tracker: &ResourceTracker) -> RunResult<(bool, String)> {
    if base == 10 {
        check_bits_str_digits_limit(li.bits())?;
    }
    let max_digits = li.bits() / u64::from(base.trailing_zeros().max(1));
    check_repeat_size(1, usize::try_from(max_digits).unwrap_or(usize::MAX), tracker)?;
    let mut digits = li.abs().inner().to_str_radix(base);
    if uppercase {
        digits.make_ascii_uppercase();
    }
    Ok((li.is_negative(), digits))
}

/// Left-pads digits with zeros up to the precision (`'%.3d' % 5` → `005`).
fn zero_extend(digits: String, precision: Option<usize>, tracker: &ResourceTracker) -> RunResult<String> {
    let target = precision.unwrap_or(0);
    if digits.len() >= target {
        Ok(digits)
    } else {
        check_repeat_size(1, target, tracker)?;
        let mut output = StringBuilder::with_capacity(target, tracker)?;
        push_repeated(&mut output, '0', target - digits.len())?;
        output.push_str(&digits)?;
        output.finish_raw()
    }
}

/// Renders the `%e`/`%f`/`%g` family through the shared f-string formatters,
/// then re-signs and pads the digit text the printf way.
fn format_float(value: &Value, type_char: TypeChar, spec: &Directive, vm: &mut VM<'_>) -> RunResult<String> {
    let number = float_operand(value, vm)?;
    let precision = spec.precision.unwrap_or(6);
    // The precision may come from an argument (`%.*f`), and the fixed and
    // exponent formatters synthesise that many digits.
    let tracker = &vm.heap.tracker;
    check_repeat_size(1, precision, tracker)?;
    let parsed = ParsedFormatSpec {
        alternate: spec.alternate,
        precision: Some(precision),
        type_char: Some(type_char),
        ..ParsedFormatSpec::default()
    };
    let text = match type_char {
        TypeChar::E => format_float_e(number, &parsed, false, tracker),
        TypeChar::EUpper => format_float_e(number, &parsed, true, tracker),
        TypeChar::G | TypeChar::GUpper => format_float_g(number, &parsed, tracker),
        _ => format_float_f(number, &parsed, tracker),
    }?;
    let (negative, body) = match text.strip_prefix('-') {
        Some(body) => (true, body),
        None => (false, text.as_str()),
    };
    pad(
        number_sign(negative, spec),
        "",
        body,
        body.len(),
        spec.zero,
        spec,
        tracker,
    )
}

/// Coerces a float directive's operand: floats and ints directly, big ints
/// when they fit a float, anything else through `__index__`.
fn float_operand(value: &Value, vm: &mut VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => match li.to_f64() {
            Some(f) if f.is_finite() => Ok(f),
            _ => Err(ExcType::overflow_int_to_float()),
        },
        _ => match value.py_index_impl(vm)? {
            Some(index) => {
                defer_drop!(index, vm);
                float_operand(index, vm)
            }
            None => Err(ExcType::type_error(format!(
                "must be real number, not {}",
                value.py_type_name(vm)
            ))),
        },
    }
}

/// Renders `%c`: a one-character string as itself, otherwise an integer code point.
fn char_text(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    match value {
        Value::Int(n) => char_from_code(*n),
        Value::Bool(b) => char_from_code(i64::from(*b)),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => Err(char_range_error()),
        _ if value.py_type(vm) == Type::Str => {
            let text = value.to_str(vm)?;
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c.to_string()),
                _ => Err(ExcType::type_error(format!(
                    "%c requires an int or a unicode character, not a string of length {}",
                    text.chars().count()
                ))),
            }
        }
        _ => match value.py_index_impl(vm)? {
            Some(index) => {
                defer_drop!(index, vm);
                char_text(index, vm)
            }
            None => Err(ExcType::type_error(format!(
                "%c requires an int or a unicode character, not {}",
                value.py_type_name(vm)
            ))),
        },
    }
}

/// The character for a `%c` code point; surrogates are rejected with the
/// range error, as `chr()` does.
fn char_from_code(code: i64) -> RunResult<String> {
    u32::try_from(code)
        .ok()
        .filter(|code| *code <= 0x0010_FFFF)
        .and_then(char::from_u32)
        .map(|c| c.to_string())
        .ok_or_else(char_range_error)
}

/// CPython's out-of-range `%c` error.
fn char_range_error() -> RunError {
    SimpleException::new_msg(ExcType::OverflowError, "%c arg not in range(0x110000)").into()
}

/// The sign printf emits: `-` for negatives, else `+` or a space on request.
fn number_sign(negative: bool, spec: &Directive) -> &'static str {
    match spec.sign {
        _ if negative => "-",
        Some(Sign::Plus) => "+",
        Some(Sign::Space) => " ",
        _ => "",
    }
}

/// Pads `sign + prefix + body` to the width: `-` left-justifies, otherwise
/// spaces lead, or zeros sit between the prefix and the body under `zero_fill`.
fn pad(
    sign: &str,
    prefix: &str,
    body: &str,
    body_chars: usize,
    zero_fill: bool,
    spec: &Directive,
    tracker: &ResourceTracker,
) -> RunResult<String> {
    let padding = spec.width.saturating_sub(sign.len() + prefix.len() + body_chars);
    // The width may come from an argument (`%*s`), so budget the padding first.
    check_repeat_size(1, padding, tracker)?;
    let mut output = StringBuilder::with_capacity(sign.len() + prefix.len() + body.len() + padding, tracker)?;
    if spec.left {
        output.push_str(sign)?;
        output.push_str(prefix)?;
        output.push_str(body)?;
        push_repeated(&mut output, ' ', padding)?;
    } else if zero_fill {
        output.push_str(sign)?;
        output.push_str(prefix)?;
        push_repeated(&mut output, '0', padding)?;
        output.push_str(body)?;
    } else {
        push_repeated(&mut output, ' ', padding)?;
        output.push_str(sign)?;
        output.push_str(prefix)?;
        output.push_str(body)?;
    }
    output.finish_raw()
}

/// Appends `count` copies of `fill`; the caller has already budgeted them.
fn push_repeated(output: &mut StringBuilder<'_>, fill: char, count: usize) -> RunResult<()> {
    for _ in 0..count {
        output.push(fill)?;
    }
    Ok(())
}

/// CPython's `unsupported format character` error, quoting the character (or
/// `?` outside printable ASCII) and its character index in the template.
fn unsupported(conversion: char, template: &str, index: usize) -> RunError {
    let shown = if (' '..='~').contains(&conversion) {
        conversion
    } else {
        '?'
    };
    value_error(format!(
        "unsupported format character '{shown}' (0x{:x}) at index {}",
        u32::from(conversion),
        template[..index].chars().count()
    ))
}
