//! F-string evaluation support.
//!
//! This module handles the runtime evaluation of f-strings (formatted string literals).
//! F-strings can contain literal text and interpolated expressions with optional
//! conversion flags (`!s`, `!r`, `!a`) and format specifications.

use std::str::FromStr;

use crate::{
    evaluate::{return_ext_call, EvalResult, EvaluateExpr},
    exception_private::{exc_err_fmt, exc_fmt, ExcType},
    expressions::ExprLoc,
    heap::{Heap, HeapData},
    intern::{Interns, StringId},
    io::PrintWriter,
    resource::ResourceTracker,
    run_frame::RunResult,
    snapshot::AbstractSnapshotTracker,
    types::PyTrait,
    value::Value,
};

// ============================================================================
// F-string type definitions
// ============================================================================

/// Conversion flags for f-string interpolations.
///
/// These control how the value is converted to string before formatting:
/// - `None`: Use default string conversion (equivalent to `str()`)
/// - `Str` (`!s`): Explicitly call `str()`
/// - `Repr` (`!r`): Call `repr()` for debugging representation
/// - `Ascii` (`!a`): Call `ascii()` for ASCII-safe representation
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConversionFlag {
    #[default]
    None,
    /// `!s` - convert using `str()`
    Str,
    /// `!r` - convert using `repr()`
    Repr,
    /// `!a` - convert using `ascii()` (escapes non-ASCII characters)
    Ascii,
}

/// A single part of an f-string.
///
/// F-strings are composed of literal text segments and interpolated expressions.
/// For example, `f"Hello {name}!"` has three parts:
/// - `Literal("Hello ")`
/// - `Interpolation { expr: name, ... }`
/// - `Literal("!")`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FStringPart {
    /// Literal text segment (e.g., "Hello " in `f"Hello {name}"`)
    Literal(String),
    /// Interpolated expression with optional conversion and format spec
    Interpolation {
        /// The expression to evaluate
        expr: Box<ExprLoc>,
        /// Conversion flag: `None`, `!s` (str), `!r` (repr), `!a` (ascii)
        conversion: ConversionFlag,
        /// Optional format specification (can contain nested interpolations)
        format_spec: Option<FormatSpec>,
        /// Debug prefix for `=` specifier (e.g., "a=" for f'{a=}', " a = " for f'{ a = }').
        /// When present, this text is prepended to the output and repr conversion is used
        /// by default (unless an explicit conversion is specified).
        debug_prefix: Option<StringId>,
    },
}

/// Format specification for f-string interpolations.
///
/// Can be either a pre-parsed static spec or contain nested interpolations.
/// For example:
/// - `f"{value:>10}"` has `FormatSpec::Static(ParsedFormatSpec { ... })`
/// - `f"{value:{width}}"` has `FormatSpec::Dynamic` with the `width` variable
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FormatSpec {
    /// Pre-parsed static format spec (e.g., ">10s", ".2f")
    ///
    /// Parsing happens at parse time to avoid runtime string parsing overhead.
    /// Invalid specs cause a parse error immediately.
    Static(ParsedFormatSpec),
    /// Dynamic format spec with nested f-string parts
    ///
    /// These must be evaluated at runtime, then parsed into a `ParsedFormatSpec`.
    Dynamic(Vec<FStringPart>),
}

/// Parsed format specification following Python's format mini-language.
///
/// Format: `[[fill]align][sign][z][#][0][width][grouping_option][.precision][type]`
///
/// This struct is parsed at parse time for static format specs, avoiding runtime
/// string parsing. For dynamic format specs, parsing happens after evaluation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ParsedFormatSpec {
    /// Fill character for padding (default: space)
    pub fill: char,
    /// Alignment: '<' (left), '>' (right), '^' (center), '=' (sign-aware)
    pub align: Option<char>,
    /// Sign handling: '+' (always), '-' (negative only), ' ' (space for positive)
    pub sign: Option<char>,
    /// Whether to zero-pad numbers
    pub zero_pad: bool,
    /// Minimum field width
    pub width: usize,
    /// Precision for floats or max width for strings
    pub precision: Option<usize>,
    /// Type character: 's', 'd', 'f', 'e', 'g', etc.
    pub type_char: Option<char>,
}

impl FromStr for ParsedFormatSpec {
    type Err = String;

    /// Parses a format specification string into its components.
    ///
    /// Returns an error if the specifier contains invalid or unrecognized characters.
    /// The error includes the original specifier for use in error messages.
    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        if spec.is_empty() {
            return Ok(Self {
                fill: ' ',
                ..Default::default()
            });
        }

        let mut result = Self {
            fill: ' ',
            ..Default::default()
        };
        let mut chars = spec.chars().peekable();

        // Parse fill and align: [[fill]align]
        let first = chars.peek().copied();
        let second_pos = spec.chars().nth(1);

        if let Some(second) = second_pos {
            if matches!(second, '<' | '>' | '^' | '=') {
                // First char is fill, second is align
                result.fill = first.unwrap_or(' ');
                chars.next();
                result.align = chars.next();
            } else if matches!(first, Some('<' | '>' | '^' | '=')) {
                result.align = chars.next();
            }
        } else if matches!(first, Some('<' | '>' | '^' | '=')) {
            result.align = chars.next();
        }

        // Parse sign: +, -, or space
        if matches!(chars.peek(), Some('+' | '-' | ' ')) {
            result.sign = chars.next();
        }

        // Skip '#' (alternate form) for now
        if chars.peek() == Some(&'#') {
            chars.next();
        }

        // Parse zero-padding flag (must come before width)
        if chars.peek() == Some(&'0') {
            result.zero_pad = true;
            chars.next();
        }

        // Parse width
        let mut width_str = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                width_str.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if !width_str.is_empty() {
            result.width = width_str.parse().unwrap_or(0);
        }

        // Skip grouping option (comma or underscore)
        if matches!(chars.peek(), Some(',' | '_')) {
            chars.next();
        }

        // Parse precision: .N
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut prec_str = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    prec_str.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if !prec_str.is_empty() {
                result.precision = Some(prec_str.parse().unwrap_or(0));
            }
        }

        // Parse type character: s, d, f, e, g, etc.
        if let Some(&c) = chars.peek() {
            if matches!(
                c,
                's' | 'd' | 'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'n' | '%' | 'b' | 'o' | 'x' | 'X' | 'c'
            ) {
                result.type_char = Some(c);
                chars.next();
            }
        }

        // Error if there are any unconsumed characters
        if chars.peek().is_some() {
            return Err(spec.to_owned());
        }

        Ok(result)
    }
}

// ============================================================================
// F-string evaluation
// ============================================================================

/// Type category for format spec validation.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ValueType {
    Int,
    Float,
    String,
    Other,
}

impl ValueType {
    fn from_value(value: &Value, heap: &Heap<impl ResourceTracker>) -> Self {
        match value {
            Value::Int(_) => ValueType::Int,
            Value::Float(_) => ValueType::Float,
            Value::InternString(_) => ValueType::String,
            Value::Ref(id) => match heap.get(*id) {
                HeapData::Str(_) => ValueType::String,
                _ => ValueType::Other,
            },
            _ => ValueType::Other,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::String => "str",
            Self::Other => "object",
        }
    }
}

/// Processes a single f-string interpolation, appending the result to the output string.
///
/// This function handles:
/// 1. Evaluating the expression
/// 2. Applying conversion flags (!s, !r, !a)
/// 3. Applying format specifications (static or dynamic)
/// 4. Appending the formatted result to the output
///
/// # Arguments
/// * `evaluator` - The evaluator instance for expression evaluation
/// * `result` - The output string to append to
/// * `value` - The evaluated expression value
/// * `conversion` - The conversion flag to apply
/// * `format_spec` - Optional format specification
///
/// # Returns
/// `Ok(())` on success, or an error if formatting fails.
/// The caller is responsible for dropping `value` after this function returns.
pub(crate) fn fstring_interpolation(
    evaluator: &mut EvaluateExpr<'_, '_, impl ResourceTracker, impl PrintWriter, impl AbstractSnapshotTracker>,
    result: &mut String,
    value: &Value,
    conversion: ConversionFlag,
    format_spec: Option<&FormatSpec>,
) -> RunResult<EvalResult<()>> {
    // 1. Get the value type for format spec validation
    // When a conversion flag is used (!s, !r, !a), the result is always a string,
    // so we should validate against String type, not the original value type.
    let value_type = if conversion == ConversionFlag::None {
        ValueType::from_value(value, evaluator.heap)
    } else {
        ValueType::String
    };

    // 2. Apply conversion flag (str, repr, ascii)
    // TODO this is really ugly we go value -> str -> parse the string to see if it's numeric!
    let converted = apply_conversion(value, conversion, evaluator.heap, evaluator.interns);

    // 3. Apply format specification if present
    if let Some(spec) = format_spec {
        match spec {
            FormatSpec::Static(parsed) => {
                // Pre-parsed at parse time - use directly
                apply_format_spec(result, &converted, parsed, value_type)?;
            }
            FormatSpec::Dynamic(parts) => {
                // Evaluate dynamic parts, then parse
                let spec_str = return_ext_call!(evaluate_dynamic_format_spec(evaluator, parts)?);
                if let Ok(parsed) = spec_str.parse() {
                    apply_format_spec(result, &converted, &parsed, value_type)?;
                } else {
                    return exc_err_fmt!(
                        ExcType::ValueError;
                        "Invalid format specifier '{}' for object of type '{}'",
                        &spec_str,
                        value_type.name()
                    );
                }
            }
        }
    } else {
        result.push_str(&converted);
    }

    Ok(EvalResult::Value(()))
}

/// Applies a conversion flag to a value, returning the string representation.
///
/// - None: Uses `py_str()` (default string conversion)
/// - Str (`!s`): Explicitly uses `py_str()`
/// - Repr (`!r`): Uses `py_repr()` for debugging representation
/// - Ascii (`!a`): Uses `py_repr()` and escapes non-ASCII characters
fn apply_conversion(
    value: &Value,
    conversion: ConversionFlag,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> String {
    match conversion {
        ConversionFlag::None | ConversionFlag::Str => value.py_str(heap, interns).into_owned(),
        ConversionFlag::Repr => value.py_repr(heap, interns).into_owned(),
        ConversionFlag::Ascii => {
            // ASCII conversion: like repr but escapes non-ASCII characters
            let repr = value.py_repr(heap, interns);
            escape_non_ascii(&repr)
        }
    }
}

/// Evaluates a dynamic format specification containing interpolated expressions.
///
/// Evaluates each part and concatenates the results into a format spec string,
/// which is then parsed into a `ParsedFormatSpec` at runtime.
fn evaluate_dynamic_format_spec(
    evaluator: &mut EvaluateExpr<'_, '_, impl ResourceTracker, impl PrintWriter, impl AbstractSnapshotTracker>,
    parts: &[FStringPart],
) -> RunResult<EvalResult<String>> {
    let mut result = String::new();
    for part in parts {
        match part {
            FStringPart::Literal(s) => result.push_str(s),
            FStringPart::Interpolation {
                expr,
                conversion,
                debug_prefix: _,
                ..
            } => {
                // Note: debug_prefix is ignored in format specs - it's only used at the top level
                let value = return_ext_call!(evaluator.evaluate_use(expr)?);
                let converted = apply_conversion(&value, *conversion, evaluator.heap, evaluator.interns);
                result.push_str(&converted);
                value.drop_with_heap(evaluator.heap);
            }
        }
    }
    Ok(EvalResult::Value(result))
}

/// Validates that a format specification is valid for the given value type.
///
/// Returns a `ValueError` if the format spec is incompatible with the value type,
/// matching CPython's error messages exactly.
fn validate_format_spec(spec: &ParsedFormatSpec, value_type: ValueType) -> RunResult<()> {
    // Check '=' alignment - only valid for numeric types
    if spec.align == Some('=') && !matches!(value_type, ValueType::Int | ValueType::Float) {
        return Err(exc_fmt!(ExcType::ValueError; "'=' alignment not allowed in string format specifier").into());
    }

    // Check type character compatibility
    if let Some(type_char) = spec.type_char {
        match type_char {
            // Integer-only format types
            'd' | 'b' | 'o' | 'x' | 'X' | 'c' => {
                if value_type != ValueType::Int {
                    return Err(exc_fmt!(
                        ExcType::ValueError;
                        "Unknown format code '{}' for object of type '{}'",
                        type_char,
                        value_type.name()
                    )
                    .into());
                }
            }
            // Numeric format types (int or float)
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'n' | '%' => {
                if !matches!(value_type, ValueType::Int | ValueType::Float) {
                    return Err(exc_fmt!(
                        ExcType::ValueError;
                        "Unknown format code '{}' for object of type '{}'",
                        type_char,
                        value_type.name()
                    )
                    .into());
                }
            }
            // String format type
            's' => {
                if value_type != ValueType::String {
                    return Err(exc_fmt!(
                        ExcType::ValueError;
                        "Unknown format code '{}' for object of type '{}'",
                        type_char,
                        value_type.name()
                    )
                    .into());
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Applies a pre-parsed format specification to a converted value.
///
/// Supports the Python format mini-language:
/// - Fill and alignment: `<` (left), `>` (right), `^` (center), `=` (sign-aware)
/// - Sign: `+` (always), `-` (negative only), space (space for positive)
/// - Width: Minimum field width
/// - Precision: `.N` for float decimal places or string truncation
/// - Type: `f` (fixed-point), `d` (integer), `s` (string), `e` (exponential)
///
/// Returns a `RunError` if the format spec is invalid for the given value type,
/// matching CPython's error messages.
fn apply_format_spec(write: &mut String, value: &str, spec: &ParsedFormatSpec, value_type: ValueType) -> RunResult<()> {
    // Validate format spec against value type
    validate_format_spec(spec, value_type)?;

    // Determine if this is a numeric value by trying to parse it
    let is_numeric = value.parse::<f64>().is_ok();

    // Format the value based on type
    let formatted = if is_numeric {
        format_numeric(value, spec)
    } else {
        format_string(value, spec)
    };

    // Apply width and alignment
    apply_width_alignment(write, &formatted, spec, is_numeric);
    Ok(())
}

/// Formats a numeric value according to the format spec.
fn format_numeric(value: &str, spec: &ParsedFormatSpec) -> String {
    // Try to parse as float first (handles both int and float)
    let num: f64 = match value.parse() {
        Ok(n) => n,
        Err(_) => return value.to_string(),
    };

    // Determine sign string
    let sign_str = if num < 0.0 {
        "-"
    } else {
        match spec.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    };

    let abs_num = num.abs();

    // Format based on type character
    let num_str = match spec.type_char {
        Some('f' | 'F') => {
            // Fixed-point notation
            let precision = spec.precision.unwrap_or(6);
            format!("{abs_num:.precision$}")
        }
        Some('e') => {
            // Exponential notation (lowercase)
            let precision = spec.precision.unwrap_or(6);
            format_exponential(abs_num, precision, false)
        }
        Some('E') => {
            // Exponential notation (uppercase)
            let precision = spec.precision.unwrap_or(6);
            format_exponential(abs_num, precision, true)
        }
        Some(g @ ('g' | 'G')) => {
            // General format - uses exponential if exponent < -4 or >= precision
            let precision = spec.precision.unwrap_or(6).max(1);
            let uppercase = g == 'G';
            format_general(abs_num, precision, uppercase)
        }
        Some('d') => {
            // Integer format
            format!("{}", abs_num as i64)
        }
        Some('%') => {
            // Percentage
            let precision = spec.precision.unwrap_or(6);
            format!("{:.precision$}%", abs_num * 100.0)
        }
        _ => {
            // Default: use precision if specified, otherwise keep as-is
            if let Some(precision) = spec.precision {
                format!("{abs_num:.precision$}")
            } else {
                format!("{abs_num}")
            }
        }
    };

    format!("{sign_str}{num_str}")
}

/// Formats a string value according to the format spec.
fn format_string(value: &str, spec: &ParsedFormatSpec) -> String {
    // Apply precision as truncation for strings
    // Use chars().count() for correct Unicode character counting
    if let Some(precision) = spec.precision {
        if value.chars().count() > precision {
            return value.chars().take(precision).collect();
        }
    }
    value.to_string()
}

/// Applies width and alignment to a formatted value.
fn apply_width_alignment(write: &mut String, value: &str, spec: &ParsedFormatSpec, is_numeric: bool) {
    // Use chars().count() for correct Unicode character counting
    let char_count = value.chars().count();
    if spec.width == 0 || spec.width <= char_count {
        write.push_str(value);
        return;
    }

    let padding = spec.width - char_count;

    // Determine fill character (zero-pad overrides fill for numbers)
    let fill = if spec.zero_pad && is_numeric && spec.align.is_none() {
        '0'
    } else {
        spec.fill
    };

    // Determine alignment:
    // - When zero_pad is true and no explicit align, use '=' (sign-aware) for numbers
    // - Otherwise default is '>' for numbers, '<' for strings
    let align = spec.align.unwrap_or(if spec.zero_pad && is_numeric {
        '='
    } else if is_numeric {
        '>'
    } else {
        '<'
    });

    match align {
        '<' => {
            // Left align
            write.push_str(value);
            push_str_repeat(write, fill, padding);
        }
        '>' => {
            // Right align
            push_str_repeat(write, fill, padding);
            write.push_str(value);
        }
        '^' => {
            // Center align
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;
            push_str_repeat(write, fill, left_pad);
            write.push_str(value);
            push_str_repeat(write, fill, right_pad);
        }
        '=' => {
            // Sign-aware padding: padding goes after sign
            // Use chars() for safety even though signs are currently single-byte ASCII
            if is_numeric && (value.starts_with('-') || value.starts_with('+') || value.starts_with(' ')) {
                let mut chars = value.chars();
                let sign = chars.next().unwrap_or_default();
                write.push(sign);
                push_str_repeat(write, fill, padding);
                for c in chars {
                    write.push(c);
                }
            } else {
                // No sign, treat as right-align
                push_str_repeat(write, fill, padding);
                write.push_str(value);
            }
        }
        _ => write.push_str(value),
    }
}

fn push_str_repeat(write: &mut String, fill: char, padding: usize) {
    for _ in 0..padding {
        write.push(fill);
    }
}

/// Formats a number in Python's general format (:g/:G).
///
/// Uses exponential notation if exponent < -4 or >= precision,
/// otherwise uses fixed-point notation. Trailing zeros are trimmed.
fn format_general(num: f64, precision: usize, uppercase: bool) -> String {
    if num == 0.0 {
        return "0".to_string();
    }

    // Calculate the exponent
    let exp = num.abs().log10().floor() as i32;

    // Python uses exponential when exp < -4 or exp >= precision
    if exp < -4 || exp >= precision as i32 {
        // Use exponential notation with (precision - 1) decimal places
        let exp_precision = precision.saturating_sub(1);
        let formatted = format_exponential(num, exp_precision, uppercase);
        // Trim trailing zeros from mantissa (but keep at least one digit after decimal)
        trim_exponential_zeros(&formatted)
    } else {
        // Use fixed-point notation
        // Precision for g/G is total significant digits, not decimal places
        let decimal_places = (precision as i32 - exp - 1).max(0) as usize;
        let formatted = format!("{num:.decimal_places$}");
        trim_trailing_zeros(&formatted)
    }
}

/// Trims trailing zeros from the mantissa of an exponential number.
fn trim_exponential_zeros(s: &str) -> String {
    let e_char = if s.contains('E') { 'E' } else { 'e' };
    if let Some(e_pos) = s.find(e_char) {
        let (mantissa, exp_part) = s.split_at(e_pos);
        let trimmed = trim_trailing_zeros(mantissa);
        format!("{trimmed}{exp_part}")
    } else {
        s.to_string()
    }
}

/// Formats a number in Python-compatible exponential notation.
///
/// Python's exponential format differs from Rust's:
/// - Always shows sign on exponent (e.g., `e+03` not `e3`)
/// - Exponent is at least 2 digits (e.g., `e+03` not `e+3`)
fn format_exponential(num: f64, precision: usize, uppercase: bool) -> String {
    // Use Rust's exponential format as a starting point
    let formatted = if uppercase {
        format!("{num:.precision$E}")
    } else {
        format!("{num:.precision$e}")
    };

    // Find the 'e' or 'E' position
    let e_char = if uppercase { 'E' } else { 'e' };
    if let Some(e_pos) = formatted.find(e_char) {
        let (mantissa, exp_part) = formatted.split_at(e_pos);
        let exp_str = &exp_part[1..]; // Skip the 'e'/'E'

        // Parse the exponent
        let exp: i32 = exp_str.parse().unwrap_or(0);

        // Format with explicit sign and at least 2 digits
        let exp_formatted = if exp >= 0 {
            format!("{e_char}+{exp:02}")
        } else {
            format!("{e_char}{exp:03}") // -XX format (sign + 2 digits)
        };

        format!("{mantissa}{exp_formatted}")
    } else {
        formatted
    }
}

/// Trims trailing zeros after the decimal point.
fn trim_trailing_zeros(s: &str) -> String {
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0');
        if let Some(stripped) = trimmed.strip_suffix('.') {
            stripped.to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        s.to_string()
    }
}

/// Escapes non-ASCII characters in a string (for `!a` conversion).
///
/// Characters are escaped as:
/// - `\xNN` for codepoints <= 0xFF
/// - `\uNNNN` for codepoints <= 0xFFFF
/// - `\UNNNNNNNN` for codepoints > 0xFFFF
fn escape_non_ascii(s: &str) -> String {
    use std::fmt::Write;

    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            result.push(c);
        } else {
            let cp = c as u32;
            if cp <= 0xFF {
                write!(result, "\\x{cp:02x}").unwrap();
            } else if cp <= 0xFFFF {
                write!(result, "\\u{cp:04x}").unwrap();
            } else {
                write!(result, "\\U{cp:08x}").unwrap();
            }
        }
    }
    result
}
