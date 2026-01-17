//! F-string type definitions and formatting functions.
//!
//! This module contains the AST types for f-strings (formatted string literals)
//! and the runtime formatting functions used by the bytecode VM.
//!
//! F-strings can contain literal text and interpolated expressions with optional
//! conversion flags (`!s`, `!r`, `!a`) and format specifications.

use std::str::FromStr;

use crate::{
    exception_private::{ExcType, RunError, SimpleException},
    expressions::ExprLoc,
    heap::Heap,
    intern::{Interns, StringId},
    resource::ResourceTracker,
    types::{PyTrait, Type},
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
/// - `Literal(interned_hello)` (StringId for "Hello ")
/// - `Interpolation { expr: name, ... }`
/// - `Literal(interned_exclaim)` (StringId for "!")
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FStringPart {
    /// Literal text segment (e.g., "Hello " in `f"Hello {name}"`)
    /// The StringId references the interned string in the Interns table.
    Literal(StringId),
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
        if let Some(&c) = chars.peek()
            && matches!(
                c,
                's' | 'd' | 'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'n' | '%' | 'b' | 'o' | 'x' | 'X' | 'c'
            )
        {
            result.type_char = Some(c);
            chars.next();
        }

        // Error if there are any unconsumed characters
        if chars.peek().is_some() {
            return Err(spec.to_owned());
        }

        Ok(result)
    }
}

// ============================================================================
// Format errors
// ============================================================================

/// Error type for format specification failures.
///
/// These errors are returned from formatting functions and should be converted
/// to appropriate Python exceptions (usually ValueError) by the VM.
#[derive(Debug, Clone)]
pub enum FormatError {
    /// Invalid alignment for the given type (e.g., '=' alignment on strings).
    InvalidAlignment(String),
    /// Value out of range (e.g., character code > 0x10FFFF).
    Overflow(String),
    /// Generic value error (e.g., invalid base, invalid Unicode).
    ValueError(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAlignment(msg) | Self::Overflow(msg) | Self::ValueError(msg) => {
                write!(f, "{msg}")
            }
        }
    }
}

/// Formats a value according to a format specification, applying type-appropriate formatting.
///
/// Dispatches to the appropriate formatting function based on the value type and format spec:
/// - Integers: `format_int`, `format_int_base`, `format_char`
/// - Floats: `format_float_f`, `format_float_e`, `format_float_g`, `format_float_percent`
/// - Strings: `format_string`
///
/// Returns a `ValueError` if the format type character is incompatible with the value type.
pub fn format_with_spec(
    value: &Value,
    spec: &ParsedFormatSpec,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> Result<String, RunError> {
    let value_type = value.py_type(heap);

    match (value, spec.type_char) {
        // Integer formatting
        (Value::Int(n), None | Some('d')) => Ok(format_int(*n, spec)),
        (Value::Int(n), Some('b')) => Ok(format_int_base(*n, 2, spec)?),
        (Value::Int(n), Some('o')) => Ok(format_int_base(*n, 8, spec)?),
        (Value::Int(n), Some('x')) => Ok(format_int_base(*n, 16, spec)?),
        (Value::Int(n), Some('X')) => Ok(format_int_base(*n, 16, spec)?.to_uppercase()),
        (Value::Int(n), Some('c')) => Ok(format_char(*n, spec)?),

        // Float formatting
        (Value::Float(f), None | Some('g' | 'G')) => Ok(format_float_g(*f, spec)),
        (Value::Float(f), Some('f' | 'F')) => Ok(format_float_f(*f, spec)),
        (Value::Float(f), Some('e')) => Ok(format_float_e(*f, spec, false)),
        (Value::Float(f), Some('E')) => Ok(format_float_e(*f, spec, true)),
        (Value::Float(f), Some('%')) => Ok(format_float_percent(*f, spec)),

        // Int to float formatting (Python allows this)
        (Value::Int(n), Some('f' | 'F')) => Ok(format_float_f(*n as f64, spec)),
        (Value::Int(n), Some('e')) => Ok(format_float_e(*n as f64, spec, false)),
        (Value::Int(n), Some('E')) => Ok(format_float_e(*n as f64, spec, true)),
        (Value::Int(n), Some('g' | 'G')) => Ok(format_float_g(*n as f64, spec)),
        (Value::Int(n), Some('%')) => Ok(format_float_percent(*n as f64, spec)),

        // String formatting (including InternString and heap strings)
        (_, None | Some('s')) if value_type == Type::Str => {
            let s = value.py_str(heap, interns);
            Ok(format_string(&s, spec)?)
        }

        // Bool as int
        (Value::Bool(b), Some('d')) => Ok(format_int(i64::from(*b), spec)),

        // No type specifier: convert to string and format
        (_, None) => {
            let s = value.py_str(heap, interns);
            Ok(format_string(&s, spec)?)
        }

        // Type mismatch errors
        (_, Some(c)) => Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("Unknown format code '{c}' for object of type '{value_type}'"),
        )
        .into()),
    }
}

/// Encodes a ParsedFormatSpec into a u64 for storage in bytecode constants.
///
/// Encoding layout (fits in 48 bits):
/// - bits 0-7: fill character (as ASCII, default space=32)
/// - bits 8-10: align (0=none, 1='<', 2='>', 3='^', 4='=')
/// - bits 11-12: sign (0=none, 1='+', 2='-', 3=' ')
/// - bit 13: zero_pad
/// - bits 14-29: width (16 bits, max 65535)
/// - bits 30-45: precision (16 bits, using 0xFFFF as "no precision")
/// - bits 46-50: type_char (0=none, 1-15=explicit type mapping: b,c,d,e,E,f,F,g,G,n,o,s,x,X,%)
pub fn encode_format_spec(spec: &ParsedFormatSpec) -> u64 {
    let fill = spec.fill as u64;
    let align = match spec.align {
        None => 0u64,
        Some('<') => 1,
        Some('>') => 2,
        Some('^') => 3,
        Some('=') => 4,
        Some(_) => 0,
    };
    let sign = match spec.sign {
        None => 0u64,
        Some('+') => 1,
        Some('-') => 2,
        Some(' ') => 3,
        Some(_) => 0,
    };
    let zero_pad = u64::from(spec.zero_pad);
    let width = spec.width as u64;
    let precision = spec.precision.map_or(0xFFFFu64, |p| p as u64);
    let type_char = spec.type_char.map_or(0u64, |c| match c {
        'b' => 1,
        'c' => 2,
        'd' => 3,
        'e' => 4,
        'E' => 5,
        'f' => 6,
        'F' => 7,
        'g' => 8,
        'G' => 9,
        'n' => 10,
        'o' => 11,
        's' => 12,
        'x' => 13,
        'X' => 14,
        '%' => 15,
        _ => 0,
    });

    fill | (align << 8) | (sign << 11) | (zero_pad << 13) | (width << 14) | (precision << 30) | (type_char << 46)
}

/// Decodes a u64 back into a ParsedFormatSpec.
///
/// Reverses the bit-packing done by `encode_format_spec`. Used by the VM
/// when executing `FormatValue` to retrieve the format specification from
/// the constant pool (where it's stored as a negative integer marker).
pub fn decode_format_spec(encoded: u64) -> ParsedFormatSpec {
    let fill = (encoded & 0xFF) as u8 as char;
    let align_bits = (encoded >> 8) & 0x07;
    let sign_bits = (encoded >> 11) & 0x03;
    let zero_pad = ((encoded >> 13) & 0x01) != 0;
    let width = ((encoded >> 14) & 0xFFFF) as usize;
    let precision_raw = ((encoded >> 30) & 0xFFFF) as usize;
    let type_bits = ((encoded >> 46) & 0x1F) as u8;

    let align = match align_bits {
        1 => Some('<'),
        2 => Some('>'),
        3 => Some('^'),
        4 => Some('='),
        _ => None,
    };

    let sign = match sign_bits {
        1 => Some('+'),
        2 => Some('-'),
        3 => Some(' '),
        _ => None,
    };

    let precision = if precision_raw == 0xFFFF {
        None
    } else {
        Some(precision_raw)
    };

    let type_char = match type_bits {
        1 => Some('b'),
        2 => Some('c'),
        3 => Some('d'),
        4 => Some('e'),
        5 => Some('E'),
        6 => Some('f'),
        7 => Some('F'),
        8 => Some('g'),
        9 => Some('G'),
        10 => Some('n'),
        11 => Some('o'),
        12 => Some('s'),
        13 => Some('x'),
        14 => Some('X'),
        15 => Some('%'),
        _ => None,
    };

    ParsedFormatSpec {
        fill,
        align,
        sign,
        zero_pad,
        width,
        precision,
        type_char,
    }
}

// ============================================================================
// Formatting functions
// ============================================================================

/// Formats a string value according to a format specification.
///
/// Applies the following transformations in order:
/// 1. Truncation: If `precision` is set, limits the string to that many characters
/// 2. Alignment: Pads to `width` using `fill` character (default left-aligned for strings)
///
/// Returns an error if `=` alignment is used (sign-aware padding only valid for numbers).
pub fn format_string(value: &str, spec: &ParsedFormatSpec) -> Result<String, FormatError> {
    // Handle precision (string truncation)
    let value = if let Some(prec) = spec.precision {
        value.chars().take(prec).collect::<String>()
    } else {
        value.to_owned()
    };

    // Validate alignment for strings (= is only for numbers)
    if spec.align == Some('=') {
        return Err(FormatError::InvalidAlignment(
            "'=' alignment not allowed in string format specifier".to_owned(),
        ));
    }

    // Default alignment for strings is left ('<')
    let align = spec.align.unwrap_or('<');
    Ok(pad_string(&value, spec.width, align, spec.fill))
}

/// Formats an integer in decimal with a format specification.
///
/// Applies the following:
/// - Sign prefix based on `sign` spec: `+` (always show), `-` (negatives only), ` ` (space for positive)
/// - Zero-padding: When `zero_pad` is true or `=` alignment, inserts zeros between sign and digits
/// - Alignment: Right-aligned by default for numbers, pads to `width` with `fill` character
pub fn format_int(n: i64, spec: &ParsedFormatSpec) -> String {
    let is_negative = n < 0;
    let abs_str = n.abs().to_string();

    // Build the sign prefix
    let sign = if is_negative {
        "-"
    } else {
        match spec.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    };

    // Default alignment for numbers is right ('>')
    let align = spec.align.unwrap_or('>');

    // Handle sign-aware zero-padding or regular padding
    if spec.zero_pad || align == '=' {
        let fill = if spec.zero_pad { '0' } else { spec.fill };
        let total_len = sign.len() + abs_str.len();
        if spec.width > total_len {
            let padding = spec.width - total_len;
            let pad_str: String = std::iter::repeat_n(fill, padding).collect();
            format!("{sign}{pad_str}{abs_str}")
        } else {
            format!("{sign}{abs_str}")
        }
    } else {
        let value = format!("{sign}{abs_str}");
        pad_string(&value, spec.width, align, spec.fill)
    }
}

/// Formats an integer in binary (base 2), octal (base 8), or hexadecimal (base 16).
///
/// Used for format types `b`, `o`, `x`, and `X`. The sign is prepended for negative numbers.
/// Does not include base prefixes like `0b`, `0o`, `0x` (those require the `#` flag which
/// is not yet implemented). Returns an error for invalid base values.
pub fn format_int_base(n: i64, base: u32, spec: &ParsedFormatSpec) -> Result<String, FormatError> {
    let is_negative = n < 0;
    let abs_val = n.unsigned_abs();

    let abs_str = match base {
        2 => format!("{abs_val:b}"),
        8 => format!("{abs_val:o}"),
        16 => format!("{abs_val:x}"),
        _ => return Err(FormatError::ValueError("Invalid base".to_owned())),
    };

    let sign = if is_negative { "-" } else { "" };
    let value = format!("{sign}{abs_str}");

    let align = spec.align.unwrap_or('>');
    Ok(pad_string(&value, spec.width, align, spec.fill))
}

/// Formats an integer as a Unicode character (format type `c`).
///
/// Converts the integer to its corresponding Unicode code point. Valid range is 0 to 0x10FFFF.
/// Returns `Overflow` error if out of range, `ValueError` if not a valid Unicode scalar value
/// (e.g., surrogate code points). Left-aligned by default like strings.
pub fn format_char(n: i64, spec: &ParsedFormatSpec) -> Result<String, FormatError> {
    if !(0..=0x0010_FFFF).contains(&n) {
        return Err(FormatError::Overflow("%c arg not in range(0x110000)".to_owned()));
    }
    let n_u32 = u32::try_from(n).expect("format_char n validated in 0..=0x10FFFF range");
    let c = char::from_u32(n_u32).ok_or_else(|| FormatError::ValueError("Invalid Unicode code point".to_owned()))?;
    let value = c.to_string();
    let align = spec.align.unwrap_or('<');
    Ok(pad_string(&value, spec.width, align, spec.fill))
}

/// Formats a float in fixed-point notation (format types `f` and `F`).
///
/// Always includes a decimal point with `precision` digits after it (default 6).
/// Handles sign prefix, zero-padding between sign and digits when `zero_pad` or `=` alignment.
/// Right-aligned by default. NaN and infinity are formatted as `nan`/`inf` (or `NAN`/`INF` for `F`).
pub fn format_float_f(f: f64, spec: &ParsedFormatSpec) -> String {
    let precision = spec.precision.unwrap_or(6);
    let is_negative = f.is_sign_negative() && !f.is_nan();
    let abs_val = f.abs();

    let abs_str = format!("{abs_val:.precision$}");

    let sign = if is_negative {
        "-"
    } else {
        match spec.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    };

    let align = spec.align.unwrap_or('>');

    if spec.zero_pad || align == '=' {
        let fill = if spec.zero_pad { '0' } else { spec.fill };
        let total_len = sign.len() + abs_str.len();
        if spec.width > total_len {
            let padding = spec.width - total_len;
            let pad_str: String = std::iter::repeat_n(fill, padding).collect();
            format!("{sign}{pad_str}{abs_str}")
        } else {
            format!("{sign}{abs_str}")
        }
    } else {
        let value = format!("{sign}{abs_str}");
        pad_string(&value, spec.width, align, spec.fill)
    }
}

/// Formats a float in exponential/scientific notation (format types `e` and `E`).
///
/// Produces output like `1.234568e+03` with `precision` digits after decimal (default 6).
/// The `uppercase` parameter controls whether to use `E` or `e` for the exponent marker.
/// Exponent is always formatted with a sign and at least 2 digits (Python convention).
pub fn format_float_e(f: f64, spec: &ParsedFormatSpec, uppercase: bool) -> String {
    let precision = spec.precision.unwrap_or(6);
    let is_negative = f.is_sign_negative() && !f.is_nan();
    let abs_val = f.abs();

    let abs_str = if uppercase {
        format!("{abs_val:.precision$E}")
    } else {
        format!("{abs_val:.precision$e}")
    };

    // Fix exponent format to match Python (e+03 not e3)
    let abs_str = fix_exp_format(&abs_str);

    let sign = if is_negative {
        "-"
    } else {
        match spec.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    };

    let value = format!("{sign}{abs_str}");
    let align = spec.align.unwrap_or('>');
    pad_string(&value, spec.width, align, spec.fill)
}

/// Formats a float in "general" format (format types `g` and `G`).
///
/// Chooses between fixed-point and exponential notation based on the magnitude:
/// - Uses exponential if exponent < -4 or >= precision
/// - Otherwise uses fixed-point notation
///
/// Unlike `f` and `e` formats, trailing zeros are stripped from the result.
/// Default precision is 6, but minimum is 1 significant digit.
pub fn format_float_g(f: f64, spec: &ParsedFormatSpec) -> String {
    let precision = spec.precision.unwrap_or(6).max(1);
    let is_negative = f.is_sign_negative() && !f.is_nan();
    let abs_val = f.abs();

    // Python's g format: use exponential if exponent < -4 or >= precision
    let exp = if abs_val == 0.0 {
        0
    } else {
        // log10 of valid floats fits in i32; floor() returns a finite f64
        f64_to_i32_trunc(abs_val.log10().floor())
    };

    // precision is typically small (default 6), safe to convert to i32
    let prec_i32 = i32::try_from(precision).unwrap_or(i32::MAX);
    let abs_str = if exp < -4 || exp >= prec_i32 {
        // Use exponential notation
        let exp_prec = precision.saturating_sub(1);
        let formatted = format!("{abs_val:.exp_prec$e}");
        // Python strips trailing zeros from the mantissa
        strip_trailing_zeros_exp(&formatted)
    } else {
        // Use fixed notation - result is non-negative due to .max(0)
        let sig_digits_i32 = (prec_i32 - exp - 1).max(0);
        let sig_digits = usize::try_from(sig_digits_i32).expect("sig_digits guaranteed non-negative");
        let formatted = format!("{abs_val:.sig_digits$}");
        strip_trailing_zeros(&formatted)
    };

    let sign = if is_negative {
        "-"
    } else {
        match spec.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    };

    let value = format!("{sign}{abs_str}");
    let align = spec.align.unwrap_or('>');
    pad_string(&value, spec.width, align, spec.fill)
}

/// Applies ASCII conversion to a string (escapes non-ASCII characters).
///
/// Used for the `!a` conversion flag in f-strings. Takes a string (typically a repr)
/// and escapes all non-ASCII characters using `\xNN`, `\uNNNN`, or `\UNNNNNNNN`.
pub fn ascii_escape(s: &str) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for c in s.chars() {
        if c.is_ascii() {
            result.push(c);
        } else {
            let code = c as u32;
            if code <= 0xFF {
                write!(result, "\\x{code:02x}")
            } else if code <= 0xFFFF {
                write!(result, "\\u{code:04x}")
            } else {
                write!(result, "\\U{code:08x}")
            }
            .expect("string write should be infallible");
        }
    }
    result
}

/// Formats a float as a percentage (format type `%`).
///
/// Multiplies the value by 100 and appends a `%` sign. Uses fixed-point notation
/// with `precision` decimal places (default 6). For example, `0.1234` becomes `12.340000%`.
pub fn format_float_percent(f: f64, spec: &ParsedFormatSpec) -> String {
    let precision = spec.precision.unwrap_or(6);
    let percent_val = f * 100.0;
    let is_negative = percent_val.is_sign_negative() && !percent_val.is_nan();
    let abs_val = percent_val.abs();

    let abs_str = format!("{abs_val:.precision$}%");

    let sign = if is_negative {
        "-"
    } else {
        match spec.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    };

    let value = format!("{sign}{abs_str}");
    let align = spec.align.unwrap_or('>');
    pad_string(&value, spec.width, align, spec.fill)
}

// ============================================================================
// Helper functions
// ============================================================================

/// Pads a string to a given width with alignment.
///
/// Alignment options:
/// - '<': left-align (pad on right)
/// - '>': right-align (pad on left)
/// - '^': center (pad both sides)
fn pad_string(value: &str, width: usize, align: char, fill: char) -> String {
    let value_len = value.chars().count();
    if width <= value_len {
        return value.to_owned();
    }

    let padding = width - value_len;

    match align {
        '<' => {
            let mut s = value.to_owned();
            for _ in 0..padding {
                s.push(fill);
            }
            s
        }
        '>' => {
            let mut s = String::new();
            for _ in 0..padding {
                s.push(fill);
            }
            s.push_str(value);
            s
        }
        '^' => {
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;
            let mut s = String::new();
            for _ in 0..left_pad {
                s.push(fill);
            }
            s.push_str(value);
            for _ in 0..right_pad {
                s.push(fill);
            }
            s
        }
        _ => value.to_owned(),
    }
}

/// Strips trailing zeros from a decimal float string.
///
/// Used by the `:g` format to remove insignificant trailing zeros.
/// Also removes the decimal point if all fractional digits are stripped.
/// Has no effect if the string doesn't contain a decimal point.
fn strip_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_owned();
    }
    let trimmed = s.trim_end_matches('0');
    if let Some(stripped) = trimmed.strip_suffix('.') {
        stripped.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Strips trailing zeros from a float in exponential notation.
///
/// Splits the string at `e` or `E`, strips zeros from the mantissa part,
/// then recombines with the exponent. Also normalizes the exponent format
/// to Python's convention (sign and at least 2 digits).
fn strip_trailing_zeros_exp(s: &str) -> String {
    if let Some(e_pos) = s.find(['e', 'E']) {
        let (mantissa, exp_part) = s.split_at(e_pos);
        let trimmed_mantissa = strip_trailing_zeros(mantissa);
        let fixed_exp = fix_exp_format(exp_part);
        format!("{trimmed_mantissa}{fixed_exp}")
    } else {
        strip_trailing_zeros(s)
    }
}

/// Converts Rust's exponential format to Python's format.
///
/// Rust produces "e3" or "e-3" but Python expects "e+03" or "e-03".
/// This function ensures the exponent has:
/// 1. A sign character ('+' or '-')
/// 2. At least 2 digits
fn fix_exp_format(s: &str) -> String {
    // Find the 'e' or 'E' marker
    let Some(e_pos) = s.find(['e', 'E']) else {
        return s.to_owned();
    };

    let (before_e, e_and_rest) = s.split_at(e_pos);
    let e_char = e_and_rest.chars().next().unwrap();
    let exp_part = &e_and_rest[1..];

    // Parse the exponent sign and value
    let (sign, digits) = if let Some(stripped) = exp_part.strip_prefix('-') {
        ('-', stripped)
    } else if let Some(stripped) = exp_part.strip_prefix('+') {
        ('+', stripped)
    } else {
        ('+', exp_part)
    };

    // Ensure at least 2 digits
    let padded_digits = if digits.len() < 2 {
        format!("{digits:0>2}")
    } else {
        digits.to_owned()
    };

    format!("{before_e}{e_char}{sign}{padded_digits}")
}

/// Truncates f64 to i32 with clamping for out-of-range values.
///
/// Used for exponent calculations where the result should fit in i32.
fn f64_to_i32_trunc(value: f64) -> i32 {
    if value >= f64::from(i32::MAX) {
        i32::MAX
    } else if value <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        // SAFETY for clippy: value is guaranteed to be in (i32::MIN, i32::MAX)
        // after the bounds checks above, so truncation cannot overflow
        #[expect(clippy::cast_possible_truncation, reason = "bounds checked above")]
        let result = value as i32;
        result
    }
}
