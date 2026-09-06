//! Printf-style `%` formatting for `str` and `bytes` (`'%5.2f' % x`, `b'%s' % data`).
//!
//! Directive parsing, argument consumption and error precedence follow
//! CPython's `PyUnicode_Format` and `_PyBytes_FormatEx`, which share a
//! grammar. Directives are ASCII, so scanning works on the template's bytes
//! for both types; the output stays typed as `String` or `Vec<u8>` through the
//! [`Target`] trait. Float digit text comes from the f-string formatters in
//! [`crate::fstring`]; padding happens here because printf pads differently
//! from the format mini-language (zero fill also covers `nan` and `inf`, text
//! right-aligns, integer precision zero-extends).

use std::ops::Range;

use monty_types::ResourceTracker;

use crate::{
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    fstring::{ParsedFormatSpec, Sign, TypeChar, format_float_e, format_float_f, format_float_g},
    heap::{ContainsHeap, DropWithContext, Heap, HeapData},
    resource_checks::check_repeat_size,
    str_format::value_error,
    string_builder::{BytesBuilder, StringBuilder},
    types::{
        LongInt, PyTrait, Type, bytes::allocate_bytes, long_int::check_bits_str_digits_limit, str::allocate_string,
    },
    value::{VALUE_SIZE, Value},
};

/// Renders `template % args` for a `str` template.
pub(crate) fn percent_format(template: &str, args: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    format_template::<StrTarget>(template, args, vm)
}

/// Renders `template % args` for a `bytes` template.
pub(crate) fn percent_format_bytes(template: &[u8], args: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    format_template::<BytesTarget>(template, args, vm)
}

/// Copies a `bytes` template through the tracker, so a template near the
/// memory limit reports `MemoryError` rather than tripping the hard ceiling.
pub(crate) fn copy_bytes_template(template: &[u8], tracker: &ResourceTracker) -> RunResult<Vec<u8>> {
    let mut copy = BytesBuilder::with_capacity(template.len(), tracker)?;
    copy.push_slice(template)?;
    Ok(copy.finish())
}

/// Bytes scanned between tracker clock polls while looking for the next `%`.
const SCAN_CHUNK: usize = 4096;

/// The largest precision CPython's integer formatter accepts (`INT_MAX - 3`);
/// above it `%d` and friends raise `OverflowError` rather than zero-extending.
const MAX_INTEGER_PRECISION: usize = i32::MAX as usize - 3;

/// A `%` formatting target: the template type, its output buffer, and the
/// behaviour that differs between `str` and `bytes` templates. Keeping the
/// output typed per target means the `str` path never re-validates UTF-8.
trait Target: Sized {
    /// `str` or `[u8]`.
    type Template: ?Sized;
    /// `String` or `Vec<u8>`.
    type Output: Default;
    /// The tracked builder for [`Self::Output`].
    type Builder<'t>: OutputBuilder<'t, Output = Self::Output>;

    /// Selects the `bytes` rules: `%b`, `ascii()` for `%r`, the error wording.
    const IS_BYTES: bool;
    /// The error for a literal precision above C `int`.
    const PRECISION_TOO_BIG: &'static str;

    /// The template's bytes; every offset the formatter slices at is an ASCII byte.
    fn as_bytes(template: &Self::Template) -> &[u8];
    /// Appends `template[range]` to the output.
    fn push_literal(builder: &mut Self::Builder<'_>, template: &Self::Template, range: Range<usize>) -> RunResult<()>;
    /// A `%(key)` as a value of the template's type.
    fn key_value(template: &Self::Template, range: Range<usize>, heap: &Heap) -> Value;
    /// An ASCII fragment (digits, signs, exponents) as an output value.
    fn from_ascii(text: String) -> Self::Output;
    /// The size of an output fragment in bytes.
    fn byte_len(output: &Self::Output) -> usize;
    /// A `str` fragment cut to the directive's precision: `%r`, `%a`, and the `str` `%s` / `%c`.
    fn text_from_string(text: String, spec: &Directive, tracker: &ResourceTracker) -> RunResult<Text<Self>>;
    /// The `%s` operand (also `%b` for `bytes`).
    fn text_operand(value: &Value, spec: &Directive, vm: &mut VM<'_>) -> RunResult<Text<Self>>;
    /// The `%c` operand, always one character or byte; the precision is ignored.
    fn char_operand(value: &Value, vm: &mut VM<'_>) -> RunResult<Text<Self>>;
    /// The error for a float directive on a non-numeric operand.
    fn float_type_error(type_name: &str) -> RunError;
    /// The error for an int too large to convert to a float.
    fn float_overflow_error() -> RunError;
    /// The error for an unknown conversion at byte `index`.
    fn unsupported(template: &Self::Template, index: usize) -> RunError;
    /// The finished output as a heap value.
    fn allocate(output: Self::Output, heap: &Heap) -> Value;
}

/// The tracked builder a [`Target`] assembles its output with.
trait OutputBuilder<'t>: Sized {
    type Output;

    /// Wraps an existing output whose capacity is already tracker-accounted.
    fn from_existing(output: Self::Output, tracker: &'t ResourceTracker) -> Self;
    /// Reserves `capacity` bytes up front after one tracker check.
    fn with_capacity(capacity: usize, tracker: &'t ResourceTracker) -> RunResult<Self>;
    /// Appends text: template literals, signs, prefixes and digits.
    fn push_text(&mut self, text: &str) -> RunResult<()>;
    /// Appends `count` copies of an ASCII fill byte.
    fn push_fill(&mut self, fill: u8, count: usize) -> RunResult<()>;
    /// Appends a rendered fragment.
    fn push_output(&mut self, fragment: &Self::Output) -> RunResult<()>;
    /// Consumes the builder.
    fn finish(self) -> RunResult<Self::Output>;
}

/// A rendered text fragment with its length in the target's units:
/// characters for `str`, bytes for `bytes`.
struct Text<T: Target> {
    body: T::Output,
    /// Only padding reads this, so a `str` directive with neither width nor
    /// precision skips the count and reports `0` rather than walking the text.
    len: usize,
}

/// `str % args`.
struct StrTarget;

impl Target for StrTarget {
    type Template = str;
    type Output = String;
    type Builder<'t> = StringBuilder<'t>;

    const IS_BYTES: bool = false;
    const PRECISION_TOO_BIG: &'static str = "precision too big";

    fn as_bytes(template: &str) -> &[u8] {
        template.as_bytes()
    }

    fn push_literal(builder: &mut StringBuilder<'_>, template: &str, range: Range<usize>) -> RunResult<()> {
        builder.push_text(&template[range])
    }

    fn key_value(template: &str, range: Range<usize>, heap: &Heap) -> Value {
        allocate_string(&template[range], heap)
    }

    fn from_ascii(text: String) -> String {
        text
    }

    fn byte_len(output: &String) -> usize {
        output.len()
    }

    fn text_from_string(mut text: String, spec: &Directive, tracker: &ResourceTracker) -> RunResult<Text<Self>> {
        if spec.precision.is_none() && spec.width == 0 {
            Ok(Text { body: text, len: 0 })
        } else {
            let mut len = 0;
            let mut end = text.len();
            for (index, (offset, _)) in text.char_indices().enumerate() {
                tracker.check_time_every(index)?;
                if spec.precision == Some(len) {
                    end = offset;
                    break;
                }
                len += 1;
            }
            text.truncate(end);
            Ok(Text { body: text, len })
        }
    }

    /// `str()` of any value.
    fn text_operand(value: &Value, spec: &Directive, vm: &mut VM<'_>) -> RunResult<Text<Self>> {
        Self::text_from_string(vm.convert_value(value, 1)?, spec, &vm.heap.tracker)
    }

    fn char_operand(value: &Value, vm: &mut VM<'_>) -> RunResult<Text<Self>> {
        Ok(Text {
            body: char_text(value, vm)?,
            len: 1,
        })
    }

    fn float_type_error(type_name: &str) -> RunError {
        ExcType::type_error(format!("must be real number, not {type_name}"))
    }

    fn float_overflow_error() -> RunError {
        ExcType::overflow_int_to_float()
    }

    /// CPython reports the character (not byte) index in a `str` template,
    /// and quotes the character only within its `31..=126` window.
    fn unsupported(template: &str, index: usize) -> RunError {
        let conversion = template[index..].chars().next().unwrap_or('?');
        let shown = if ('\x1f'..='~').contains(&conversion) {
            conversion
        } else {
            '?'
        };
        unsupported_character(shown, conversion, template[..index].chars().count())
    }

    fn allocate(output: String, heap: &Heap) -> Value {
        allocate_string(output, heap)
    }
}

/// `bytes % args`.
struct BytesTarget;

impl Target for BytesTarget {
    type Template = [u8];
    type Output = Vec<u8>;
    type Builder<'t> = BytesBuilder<'t>;

    const IS_BYTES: bool = true;
    const PRECISION_TOO_BIG: &'static str = "prec too big";

    fn as_bytes(template: &[u8]) -> &[u8] {
        template
    }

    fn push_literal(builder: &mut BytesBuilder<'_>, template: &[u8], range: Range<usize>) -> RunResult<()> {
        Ok(builder.push_slice(&template[range])?)
    }

    fn key_value(template: &[u8], range: Range<usize>, heap: &Heap) -> Value {
        allocate_bytes(template[range].to_vec(), heap)
    }

    fn from_ascii(text: String) -> Vec<u8> {
        text.into_bytes()
    }

    fn byte_len(output: &Vec<u8>) -> usize {
        output.len()
    }

    /// Only ASCII text reaches a `bytes` template (`ascii()` output), so
    /// bytes and characters coincide.
    fn text_from_string(text: String, spec: &Directive, _tracker: &ResourceTracker) -> RunResult<Text<Self>> {
        Ok(Self::from_bytes(text.as_bytes(), spec.precision))
    }

    /// Only `bytes` values; Monty has no `bytearray`, `memoryview` or `__bytes__`.
    fn text_operand(value: &Value, spec: &Directive, vm: &mut VM<'_>) -> RunResult<Text<Self>> {
        match value {
            Value::InternBytes(id) => Ok(Self::from_bytes(vm.interns.get_bytes(*id), spec.precision)),
            Value::Ref(id) if let HeapData::Bytes(bytes) = vm.heap.get(*id) => {
                Ok(Self::from_bytes(bytes.as_slice(), spec.precision))
            }
            _ => Err(ExcType::type_error(format!(
                "%b requires a bytes-like object, or an object that implements __bytes__, not '{}'",
                value.py_type_name(vm)
            ))),
        }
    }

    fn char_operand(value: &Value, vm: &mut VM<'_>) -> RunResult<Text<Self>> {
        Ok(Self::from_bytes(&[byte_char(value, vm)?], None))
    }

    fn float_type_error(type_name: &str) -> RunError {
        ExcType::type_error(format!("float argument required, not {type_name}"))
    }

    /// CPython's bytes formatter folds the overflow into its generic type error.
    fn float_overflow_error() -> RunError {
        ExcType::type_error("float argument required, not int")
    }

    /// CPython quotes any ASCII byte as it is; a non-ASCII byte gets an odd
    /// `OverflowError` rather than the unsupported-character error.
    fn unsupported(template: &[u8], index: usize) -> RunError {
        let conversion = template[index];
        if conversion.is_ascii() {
            unsupported_character(char::from(conversion), char::from(conversion), index)
        } else {
            SimpleException::new_msg(ExcType::OverflowError, "character argument not in range(0x110000)").into()
        }
    }

    fn allocate(output: Vec<u8>, heap: &Heap) -> Value {
        allocate_bytes(output, heap)
    }
}

impl BytesTarget {
    /// A `bytes` fragment, copying only the first `precision` bytes.
    fn from_bytes(bytes: &[u8], precision: Option<usize>) -> Text<Self> {
        let len = precision.map_or(bytes.len(), |precision| precision.min(bytes.len()));
        Text {
            body: bytes[..len].to_vec(),
            len,
        }
    }
}

impl<'t> OutputBuilder<'t> for StringBuilder<'t> {
    type Output = String;

    fn from_existing(output: String, tracker: &'t ResourceTracker) -> Self {
        StringBuilder::from_existing(output, tracker)
    }

    fn with_capacity(capacity: usize, tracker: &'t ResourceTracker) -> RunResult<Self> {
        Ok(StringBuilder::with_capacity(capacity, tracker)?)
    }

    fn push_text(&mut self, text: &str) -> RunResult<()> {
        Ok(self.push_str(text)?)
    }

    fn push_fill(&mut self, fill: u8, count: usize) -> RunResult<()> {
        Ok(self.push_repeated(char::from(fill), count)?)
    }

    fn push_output(&mut self, fragment: &String) -> RunResult<()> {
        Ok(self.push_str(fragment)?)
    }

    fn finish(self) -> RunResult<String> {
        self.finish_raw()
    }
}

impl<'t> OutputBuilder<'t> for BytesBuilder<'t> {
    type Output = Vec<u8>;

    fn from_existing(output: Vec<u8>, tracker: &'t ResourceTracker) -> Self {
        BytesBuilder::from_existing(output, tracker)
    }

    fn with_capacity(capacity: usize, tracker: &'t ResourceTracker) -> RunResult<Self> {
        Ok(BytesBuilder::with_capacity(capacity, tracker)?)
    }

    fn push_text(&mut self, text: &str) -> RunResult<()> {
        Ok(self.push_slice(text.as_bytes())?)
    }

    fn push_fill(&mut self, fill: u8, count: usize) -> RunResult<()> {
        Ok(self.push_repeated(fill, count)?)
    }

    fn push_output(&mut self, fragment: &Vec<u8>) -> RunResult<()> {
        Ok(self.push_slice(fragment)?)
    }

    fn finish(self) -> RunResult<Vec<u8>> {
        Ok(BytesBuilder::finish(self))
    }
}

/// Walks the template, copying literal runs and rendering each `%` directive.
fn format_template<T: Target>(template: &T::Template, args: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let bytes = T::as_bytes(template);
    let arguments = Arguments::new(args, T::IS_BYTES, vm)?;
    defer_drop!(arguments, vm);
    let cursor = Cursor::default();
    defer_drop_mut!(cursor, vm);

    let mut output = T::Output::default();
    let mut index = 0;
    let mut steps = 0;
    while index < bytes.len() {
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let literal_end = find_percent(bytes, index, &vm.heap.tracker)?;
        output = extend::<T>(output, &vm.heap.tracker, |builder| {
            T::push_literal(builder, template, index..literal_end)
        })?;
        if literal_end == bytes.len() {
            break;
        }
        index = literal_end + 1;
        // Only an immediate `%%` is an escape: `%5%` is a directive whose conversion is `%`.
        if bytes.get(index) == Some(&b'%') {
            output = extend::<T>(output, &vm.heap.tracker, |builder| builder.push_text("%"))?;
            index += 1;
        } else {
            let (rendered, next) = render_directive::<T>(template, index, arguments, cursor, vm)?;
            // A directive that opens the template (the whole of `'%s' % s`) is the output so far.
            output = if T::byte_len(&output) == 0 {
                rendered
            } else {
                extend::<T>(output, &vm.heap.tracker, |builder| builder.push_output(&rendered))?
            };
            index = next;
        }
    }

    // A mapping operand never reports leftovers, so `'abc' % {}` is fine.
    if arguments.mapping.is_none() && cursor.next < arguments.positional.len() {
        let noun = if T::IS_BYTES { "bytes" } else { "string" };
        Err(ExcType::type_error(format!(
            "not all arguments converted during {noun} formatting"
        )))
    } else {
        Ok(T::allocate(output, vm.heap))
    }
}

/// Finds the next `%` at or after `start` (or the template's end), polling
/// the tracker's clock per chunk so a long literal run stays interruptible.
fn find_percent(bytes: &[u8], start: usize, tracker: &ResourceTracker) -> RunResult<usize> {
    for (chunk_index, chunk) in bytes[start..].chunks(SCAN_CHUNK).enumerate() {
        tracker.check_time_every(chunk_index)?;
        if let Some(offset) = chunk.iter().position(|byte| *byte == b'%') {
            return Ok(start + chunk_index * SCAN_CHUNK + offset);
        }
    }
    Ok(bytes.len())
}

/// Appends to `output` through a short-lived tracked builder, so the tracker
/// borrow ends before the VM is needed again.
fn extend<T: Target>(
    output: T::Output,
    tracker: &ResourceTracker,
    push: impl FnOnce(&mut T::Builder<'_>) -> RunResult<()>,
) -> RunResult<T::Output> {
    let mut builder = <T::Builder<'_>>::from_existing(output, tracker);
    push(&mut builder)?;
    builder.finish()
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
    fn new(args: &Value, bytes_template: bool, vm: &VM<'_>) -> RunResult<Self> {
        let positional = match args {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Tuple(tuple) => clone_items(tuple.as_slice(), vm)?,
                HeapData::NamedTuple(tuple) => clone_items(tuple.as_vec(), vm)?,
                _ => vec![args.clone_with_heap(vm.heap)],
            },
            _ => vec![args.clone_with_heap(vm.heap)],
        };
        let mapping = match args.py_type(vm) {
            Type::Dict | Type::DefaultDict | Type::Counter | Type::List | Type::Range => true,
            Type::Bytes => !bytes_template,
            _ => false,
        }
        .then(|| args.clone_with_heap(vm.heap));
        Ok(Self { positional, mapping })
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

/// Clones tuple items so the operand can be released before rendering ends,
/// preflighting the copy like other bulk container clones.
fn clone_items(items: &[Value], vm: &VM<'_>) -> RunResult<Vec<Value>> {
    vm.heap
        .tracker
        .check_allocation(items.len().saturating_mul(VALUE_SIZE))?;
    Ok(items.iter().map(|item| item.clone_with_heap(vm.heap)).collect())
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

/// Parses and renders the directive starting just after its `%`.
fn render_directive<T: Target>(
    template: &T::Template,
    start: usize,
    arguments: &Arguments,
    cursor: &mut Cursor,
    vm: &mut VM<'_>,
) -> RunResult<(T::Output, usize)> {
    let bytes = T::as_bytes(template);
    let mut index = start;
    // `%(key)` is looked up as soon as it is read, so a missing key reports
    // before a malformed spec does.
    if bytes.get(index) == Some(&b'(') {
        let key_end = find_key_end(bytes, index + 1, vm)?;
        let key = T::key_value(template, index + 1..key_end, vm.heap);
        lookup_key(key, arguments, cursor, vm)?;
        index = key_end + 1;
    }
    let parsed = parse_directive::<T>(bytes, index, arguments, cursor, vm)?;

    // The argument is taken before the conversion is checked, so `'%5%' % ()`
    // reports the missing argument rather than the bad conversion.
    let value = next_argument(arguments, cursor, vm)?;
    defer_drop!(value, vm);
    let spec = &parsed.spec;
    let rendered = match parsed.conversion {
        b's' => T::text_operand(value, spec, vm).and_then(|text| pad_text(text, spec, &vm.heap.tracker)),
        b'b' if T::IS_BYTES => T::text_operand(value, spec, vm).and_then(|text| pad_text(text, spec, &vm.heap.tracker)),
        b'r' | b'a' => repr_operand::<T>(value, parsed.conversion, spec, vm)
            .and_then(|text| pad_text(text, spec, &vm.heap.tracker)),
        b'c' => T::char_operand(value, vm).and_then(|text| pad_text(text, spec, &vm.heap.tracker)),
        b'd' | b'i' | b'u' => format_integer::<T>(value, 10, parsed.conversion, spec, vm),
        b'o' => format_integer::<T>(value, 8, b'o', spec, vm),
        b'x' => format_integer::<T>(value, 16, b'x', spec, vm),
        b'X' => format_integer::<T>(value, 16, b'X', spec, vm),
        b'e' => format_float::<T>(value, TypeChar::E, spec, vm),
        b'E' => format_float::<T>(value, TypeChar::EUpper, spec, vm),
        b'f' => format_float::<T>(value, TypeChar::F, spec, vm),
        b'F' => format_float::<T>(value, TypeChar::FUpper, spec, vm),
        b'g' => format_float::<T>(value, TypeChar::G, spec, vm),
        b'G' => format_float::<T>(value, TypeChar::GUpper, spec, vm),
        _ => Err(T::unsupported(template, parsed.end - 1)),
    }?;
    Ok((rendered, parsed.end))
}

/// Reads the flags, width, precision, optional C length modifier and
/// conversion byte, consuming the arguments a `*` width or precision names.
fn parse_directive<T: Target>(
    template: &[u8],
    start: usize,
    arguments: &Arguments,
    cursor: &mut Cursor,
    vm: &mut VM<'_>,
) -> RunResult<ParsedDirective> {
    let mut spec = Directive::default();
    let mut index = start;
    while let Some(flag) = template.get(index) {
        vm.heap.tracker.check_time_every(index)?;
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
        (spec.width, index) = parse_number(template, index, isize::MAX, "width too big", &vm.heap.tracker)?;
    }

    if template.get(index) == Some(&b'.') {
        index += 1;
        if template.get(index) == Some(&b'*') {
            let precision = star_operand(arguments, cursor, vm, ExcType::overflow_c_int)?;
            // The precision must fit a C `int`; a negative one clamps to zero.
            let precision = i32::try_from(precision).map_err(|_| ExcType::overflow_c_int())?;
            spec.precision = Some(usize::try_from(precision).unwrap_or(0));
            index += 1;
        } else {
            let (precision, next) = parse_number(
                template,
                index,
                i32::MAX as isize,
                T::PRECISION_TOO_BIG,
                &vm.heap.tracker,
            )?;
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

/// Parses a run of decimal digits, raising `message` above `max`: CPython
/// bounds a width by `ssize_t` but a precision by C `int`.
fn parse_number(
    template: &[u8],
    start: usize,
    max: isize,
    message: &str,
    tracker: &ResourceTracker,
) -> RunResult<(usize, usize)> {
    let mut index = start;
    let mut number = 0usize;
    while let Some(digit) = template.get(index).filter(|byte| byte.is_ascii_digit()) {
        tracker.check_time_every(index)?;
        number = number
            .checked_mul(10)
            .and_then(|number| number.checked_add(usize::from(digit - b'0')))
            .filter(|number| isize::try_from(*number).is_ok_and(|number| number <= max))
            .ok_or_else(|| value_error(message))?;
        index += 1;
    }
    Ok((number, index))
}

/// Looks `key` up in the mapping operand and makes the result the one
/// remaining argument, as CPython does. Takes ownership of `key`.
fn lookup_key(key: Value, arguments: &Arguments, cursor: &mut Cursor, vm: &mut VM<'_>) -> RunResult<()> {
    defer_drop!(key, vm);
    let Some(mapping) = &arguments.mapping else {
        return Err(ExcType::type_error("format requires a mapping"));
    };
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

/// The `%r` / `%a` operand: `repr()` or `ascii()` for a `str` template, and
/// always `ascii()` for a `bytes` template.
fn repr_operand<T: Target>(value: &Value, conversion: u8, spec: &Directive, vm: &mut VM<'_>) -> RunResult<Text<T>> {
    let ascii = T::IS_BYTES || conversion == b'a';
    let text = vm.convert_value(value, if ascii { 3 } else { 2 })?;
    T::text_from_string(text, spec, &vm.heap.tracker)
}

/// Pads a text fragment to the width: text right-aligns unless `-` was
/// given, and the `0` flag is ignored.
fn pad_text<T: Target>(text: Text<T>, spec: &Directive, tracker: &ResourceTracker) -> RunResult<T::Output> {
    pad::<T>("", "", text.body, text.len, false, spec, tracker)
}

/// Renders `%d`/`%i`/`%u` (base 10), `%o`, `%x` and `%X`: the magnitude in
/// `base`, zero-extended to the precision, then signed, prefixed and padded.
fn format_integer<T: Target>(
    value: &Value,
    base: u32,
    conversion: u8,
    spec: &Directive,
    vm: &mut VM<'_>,
) -> RunResult<T::Output> {
    let operand = integer_operand(value, conversion, vm)?;
    defer_drop!(operand, vm);
    // CPython checks this once the operand is an int, before rendering digits.
    if spec
        .precision
        .is_some_and(|precision| precision > MAX_INTEGER_PRECISION)
    {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "precision too large").into());
    }
    let uppercase = conversion == b'X';
    let (negative, digits) = magnitude_digits(operand, base, uppercase, vm)?;
    let digits = zero_extend(digits, spec.precision, &vm.heap.tracker)?;
    let prefix = match (spec.alternate, base) {
        (true, 8) => "0o",
        (true, 16) if uppercase => "0X",
        (true, 16) => "0x",
        _ => "",
    };
    let digits_len = digits.len();
    pad::<T>(
        number_sign(negative, spec),
        prefix,
        T::from_ascii(digits),
        digits_len,
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
    let max_digits = li.bits().div_ceil(u64::from(base.trailing_zeros().max(1)));
    check_repeat_size(
        1,
        usize::try_from(max_digits).unwrap_or(usize::MAX).saturating_add(2),
        tracker,
    )?;
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
        let mut output = StringBuilder::with_capacity(target, tracker)?;
        output.push_repeated('0', target - digits.len())?;
        output.push_str(&digits)?;
        output.finish_raw()
    }
}

/// Renders the `%e`/`%f`/`%g` family through the shared f-string formatters,
/// then re-signs and pads the digit text the printf way.
fn format_float<T: Target>(
    value: &Value,
    type_char: TypeChar,
    spec: &Directive,
    vm: &mut VM<'_>,
) -> RunResult<T::Output> {
    let number = float_operand::<T>(value, vm)?;
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
    let mut text = match type_char {
        TypeChar::E => format_float_e(number, &parsed, false, tracker),
        TypeChar::EUpper => format_float_e(number, &parsed, true, tracker),
        TypeChar::G | TypeChar::GUpper => format_float_g(number, &parsed, tracker),
        _ => format_float_f(number, &parsed, tracker),
    }?;
    // The formatter signs negatives itself; printf's flags decide the sign here.
    let negative = text.starts_with('-');
    if negative {
        text.remove(0);
    }
    let body_len = text.len();
    pad::<T>(
        number_sign(negative, spec),
        "",
        T::from_ascii(text),
        body_len,
        spec.zero,
        spec,
        tracker,
    )
}

/// Coerces a float directive's operand: floats and ints directly, big ints
/// when they fit a float, anything else through `__index__`.
fn float_operand<T: Target>(value: &Value, vm: &mut VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => match li.to_f64() {
            Some(f) if f.is_finite() => Ok(f),
            _ => Err(T::float_overflow_error()),
        },
        _ => {
            if let Some(index) = value.py_index_impl(vm)? {
                defer_drop!(index, vm);
                float_operand::<T>(index, vm)
            } else {
                Err(T::float_type_error(&value.py_type_name(vm)))
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
/// A body with nothing to add is returned as it is, so `'%s' % s` copies once.
fn pad<T: Target>(
    sign: &str,
    prefix: &str,
    body: T::Output,
    body_len: usize,
    zero_fill: bool,
    spec: &Directive,
    tracker: &ResourceTracker,
) -> RunResult<T::Output> {
    let padding = spec.width.saturating_sub(sign.len() + prefix.len() + body_len);
    if padding == 0 && sign.is_empty() && prefix.is_empty() {
        Ok(body)
    } else {
        // The width may come from an argument (`%*s`), so the reservation budgets the padding.
        let capacity = sign.len() + prefix.len() + T::byte_len(&body) + padding;
        let mut output = <T::Builder<'_>>::with_capacity(capacity, tracker)?;
        if spec.left {
            output.push_text(sign)?;
            output.push_text(prefix)?;
            output.push_output(&body)?;
            output.push_fill(b' ', padding)?;
        } else if zero_fill {
            output.push_text(sign)?;
            output.push_text(prefix)?;
            output.push_fill(b'0', padding)?;
            output.push_output(&body)?;
        } else {
            output.push_fill(b' ', padding)?;
            output.push_text(sign)?;
            output.push_text(prefix)?;
            output.push_output(&body)?;
        }
        output.finish()
    }
}

/// CPython's `unsupported format character` error: `shown` is the quoted
/// character (the target decides when that is `?`), `conversion` the one
/// reported in hex.
fn unsupported_character(shown: char, conversion: char, index: usize) -> RunError {
    value_error(format!(
        "unsupported format character '{shown}' (0x{:x}) at index {index}",
        u32::from(conversion)
    ))
}
