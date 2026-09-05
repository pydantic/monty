//! Printf-style `%` formatting for `str` and `bytes` (`'%5.2f' % x`, `b'%s' % data`).
//!
//! Directive parsing, argument consumption and error precedence follow
//! CPython's `PyUnicode_Format` and `_PyBytes_FormatEx`, which share a
//! grammar. The core works on bytes for both modes: numeric text is ASCII and
//! every fragment appended in `str` mode is UTF-8. Float digit text comes from
//! the f-string formatters in [`crate::fstring`]; padding happens here because
//! printf pads differently from the format mini-language (zero fill also
//! covers `nan` and `inf`, text right-aligns, integer precision zero-extends).

use monty_types::ResourceTracker;

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    fstring::{ParsedFormatSpec, Sign, TypeChar, format_float_e, format_float_f, format_float_g},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData},
    resource_checks::check_repeat_size,
    str_format::value_error,
    string_builder::BytesBuilder,
    types::{
        LongInt, PyTrait, Type, bytes::allocate_bytes, long_int::check_bits_str_digits_limit, str::allocate_string,
    },
    value::Value,
};

/// Renders `template % args` for a `str` template.
pub(crate) fn percent_format(template: &str, args: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let output = format_template(template.as_bytes(), Mode::Str, args, vm)?;
    let output = String::from_utf8(output).expect("str formatting appends only UTF-8 fragments");
    Ok(allocate_string(output, vm.heap))
}

/// Renders `template % args` for a `bytes` template.
pub(crate) fn percent_format_bytes(template: &[u8], args: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let output = format_template(template, Mode::Bytes, args, vm)?;
    Ok(allocate_bytes(output, vm.heap))
}

/// The operand type being formatted; decides the text conversions, the `%c`
/// rules, and the wording of a few errors.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Str,
    Bytes,
}

impl Mode {
    /// The noun CPython uses in this mode's leftover-arguments error.
    fn noun(self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Bytes => "bytes",
        }
    }
}

/// Walks the template, copying literal runs and rendering each `%` directive.
fn format_template(template: &[u8], mode: Mode, args: &Value, vm: &mut VM<'_>) -> RunResult<Vec<u8>> {
    let arguments = Arguments::new(args, mode, vm);
    defer_drop!(arguments, vm);
    let mut cursor = DropGuard::new(Cursor::default(), vm);
    let (cursor, vm) = cursor.as_parts_mut();

    let mut output = Vec::new();
    let mut index = 0;
    let mut steps = 0;
    while index < template.len() {
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let Some(offset) = template[index..].iter().position(|byte| *byte == b'%') else {
            output = push_tracked(output, &template[index..], vm)?;
            break;
        };
        output = push_tracked(output, &template[index..index + offset], vm)?;
        index += offset + 1;
        // Only an immediate `%%` is an escape: `%5%` is a directive whose conversion is `%`.
        if template.get(index) == Some(&b'%') {
            output = push_tracked(output, b"%", vm)?;
            index += 1;
        } else {
            let (rendered, next) = render_directive(template, index, mode, arguments, cursor, vm)?;
            output = push_tracked(output, &rendered, vm)?;
            index = next;
        }
    }

    // A mapping operand never reports leftovers, so `'abc' % {}` is fine.
    if arguments.mapping.is_none() && cursor.next < arguments.positional.len() {
        Err(ExcType::type_error(format!(
            "not all arguments converted during {} formatting",
            mode.noun()
        )))
    } else {
        Ok(output)
    }
}

/// The right-hand operand, split the way CPython's `PyUnicode_Format` sees it.
struct Arguments {
    /// The tuple's items, or the lone operand itself.
    positional: Vec<Value>,
    /// The operand again when `%(key)` may index it: the types CPython's
    /// mapping check accepts (see `limitations/format.md`), minus `bytes`
    /// for a `bytes` template.
    mapping: Option<Value>,
}

impl Arguments {
    fn new(args: &Value, mode: Mode, vm: &VM<'_>) -> Self {
        let positional = match args {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Tuple(tuple) => clone_items(tuple.as_slice(), vm),
                HeapData::NamedTuple(tuple) => clone_items(tuple.as_vec(), vm),
                _ => vec![args.clone_with_heap(vm.heap)],
            },
            _ => vec![args.clone_with_heap(vm.heap)],
        };
        let mapping = match args.py_type(vm) {
            Type::Dict | Type::DefaultDict | Type::Counter | Type::List | Type::Range => true,
            Type::Bytes => mode == Mode::Str,
            _ => false,
        }
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

/// A directive read up to its conversion byte.
struct ParsedDirective {
    spec: Directive,
    conversion: u8,
    /// Index just past the conversion byte.
    end: usize,
}

/// A rendered text fragment with its length in the mode's units:
/// characters for `str`, bytes for `bytes`.
struct Text {
    bytes: Vec<u8>,
    len: usize,
}

impl Text {
    /// Truncates a `str` fragment to `precision` characters.
    fn from_string(text: String, precision: Option<usize>, tracker: &ResourceTracker) -> RunResult<Self> {
        let mut len = 0;
        let mut end = text.len();
        for (index, (offset, _)) in text.char_indices().enumerate() {
            tracker.check_time_every(index)?;
            if precision == Some(len) {
                end = offset;
                break;
            }
            len += 1;
        }
        let mut bytes = text.into_bytes();
        bytes.truncate(end);
        Ok(Self { bytes, len })
    }

    /// Truncates a `bytes` fragment to `precision` bytes.
    fn from_bytes(mut bytes: Vec<u8>, precision: Option<usize>) -> Self {
        if let Some(precision) = precision {
            bytes.truncate(precision);
        }
        let len = bytes.len();
        Self { bytes, len }
    }
}

/// Parses and renders the directive starting just after its `%`.
fn render_directive(
    template: &[u8],
    start: usize,
    mode: Mode,
    arguments: &Arguments,
    cursor: &mut Cursor,
    vm: &mut VM<'_>,
) -> RunResult<(Vec<u8>, usize)> {
    let mut index = start;
    // `%(key)` is looked up as soon as it is read, so a missing key reports
    // before a malformed spec does.
    if template.get(index) == Some(&b'(') {
        let key_end = find_key_end(template, index + 1, vm)?;
        lookup_key(&template[index + 1..key_end], mode, arguments, cursor, vm)?;
        index = key_end + 1;
    }
    let parsed = parse_directive(template, index, arguments, cursor, vm)?;

    // The argument is taken before the conversion is checked, so `'%5%' % ()`
    // reports the missing argument rather than the bad conversion.
    let value = next_argument(arguments, cursor, vm)?;
    defer_drop!(value, vm);
    let spec = &parsed.spec;
    let rendered = match parsed.conversion {
        b's' => text_operand(value, spec.precision, mode, vm).and_then(|text| pad_text(&text, spec, &vm.heap.tracker)),
        b'b' if mode == Mode::Bytes => {
            text_operand(value, spec.precision, mode, vm).and_then(|text| pad_text(&text, spec, &vm.heap.tracker))
        }
        b'r' | b'a' => repr_operand(value, parsed.conversion, spec.precision, mode, vm)
            .and_then(|text| pad_text(&text, spec, &vm.heap.tracker)),
        // `%c` ignores the precision.
        b'c' => char_operand(value, mode, vm).and_then(|text| pad_text(&text, spec, &vm.heap.tracker)),
        b'd' | b'i' | b'u' => format_integer(value, 10, parsed.conversion, spec, vm),
        b'o' => format_integer(value, 8, b'o', spec, vm),
        b'x' => format_integer(value, 16, b'x', spec, vm),
        b'X' => format_integer(value, 16, b'X', spec, vm),
        b'e' => format_float(value, TypeChar::E, mode, spec, vm),
        b'E' => format_float(value, TypeChar::EUpper, mode, spec, vm),
        b'f' => format_float(value, TypeChar::F, mode, spec, vm),
        b'F' => format_float(value, TypeChar::FUpper, mode, spec, vm),
        b'g' => format_float(value, TypeChar::G, mode, spec, vm),
        b'G' => format_float(value, TypeChar::GUpper, mode, spec, vm),
        _ => Err(unsupported(template, parsed.end - 1, mode)),
    }?;
    Ok((rendered, parsed.end))
}

/// Reads the flags, width, precision, optional C length modifier and
/// conversion byte, consuming the arguments a `*` width or precision names.
fn parse_directive(
    template: &[u8],
    start: usize,
    arguments: &Arguments,
    cursor: &mut Cursor,
    vm: &mut VM<'_>,
) -> RunResult<ParsedDirective> {
    let mut spec = Directive::default();
    let mut index = start;
    while let Some(flag) = template.get(index) {
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

    if template.get(index) == Some(&b'*') {
        let width = star_operand(arguments, cursor, vm, ExcType::overflow_c_ssize_t)?;
        // A negative `*` width left-justifies, like the `-` flag.
        spec.left |= width < 0;
        spec.width = usize::try_from(width.unsigned_abs()).map_err(|_| ExcType::overflow_c_ssize_t())?;
        index += 1;
    } else {
        (spec.width, index) = parse_number(template, index, "width too big")?;
    }

    if template.get(index) == Some(&b'.') {
        index += 1;
        if template.get(index) == Some(&b'*') {
            let precision = star_operand(arguments, cursor, vm, ExcType::overflow_c_int)?;
            // A negative `*` precision clamps to zero.
            spec.precision = Some(usize::try_from(precision).unwrap_or(0));
            index += 1;
        } else {
            let (precision, next) = parse_number(template, index, "precision too big")?;
            spec.precision = Some(precision);
            index = next;
        }
    }

    // C length modifiers are accepted and ignored.
    if matches!(template.get(index), Some(b'h' | b'l' | b'L')) {
        index += 1;
    }

    match template.get(index) {
        Some(conversion) => Ok(ParsedDirective {
            spec,
            conversion: *conversion,
            end: index + 1,
        }),
        None => Err(value_error("incomplete format")),
    }
}

/// Finds the `)` closing a `%(key)`, allowing nested parentheses in the key.
fn find_key_end(template: &[u8], start: usize, vm: &VM<'_>) -> RunResult<usize> {
    let mut depth = 0usize;
    for (offset, byte) in template[start..].iter().enumerate() {
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
fn parse_number(template: &[u8], start: usize, message: &str) -> RunResult<(usize, usize)> {
    let mut index = start;
    let mut number = 0usize;
    while let Some(digit) = template.get(index).filter(|byte| byte.is_ascii_digit()) {
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
/// remaining argument, as CPython does. The key takes the template's type.
fn lookup_key(key: &[u8], mode: Mode, arguments: &Arguments, cursor: &mut Cursor, vm: &mut VM<'_>) -> RunResult<()> {
    let Some(mapping) = &arguments.mapping else {
        return Err(ExcType::type_error("format requires a mapping"));
    };
    let key = match mode {
        Mode::Str => allocate_string(&*String::from_utf8_lossy(key), vm.heap),
        Mode::Bytes => allocate_bytes(key.to_vec(), vm.heap),
    };
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

/// The `%s` (and bytes `%b`) operand: `str()` of anything for a `str`
/// template, but only `bytes` for a `bytes` template.
fn text_operand(value: &Value, precision: Option<usize>, mode: Mode, vm: &mut VM<'_>) -> RunResult<Text> {
    match mode {
        Mode::Str => Text::from_string(vm.convert_value(value, 1)?, precision, &vm.heap.tracker),
        Mode::Bytes => match value {
            Value::InternBytes(id) => Ok(Text::from_bytes(vm.interns.get_bytes(*id).to_vec(), precision)),
            Value::Ref(id) if let HeapData::Bytes(bytes) = vm.heap.get(*id) => {
                Ok(Text::from_bytes(bytes.as_slice().to_vec(), precision))
            }
            _ => Err(ExcType::type_error(format!(
                "%b requires a bytes-like object, or an object that implements __bytes__, not '{}'",
                value.py_type_name(vm)
            ))),
        },
    }
}

/// The `%r` / `%a` operand: `repr()` or `ascii()` for a `str` template, and
/// always `ascii()` for a `bytes` template.
fn repr_operand(
    value: &Value,
    conversion: u8,
    precision: Option<usize>,
    mode: Mode,
    vm: &mut VM<'_>,
) -> RunResult<Text> {
    let ascii = mode == Mode::Bytes || conversion == b'a';
    let text = vm.convert_value(value, if ascii { 3 } else { 2 })?;
    Text::from_string(text, precision, &vm.heap.tracker)
}

/// The `%c` operand: a code point or one-character string for a `str`
/// template, a byte value or one-byte `bytes` for a `bytes` template.
fn char_operand(value: &Value, mode: Mode, vm: &mut VM<'_>) -> RunResult<Text> {
    match mode {
        Mode::Str => Text::from_string(char_text(value, vm)?, None, &vm.heap.tracker),
        Mode::Bytes => Ok(Text::from_bytes(vec![byte_char(value, vm)?], None)),
    }
}

/// Pads a text fragment to the width: text right-aligns unless `-` was
/// given, and the `0` flag is ignored.
fn pad_text(text: &Text, spec: &Directive, tracker: &ResourceTracker) -> RunResult<Vec<u8>> {
    pad("", "", &text.bytes, text.len, false, spec, tracker)
}

/// Renders `%d`/`%i`/`%u` (base 10), `%o`, `%x` and `%X`: the magnitude in
/// `base`, zero-extended to the precision, then signed, prefixed and padded.
fn format_integer(value: &Value, base: u32, conversion: u8, spec: &Directive, vm: &mut VM<'_>) -> RunResult<Vec<u8>> {
    let operand = integer_operand(value, conversion, vm)?;
    defer_drop!(operand, vm);
    let uppercase = conversion == b'X';
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
fn integer_operand(value: &Value, conversion: u8, vm: &mut VM<'_>) -> RunResult<Value> {
    let decimal = matches!(conversion, b'd' | b'i' | b'u');
    match value {
        Value::Int(_) | Value::Bool(_) => Ok(value.clone_with_heap(vm.heap)),
        Value::Float(f) if decimal => LongInt::value_from_f64(*f, vm.heap),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => Ok(value.clone_with_heap(vm.heap)),
        _ => {
            let requirement = if decimal { "a real number" } else { "an integer" };
            value.py_index_impl(vm)?.ok_or_else(|| {
                ExcType::type_error(format!(
                    "%{} format: {requirement} is required, not {}",
                    char::from(conversion),
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
fn zero_extend(digits: String, precision: Option<usize>, tracker: &ResourceTracker) -> RunResult<Vec<u8>> {
    let target = precision.unwrap_or(0);
    if digits.len() >= target {
        Ok(digits.into_bytes())
    } else {
        check_repeat_size(1, target, tracker)?;
        let mut output = BytesBuilder::with_capacity(target, tracker)?;
        push_repeated(&mut output, b'0', target - digits.len())?;
        output.push_slice(digits.as_bytes())?;
        Ok(output.finish())
    }
}

/// Renders the `%e`/`%f`/`%g` family through the shared f-string formatters,
/// then re-signs and pads the digit text the printf way.
fn format_float(
    value: &Value,
    type_char: TypeChar,
    mode: Mode,
    spec: &Directive,
    vm: &mut VM<'_>,
) -> RunResult<Vec<u8>> {
    let number = float_operand(value, mode, vm)?;
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
        body.as_bytes(),
        body.len(),
        spec.zero,
        spec,
        tracker,
    )
}

/// Coerces a float directive's operand: floats and ints directly, big ints
/// when they fit a float, anything else through `__index__`.
fn float_operand(value: &Value, mode: Mode, vm: &mut VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => match (li.to_f64(), mode) {
            (Some(f), _) if f.is_finite() => Ok(f),
            (_, Mode::Str) => Err(ExcType::overflow_int_to_float()),
            // CPython's bytes formatter folds the overflow into its generic type error.
            (_, Mode::Bytes) => Err(ExcType::type_error("float argument required, not int")),
        },
        _ => {
            if let Some(index) = value.py_index_impl(vm)? {
                defer_drop!(index, vm);
                float_operand(index, mode, vm)
            } else {
                let type_name = value.py_type_name(vm);
                Err(ExcType::type_error(match mode {
                    Mode::Str => format!("must be real number, not {type_name}"),
                    Mode::Bytes => format!("float argument required, not {type_name}"),
                }))
            }
        }
    }
}

/// Renders a `str` `%c`: a one-character string as itself, otherwise an
/// integer code point.
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

/// The character for a `str` `%c` code point; surrogates are rejected with
/// the range error, as `chr()` does.
fn char_from_code(code: i64) -> RunResult<String> {
    u32::try_from(code)
        .ok()
        .filter(|code| *code <= 0x0010_FFFF)
        .and_then(char::from_u32)
        .map(|c| c.to_string())
        .ok_or_else(char_range_error)
}

/// CPython's out-of-range error for a `str` `%c`.
fn char_range_error() -> RunError {
    SimpleException::new_msg(ExcType::OverflowError, "%c arg not in range(0x110000)").into()
}

/// Renders a `bytes` `%c`: a one-byte `bytes` as itself, otherwise an
/// integer in `range(256)`.
fn byte_char(value: &Value, vm: &mut VM<'_>) -> RunResult<u8> {
    match value {
        Value::Int(n) => u8::try_from(*n).map_err(|_| byte_range_error()),
        Value::Bool(b) => Ok(u8::from(*b)),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => Err(byte_range_error()),
        Value::InternBytes(id) => single_byte(vm.interns.get_bytes(*id)),
        Value::Ref(id) if let HeapData::Bytes(bytes) = vm.heap.get(*id) => single_byte(bytes.as_slice()),
        _ => match value.py_index_impl(vm)? {
            Some(index) => {
                defer_drop!(index, vm);
                byte_char(index, vm)
            }
            None => Err(ExcType::type_error(format!(
                "%c requires an integer in range(256) or a single byte, not {}",
                value.py_type_name(vm)
            ))),
        },
    }
}

/// The one byte of a `bytes` `%c` operand.
fn single_byte(bytes: &[u8]) -> RunResult<u8> {
    match bytes {
        [byte] => Ok(*byte),
        _ => Err(ExcType::type_error(format!(
            "%c requires an integer in range(256) or a single byte, not a bytes object of length {}",
            bytes.len()
        ))),
    }
}

/// CPython's out-of-range error for a `bytes` `%c`.
fn byte_range_error() -> RunError {
    SimpleException::new_msg(ExcType::OverflowError, "%c arg not in range(256)").into()
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
    body: &[u8],
    body_len: usize,
    zero_fill: bool,
    spec: &Directive,
    tracker: &ResourceTracker,
) -> RunResult<Vec<u8>> {
    let padding = spec.width.saturating_sub(sign.len() + prefix.len() + body_len);
    // The width may come from an argument (`%*s`), so budget the padding first.
    check_repeat_size(1, padding, tracker)?;
    let mut output = BytesBuilder::with_capacity(sign.len() + prefix.len() + body.len() + padding, tracker)?;
    if spec.left {
        output.push_slice(sign.as_bytes())?;
        output.push_slice(prefix.as_bytes())?;
        output.push_slice(body)?;
        push_repeated(&mut output, b' ', padding)?;
    } else if zero_fill {
        output.push_slice(sign.as_bytes())?;
        output.push_slice(prefix.as_bytes())?;
        push_repeated(&mut output, b'0', padding)?;
        output.push_slice(body)?;
    } else {
        push_repeated(&mut output, b' ', padding)?;
        output.push_slice(sign.as_bytes())?;
        output.push_slice(prefix.as_bytes())?;
        output.push_slice(body)?;
    }
    Ok(output.finish())
}

/// Appends `count` copies of `fill`; the caller has already budgeted them.
fn push_repeated(output: &mut BytesBuilder<'_>, fill: u8, count: usize) -> RunResult<()> {
    for _ in 0..count {
        output.push(fill)?;
    }
    Ok(())
}

/// Appends a fragment to the output while preserving `BytesBuilder` accounting.
fn push_tracked(output: Vec<u8>, fragment: &[u8], vm: &VM<'_>) -> RunResult<Vec<u8>> {
    let mut builder = BytesBuilder::from_existing(output, &vm.heap.tracker);
    builder.push_slice(fragment)?;
    Ok(builder.finish())
}

/// The error for an unknown conversion at byte `index`. CPython reports the
/// character index in a `str` template and the byte index in a `bytes` one,
/// and trips over a non-ASCII byte in a `bytes` template with an odd
/// `OverflowError` instead.
fn unsupported(template: &[u8], index: usize, mode: Mode) -> RunError {
    let (conversion, char_index) = match mode {
        Mode::Bytes if !template[index].is_ascii() => {
            return SimpleException::new_msg(ExcType::OverflowError, "character argument not in range(0x110000)")
                .into();
        }
        Mode::Bytes => (char::from(template[index]), index),
        Mode::Str => (
            String::from_utf8_lossy(&template[index..])
                .chars()
                .next()
                .unwrap_or('?'),
            String::from_utf8_lossy(&template[..index]).chars().count(),
        ),
    };
    // Only printable ASCII is quoted as itself.
    let shown = if (' '..='~').contains(&conversion) {
        conversion
    } else {
        '?'
    };
    value_error(format!(
        "unsupported format character '{shown}' (0x{:x}) at index {char_index}",
        u32::from(conversion)
    ))
}
