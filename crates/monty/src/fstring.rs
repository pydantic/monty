//! F-string type definitions and formatting functions.
//!
//! This module contains the AST types for f-strings (formatted string literals)
//! and the runtime formatting functions used by the bytecode VM.
//!
//! F-strings can contain literal text and interpolated expressions with optional
//! conversion flags (`!s`, `!r`, `!a`) and format specifications.

use std::{fmt, fmt::Write, iter, iter::Peekable, str::FromStr};

use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunError, SimpleException},
    expressions::ExprLoc,
    intern::StringId,
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
/// - `f"{value:>10}"` has `FormatSpec::Static(encoded)` where `encoded` is the
///   bit-packed form produced by [`encode_format_spec`]
/// - `f"{value:{width}}"` has `FormatSpec::Dynamic` with the `width` variable
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FormatSpec {
    /// Pre-parsed and pre-encoded static format spec (e.g., ">10s", ".2f").
    ///
    /// Parsing and encoding both happen at parse time so the compiler can
    /// stamp this value straight into the bytecode constant pool as a
    /// `Value::Int` — no further work, no fallible conversions. The VM
    /// recognises it via the `FORMAT_VALUE_STATIC_SPEC` flag on the
    /// emitted `FormatValue` opcode (not by inspecting the `Value`
    /// variant) and decodes in-place.
    ///
    /// Specs whose width or precision exceed the encoding's capacity (see
    /// [`MAX_ENCODED_WIDTH`]/[`MAX_ENCODED_PRECISION`]) are emitted as
    /// `Dynamic` instead so the VM can re-parse them at runtime.
    Static(i64),
    /// Dynamic format spec with nested f-string parts
    ///
    /// These must be evaluated at runtime, then parsed into a `ParsedFormatSpec`.
    Dynamic(Vec<FStringPart>),
}

/// Alignment specifier for the format mini-language.
///
/// `Align::SignAware` (`=`) is only valid on numeric formats; the others
/// apply to any value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Align {
    /// `<` — left-align the value, pad on the right.
    Left,
    /// `>` — right-align the value, pad on the left.
    Right,
    /// `^` — center the value, pad on both sides.
    Center,
    /// `=` — sign-aware: pad between sign and digits (numbers only).
    SignAware,
}

/// Sign handling specifier for numeric formats.
///
/// `Sign::Minus` is Python's default (sign shown only for negative values),
/// and is also what an absent specifier means at runtime — so the parser
/// stores it as `Option<Sign>::None` to keep "no spec given" distinct from
/// the explicit `-` form for round-tripping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sign {
    /// `+` — always emit a sign (`+` for positives, `-` for negatives).
    Plus,
    /// `-` — sign shown only for negatives (Python default).
    Minus,
    /// ` ` (space) — space for positives, `-` for negatives.
    Space,
}

/// Type character for the format mini-language.
///
/// Selects between formatting families (integer base, float notation,
/// string). Values that don't appear here (e.g. `i`, `r`) are rejected at
/// parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TypeChar {
    /// `b` — binary integer.
    B,
    /// `c` — integer codepoint as a single character.
    C,
    /// `d` — decimal integer.
    D,
    /// `e` — lowercase exponential float.
    E,
    /// `E` — uppercase exponential float.
    EUpper,
    /// `f` — fixed-point float.
    F,
    /// `F` — fixed-point float (uppercase NaN/inf).
    FUpper,
    /// `g` — general-format float (chooses between fixed and exponential).
    G,
    /// `G` — general-format float (uppercase exponent).
    GUpper,
    /// `n` — locale-aware integer (currently unimplemented; rejected at runtime).
    N,
    /// `o` — octal integer.
    O,
    /// `s` — string.
    S,
    /// `x` — lowercase hex integer.
    X,
    /// `X` — uppercase hex integer.
    XUpper,
    /// `%` — percentage float (multiplies by 100 and appends `%`).
    Percent,
}

impl Align {
    /// Parses a format-spec alignment character into the corresponding variant.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '<' => Some(Self::Left),
            '>' => Some(Self::Right),
            '^' => Some(Self::Center),
            '=' => Some(Self::SignAware),
            _ => None,
        }
    }
}

impl Sign {
    /// Parses a format-spec sign character into the corresponding variant.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '+' => Some(Self::Plus),
            '-' => Some(Self::Minus),
            ' ' => Some(Self::Space),
            _ => None,
        }
    }
}

impl TypeChar {
    /// Parses a format-spec type character into the corresponding variant.
    ///
    /// Returns `None` for characters that aren't part of the format
    /// mini-language type set — used by [`ParsedFormatSpec::from_str`] to
    /// decide whether the trailing character is a type spec or an error.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'b' => Some(Self::B),
            'c' => Some(Self::C),
            'd' => Some(Self::D),
            'e' => Some(Self::E),
            'E' => Some(Self::EUpper),
            'f' => Some(Self::F),
            'F' => Some(Self::FUpper),
            'g' => Some(Self::G),
            'G' => Some(Self::GUpper),
            'n' => Some(Self::N),
            'o' => Some(Self::O),
            's' => Some(Self::S),
            'x' => Some(Self::X),
            'X' => Some(Self::XUpper),
            '%' => Some(Self::Percent),
            _ => None,
        }
    }

    /// Renders the type character back into its source form. Used for
    /// error messages like "Unknown format code 'X' for object of type 'T'".
    pub fn as_char(self) -> char {
        match self {
            Self::B => 'b',
            Self::C => 'c',
            Self::D => 'd',
            Self::E => 'e',
            Self::EUpper => 'E',
            Self::F => 'f',
            Self::FUpper => 'F',
            Self::G => 'g',
            Self::GUpper => 'G',
            Self::N => 'n',
            Self::O => 'o',
            Self::S => 's',
            Self::X => 'x',
            Self::XUpper => 'X',
            Self::Percent => '%',
        }
    }
}

/// Parsed format specification following Python's format mini-language.
///
/// Format: `[[fill]align][sign][z][#][0][width][grouping_option][.precision][type]`
///
/// This struct is parsed at parse time for static format specs, avoiding runtime
/// string parsing. For dynamic format specs, parsing happens after evaluation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ParsedFormatSpec {
    /// Fill character for padding (default: space).
    pub fill: char,
    /// Alignment, or `None` if not specified.
    pub align: Option<Align>,
    /// Sign handling, or `None` if not specified (treated as [`Sign::Minus`]).
    pub sign: Option<Sign>,
    /// Whether to zero-pad numbers.
    pub zero_pad: bool,
    /// Minimum field width.
    pub width: usize,
    /// Precision for floats or max width for strings.
    pub precision: Option<usize>,
    /// Type character, or `None` if not specified (defaults are type-dependent).
    pub type_char: Option<TypeChar>,
}

/// Reason a [`ParsedFormatSpec`] couldn't be built from its source text.
///
/// Lets callers distinguish CPython-style invalid specs ([`Self::Malformed`])
/// from specs that are syntactically valid in Python but use features Monty
/// hasn't implemented yet ([`Self::UnsupportedFlag`]), and from specs whose
/// width or precision exceeds [`usize`] ([`Self::NumberOverflow`]). The
/// `Display` impl on [`ParseFormatSpecError`] turns each variant into a
/// human-readable message; runtime callers append `" for object of type
/// 'T'"` to mirror CPython's error style.
#[derive(Debug, Clone)]
pub enum ParseFormatSpecReason {
    /// Spec doesn't match the format mini-language grammar — what CPython
    /// itself raises `ValueError: Invalid format specifier` for.
    Malformed,
    /// Spec uses a flag character that's part of Python's mini-language
    /// (`#` alternate form, `,` or `_` thousands separator) but isn't yet
    /// implemented. Carries the flag char so callers can report it.
    UnsupportedFlag(char),
    /// A width or precision decimal integer overflows [`usize`] (e.g.
    /// 22 nines in a row). Without this we'd silently truncate to 0 — see
    /// [`consume_decimal_usize`].
    NumberOverflow,
}

/// Error returned by [`ParsedFormatSpec::from_str`].
///
/// Holds the original spec text plus a [`ParseFormatSpecReason`] so the
/// runtime and compile-time error wrappers can choose between
/// CPython-matching messages and Monty-specific ones.
#[derive(Debug, Clone)]
pub struct ParseFormatSpecError {
    /// The full spec text that failed to parse.
    pub spec: String,
    /// Why parsing failed.
    pub reason: ParseFormatSpecReason,
}

impl ParseFormatSpecError {
    fn new(spec: &str, reason: ParseFormatSpecReason) -> Self {
        Self {
            spec: spec.to_owned(),
            reason,
        }
    }
}

impl fmt::Display for ParseFormatSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid format specifier '{}'", self.spec)?;
        match &self.reason {
            ParseFormatSpecReason::Malformed => Ok(()),
            ParseFormatSpecReason::UnsupportedFlag(c) => write!(
                f,
                ": '{c}' ({}) is not yet supported in Monty",
                unsupported_flag_name(*c),
            ),
            ParseFormatSpecReason::NumberOverflow => {
                write!(f, ": width or precision overflows usize")
            }
        }
    }
}

/// Maps an unsupported flag character to a short human-readable name for
/// error messages — keeps the [`fmt::Display`] impl on
/// [`ParseFormatSpecError`] terse and lets us extend the list in one place.
fn unsupported_flag_name(c: char) -> &'static str {
    match c {
        '#' => "alternate form",
        ',' => "comma thousands separator",
        '_' => "underscore thousands separator",
        _ => "format flag",
    }
}

impl FromStr for ParsedFormatSpec {
    type Err = ParseFormatSpecError;

    /// Parses a format specification string into its components.
    ///
    /// Returns a [`ParseFormatSpecError`] for malformed specs, specs that
    /// rely on flags Monty doesn't implement yet (`#`, `,`, `_`), or specs
    /// whose width/precision overflows [`usize`].
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
        // If the second char is an align marker, the first is the fill; otherwise
        // the first char (if any) may itself be the align.
        if let Some(align) = spec.chars().nth(1).and_then(Align::from_char) {
            result.fill = chars.next().unwrap_or(' ');
            chars.next();
            result.align = Some(align);
        } else {
            result.align = chars.next_if_map(|c| Align::from_char(c).ok_or(c));
        }

        result.sign = chars.next_if_map(|c| Sign::from_char(c).ok_or(c));

        // `#` (alternate form): part of Python's format mini-language but not
        // yet implemented in Monty — reject loudly rather than silently
        // dropping the flag.
        if chars.next_if_eq(&'#').is_some() {
            return Err(ParseFormatSpecError::new(
                spec,
                ParseFormatSpecReason::UnsupportedFlag('#'),
            ));
        }

        // Parse zero-padding flag (must come before width)
        if chars.next_if_eq(&'0').is_some() {
            result.zero_pad = true;
        }

        // Parse width
        result.width = consume_decimal_usize(&mut chars)
            .map_err(|()| ParseFormatSpecError::new(spec, ParseFormatSpecReason::NumberOverflow))?
            .unwrap_or(0);

        // Grouping option (`,` or `_` thousands separator): both are valid
        // Python flags that Monty doesn't implement yet — same rejection
        // policy as `#`.
        if let Some(g) = chars.next_if(|c| matches!(c, ',' | '_')) {
            return Err(ParseFormatSpecError::new(
                spec,
                ParseFormatSpecReason::UnsupportedFlag(g),
            ));
        }

        // Parse precision: .N
        if chars.next_if_eq(&'.').is_some() {
            result.precision = consume_decimal_usize(&mut chars)
                .map_err(|()| ParseFormatSpecError::new(spec, ParseFormatSpecReason::NumberOverflow))?;
        }

        result.type_char = chars.next_if_map(|c| TypeChar::from_char(c).ok_or(c));

        // Error if there are any unconsumed characters
        if chars.peek().is_some() {
            return Err(ParseFormatSpecError::new(spec, ParseFormatSpecReason::Malformed));
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

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    vm: &mut VM<'_, impl ResourceTracker>,
) -> Result<String, RunError> {
    let value_type = value.py_type(vm);

    match (value, spec.type_char) {
        // Integer formatting
        (Value::Int(n), None | Some(TypeChar::D)) => Ok(format_int(*n, spec)),
        (Value::Int(n), Some(TypeChar::B)) => Ok(format_int_base(*n, 2, spec)?),
        (Value::Int(n), Some(TypeChar::O)) => Ok(format_int_base(*n, 8, spec)?),
        (Value::Int(n), Some(TypeChar::X)) => Ok(format_int_base(*n, 16, spec)?),
        (Value::Int(n), Some(TypeChar::XUpper)) => Ok(format_int_base(*n, 16, spec)?.to_uppercase()),
        (Value::Int(n), Some(TypeChar::C)) => Ok(format_char(*n, spec)?),

        // Float formatting
        (Value::Float(f), None | Some(TypeChar::G | TypeChar::GUpper)) => Ok(format_float_g(*f, spec)),
        (Value::Float(f), Some(TypeChar::F | TypeChar::FUpper)) => Ok(format_float_f(*f, spec)),
        (Value::Float(f), Some(TypeChar::E)) => Ok(format_float_e(*f, spec, false)),
        (Value::Float(f), Some(TypeChar::EUpper)) => Ok(format_float_e(*f, spec, true)),
        (Value::Float(f), Some(TypeChar::Percent)) => Ok(format_float_percent(*f, spec)),

        // Int to float formatting (Python allows this)
        (Value::Int(n), Some(TypeChar::F | TypeChar::FUpper)) => Ok(format_float_f(*n as f64, spec)),
        (Value::Int(n), Some(TypeChar::E)) => Ok(format_float_e(*n as f64, spec, false)),
        (Value::Int(n), Some(TypeChar::EUpper)) => Ok(format_float_e(*n as f64, spec, true)),
        (Value::Int(n), Some(TypeChar::G | TypeChar::GUpper)) => Ok(format_float_g(*n as f64, spec)),
        (Value::Int(n), Some(TypeChar::Percent)) => Ok(format_float_percent(*n as f64, spec)),

        // String formatting (including InternString and heap strings)
        (_, None | Some(TypeChar::S)) if value_type == Type::Str => {
            let s = value.py_str(vm)?;
            Ok(format_string(&s, spec)?)
        }

        // Bool as int
        (Value::Bool(b), Some(TypeChar::D)) => Ok(format_int(i64::from(*b), spec)),

        // No type specifier: convert to string and format
        (_, None) => {
            let s = value.py_str(vm)?;
            Ok(format_string(&s, spec)?)
        }

        // Type mismatch errors
        (_, Some(c)) => Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!(
                "Unknown format code '{}' for object of type '{value_type}'",
                c.as_char()
            ),
        )
        .into()),
    }
}

/// Maximum fill codepoint that fits in the 8-bit fill field of the encoded
/// format spec. Latin-1 covers the common cases (`*`, `_`, `-`, `.`, plus
/// any single-byte char); higher codepoints (CJK, emoji, etc.) fall back to
/// a dynamic spec so the VM re-parses at runtime.
pub const MAX_ENCODED_FILL: u32 = 0xFF;

/// Maximum width that fits in the 20-bit width field of the encoded format spec.
pub const MAX_ENCODED_WIDTH: usize = (1 << 20) - 1;

/// Maximum precision that fits in the 21-bit precision field of the encoded format
/// spec. One slot (the zero value) is reserved to mean "no precision", so the
/// usable range for an explicit precision is `0..=MAX_ENCODED_PRECISION`.
pub const MAX_ENCODED_PRECISION: usize = (1 << 21) - 2;

/// Encodes a [`ParsedFormatSpec`] into an `i64` for storage in bytecode constants.
///
/// Returns `None` if any field exceeds the encoding's capacity — the caller
/// should fall back to a dynamic (string-based) format spec in that case.
///
/// Encoding layout (occupies bits 0-59; the sign bit is always 0, so the
/// result is a non-negative `i64`):
/// - bits 0-7: fill codepoint (Latin-1; max [`MAX_ENCODED_FILL`], default space=32)
/// - bits 8-10: [`Align`] (0=none, 1=Left, 2=Right, 3=Center, 4=SignAware)
/// - bits 11-12: [`Sign`] (0=none, 1=Plus, 2=Minus, 3=Space)
/// - bit 13: zero_pad
/// - bits 14-33: width (20 bits, max [`MAX_ENCODED_WIDTH`])
/// - bits 34-54: precision+1 (21 bits; 0 = no precision)
/// - bits 55-59: [`TypeChar`] (0=none, 1-15=B/C/D/E/EUpper/F/FUpper/G/GUpper/N/O/S/X/XUpper/Percent)
pub fn encode_format_spec(spec: &ParsedFormatSpec) -> Option<i64> {
    let fill_code = u32::from(spec.fill);
    if fill_code > MAX_ENCODED_FILL {
        return None;
    }
    if spec.width > MAX_ENCODED_WIDTH {
        return None;
    }
    if let Some(p) = spec.precision
        && p > MAX_ENCODED_PRECISION
    {
        return None;
    }

    let fill = i64::from(fill_code);
    let align: i64 = spec.align.map_or(0, |a| match a {
        Align::Left => 1,
        Align::Right => 2,
        Align::Center => 3,
        Align::SignAware => 4,
    });
    let sign: i64 = spec.sign.map_or(0, |s| match s {
        Sign::Plus => 1,
        Sign::Minus => 2,
        Sign::Space => 3,
    });
    let zero_pad = i64::from(spec.zero_pad);
    // `try_from` is infallible after the bounds checks above; the expects
    // document the invariant that keeps clippy's wrap-on-64-bit lint at bay.
    let width = i64::try_from(spec.width).expect("width bounds-checked by MAX_ENCODED_WIDTH");
    // Store precision as `p + 1`, reserving 0 for the "no precision" marker.
    let precision: i64 = spec.precision.map_or(0, |p| {
        i64::try_from(p).expect("precision bounds-checked by MAX_ENCODED_PRECISION") + 1
    });
    let type_char: i64 = spec.type_char.map_or(0, |c| match c {
        TypeChar::B => 1,
        TypeChar::C => 2,
        TypeChar::D => 3,
        TypeChar::E => 4,
        TypeChar::EUpper => 5,
        TypeChar::F => 6,
        TypeChar::FUpper => 7,
        TypeChar::G => 8,
        TypeChar::GUpper => 9,
        TypeChar::N => 10,
        TypeChar::O => 11,
        TypeChar::S => 12,
        TypeChar::X => 13,
        TypeChar::XUpper => 14,
        TypeChar::Percent => 15,
    });

    // Every field occupies bits 0..60, so the sign bit is never set and the
    // shifts/ORs stay within well-defined i64 territory.
    Some(fill | (align << 8) | (sign << 11) | (zero_pad << 13) | (width << 14) | (precision << 34) | (type_char << 55))
}

/// Decodes an [`i64`] back into a [`ParsedFormatSpec`].
///
/// Reverses the bit-packing done by [`encode_format_spec`]. Used by the VM
/// when executing `FormatValue` with the `FORMAT_VALUE_STATIC_SPEC` flag to
/// recover the pre-parsed spec from the constant pool entry.
pub fn decode_format_spec(encoded: i64) -> ParsedFormatSpec {
    // The valid encoding sits in bits 0..60 so `cast_unsigned` is a no-op
    // reinterpret — the sign bit is always 0 here.
    let encoded = encoded.cast_unsigned();
    let fill = (encoded & 0xFF) as u8 as char;
    let align_bits = (encoded >> 8) & 0x07;
    let sign_bits = (encoded >> 11) & 0x03;
    let zero_pad = ((encoded >> 13) & 0x01) != 0;
    let width = ((encoded >> 14) & 0xF_FFFF) as usize;
    let precision_raw = ((encoded >> 34) & 0x1F_FFFF) as usize;
    let type_bits = ((encoded >> 55) & 0x1F) as u8;

    let align = match align_bits {
        1 => Some(Align::Left),
        2 => Some(Align::Right),
        3 => Some(Align::Center),
        4 => Some(Align::SignAware),
        _ => None,
    };

    let sign = match sign_bits {
        1 => Some(Sign::Plus),
        2 => Some(Sign::Minus),
        3 => Some(Sign::Space),
        _ => None,
    };

    // Encoding stores `precision + 1`, so 0 means "no precision".
    let precision = if precision_raw == 0 {
        None
    } else {
        Some(precision_raw - 1)
    };

    let type_char = match type_bits {
        1 => Some(TypeChar::B),
        2 => Some(TypeChar::C),
        3 => Some(TypeChar::D),
        4 => Some(TypeChar::E),
        5 => Some(TypeChar::EUpper),
        6 => Some(TypeChar::F),
        7 => Some(TypeChar::FUpper),
        8 => Some(TypeChar::G),
        9 => Some(TypeChar::GUpper),
        10 => Some(TypeChar::N),
        11 => Some(TypeChar::O),
        12 => Some(TypeChar::S),
        13 => Some(TypeChar::X),
        14 => Some(TypeChar::XUpper),
        15 => Some(TypeChar::Percent),
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
    if spec.align == Some(Align::SignAware) {
        return Err(FormatError::InvalidAlignment(
            "'=' alignment not allowed in string format specifier".to_owned(),
        ));
    }

    // Default alignment for strings is left
    let align = spec.align.unwrap_or(Align::Left);
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
    // Use unsigned_abs() to avoid overflow panic on i64::MIN
    let abs_str = n.unsigned_abs().to_string();
    let sign = if is_negative {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    };
    pad_signed_numeric(sign, &abs_str, spec)
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

    let sign = if is_negative {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    };
    Ok(pad_signed_numeric(sign, &abs_str, spec))
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
    // `=` (SignAware) on `:c` is accepted by CPython but degenerates to right-align
    // because there's no sign component to pad between. Map it now so `pad_string`
    // (which treats SignAware as a no-op) does the right thing.
    let align = match spec.align.unwrap_or(Align::Left) {
        Align::SignAware => Align::Right,
        other => other,
    };
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
    let abs_str = fmt_float_fixed(abs_val, precision);
    let sign = if is_negative {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    };
    pad_signed_numeric(sign, &abs_str, spec)
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
    let abs_str = fmt_float_exp(abs_val, precision, uppercase);
    // Fix exponent format to match Python (e+03 not e3)
    let abs_str = fix_exp_format(&abs_str);
    let sign = if is_negative {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    };
    pad_signed_numeric(sign, &abs_str, spec)
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
        // Cap Rust precision; trailing zeros are stripped so padding isn't needed.
        let formatted = fmt_float_exp(abs_val, exp_prec.min(MAX_FMT_PRECISION_EXP), false);
        // Python strips trailing zeros from the mantissa
        strip_trailing_zeros_exp(&formatted)
    } else {
        // Use fixed notation - result is non-negative due to .max(0)
        let sig_digits_i32 = (prec_i32 - exp - 1).max(0);
        let sig_digits = usize::try_from(sig_digits_i32).expect("sig_digits guaranteed non-negative");
        // Cap Rust precision; trailing zeros are stripped so padding isn't needed.
        let cap = sig_digits.min(MAX_FMT_PRECISION);
        let formatted = format!("{abs_val:.cap$}");
        strip_trailing_zeros(&formatted)
    };

    let sign = if is_negative {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    };
    pad_signed_numeric(sign, &abs_str, spec)
}

/// Applies ASCII conversion to a string (escapes non-ASCII characters).
///
/// Used for the `!a` conversion flag in f-strings. Takes a string (typically a repr)
/// and escapes all non-ASCII characters using `\xNN`, `\uNNNN`, or `\UNNNNNNNN`.
pub fn ascii_escape(s: &str) -> String {
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

    let abs_str = format!("{}%", fmt_float_fixed(abs_val, precision));
    let sign = if is_negative {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    };
    pad_signed_numeric(sign, &abs_str, spec)
}

// ============================================================================
// Helper functions
// ============================================================================

/// Renders the sign prefix that precedes a non-negative number's digits.
///
/// Centralizes the `+`/space/empty decision that every numeric formatter
/// (`format_int`, `format_float_*`, etc.) needs when the value isn't
/// negative. Returns `""` for `None` and for `Some(Sign::Minus)` since both
/// mean "no leading mark on positives".
fn positive_sign_prefix(sign: Option<Sign>) -> &'static str {
    match sign {
        Some(Sign::Plus) => "+",
        Some(Sign::Space) => " ",
        None | Some(Sign::Minus) => "",
    }
}

/// Pads `sign + abs_str` to `spec.width` with the right alignment semantics
/// for a signed numeric value.
///
/// Numeric formatters all share three padding modes:
/// - `zero_pad` (`0` flag): insert `'0'` between the sign and the digits.
/// - `Align::SignAware` (`=`): insert `spec.fill` between the sign and the
///   digits.
/// - Anything else: glue `sign` + `abs_str` together and let [`pad_string`]
///   place fill outside the value.
///
/// Without this helper each formatter that wants sign-aware behaviour had
/// to inline the same conditional, and the ones that *didn't* (the
/// non-decimal integer bases, all the float formats except `:f`) silently
/// dropped width for `=` — see `parse_errors.rs::format_spec_…` tests.
/// Default alignment is right because all callers are numeric formats;
/// `format_char` (default left, no sign) needs separate handling.
fn pad_signed_numeric(sign: &str, abs_str: &str, spec: &ParsedFormatSpec) -> String {
    let align = spec.align.unwrap_or(Align::Right);
    if spec.zero_pad || align == Align::SignAware {
        let fill = if spec.zero_pad { '0' } else { spec.fill };
        let total_len = sign.len() + abs_str.len();
        if spec.width > total_len {
            let padding = spec.width - total_len;
            let pad_str: String = iter::repeat_n(fill, padding).collect();
            format!("{sign}{pad_str}{abs_str}")
        } else {
            format!("{sign}{abs_str}")
        }
    } else {
        let value = format!("{sign}{abs_str}");
        pad_string(&value, spec.width, align, spec.fill)
    }
}

/// Consumes a run of ASCII digits and folds them into a decimal [`usize`].
///
/// Returns `Ok(None)` when no digit is present, `Ok(Some(n))` for a parsed
/// number, and `Err(())` if accumulating would overflow [`usize`]. Used for
/// the width and precision fields of the format mini-language — both are
/// decimal integers terminated by the next non-digit.
///
/// Folding digits inline avoids the intermediate `String` that
/// `.parse::<usize>()` would need, and surfaces overflow so the caller
/// can bail with a parse error rather than silently clamping to 0.
fn consume_decimal_usize(chars: &mut Peekable<impl Iterator<Item = char>>) -> Result<Option<usize>, ()> {
    let mut value: Option<usize> = None;
    while let Some(c) = chars.next_if(char::is_ascii_digit) {
        let digit = c.to_digit(10).expect("char::is_ascii_digit guarantees a 0-9 digit") as usize;
        let next = value
            .unwrap_or(0)
            .checked_mul(10)
            .and_then(|n| n.checked_add(digit))
            .ok_or(())?;
        value = Some(next);
    }
    Ok(value)
}

/// Maximum precision Rust's `format!` accepts for fixed-point float formatting
/// before it panics with "Formatting argument out of range" (i.e. `u16::MAX`).
///
/// Python allows arbitrary precision in f-strings (e.g. `.{10**6}f`), so
/// we cap at this limit and pad manually with zeros beyond it.
const MAX_FMT_PRECISION: usize = u16::MAX as usize;

/// Maximum precision Rust's `format!` accepts for exponential (`e`/`E`) float
/// formatting. One less than `MAX_FMT_PRECISION` because Rust's internal
/// `to_exact_exp_str` uses `ndigits = precision + 1`, which would overflow
/// `u16::MAX` and hit an `ndigits > 0` assertion at exactly `u16::MAX`.
const MAX_FMT_PRECISION_EXP: usize = (u16::MAX as usize) - 1;

/// Formats a float in fixed-point notation at an arbitrary precision.
///
/// Rust's `format!` panics if precision exceeds `u16::MAX`. For non-finite
/// values (NaN/inf) precision is ignored entirely, matching Rust's behavior.
/// For finite values beyond the native limit we format at `MAX_FMT_PRECISION`
/// and append trailing zeros — f64 precision bottoms out long before this, so
/// every additional digit Python would emit is a zero anyway.
fn fmt_float_fixed(abs_val: f64, precision: usize) -> String {
    if precision <= MAX_FMT_PRECISION || !abs_val.is_finite() {
        return format!("{abs_val:.precision$}");
    }
    let mut s = format!("{abs_val:.MAX_FMT_PRECISION$}");
    s.extend(iter::repeat_n('0', precision - MAX_FMT_PRECISION));
    s
}

/// Formats a float in exponential notation at an arbitrary precision.
///
/// Same precision-capping strategy as `fmt_float_fixed`, but trailing zeros
/// are injected into the mantissa (before the exponent marker) rather than
/// appended to the end.
fn fmt_float_exp(abs_val: f64, precision: usize, uppercase: bool) -> String {
    if precision <= MAX_FMT_PRECISION_EXP || !abs_val.is_finite() {
        return if uppercase {
            format!("{abs_val:.precision$E}")
        } else {
            format!("{abs_val:.precision$e}")
        };
    }
    let base = if uppercase {
        format!("{abs_val:.MAX_FMT_PRECISION_EXP$E}")
    } else {
        format!("{abs_val:.MAX_FMT_PRECISION_EXP$e}")
    };
    let extra = precision - MAX_FMT_PRECISION_EXP;
    // Inject padding zeros immediately before the exponent marker.
    if let Some(e_pos) = base.find(['e', 'E']) {
        let (mantissa, exp_part) = base.split_at(e_pos);
        let zeros: String = iter::repeat_n('0', extra).collect();
        format!("{mantissa}{zeros}{exp_part}")
    } else {
        base
    }
}

/// Pads a string to a given width with alignment.
///
/// `Align::SignAware` must not reach this function — numeric formatters
/// handle `=` via [`pad_signed_numeric`] (which inserts fill between sign
/// and digits before any call to `pad_string`), and [`format_char`] maps
/// `=` to right-align since chars have no sign. Routing a SignAware value
/// here would silently drop width, which `debug_assert!` catches in test
/// builds; release builds degrade to no-op padding as a safety net.
fn pad_string(value: &str, width: usize, align: Align, fill: char) -> String {
    debug_assert!(
        align != Align::SignAware,
        "pad_string received Align::SignAware; callers must handle `=` themselves \
         (numeric formatters via pad_signed_numeric, format_char by mapping to Right)"
    );
    let value_len = value.chars().count();
    if width <= value_len {
        return value.to_owned();
    }

    let padding = width - value_len;

    match align {
        Align::Left => {
            let mut s = value.to_owned();
            for _ in 0..padding {
                s.push(fill);
            }
            s
        }
        Align::Right => {
            let mut s = String::new();
            for _ in 0..padding {
                s.push(fill);
            }
            s.push_str(value);
            s
        }
        Align::Center => {
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
        Align::SignAware => value.to_owned(),
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
