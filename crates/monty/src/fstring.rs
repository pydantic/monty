//! F-string type definitions and formatting functions.
//!
//! This module contains the AST types for f-strings (formatted string literals)
//! and the runtime formatting functions used by the bytecode VM.
//!
//! F-strings can contain literal text and interpolated expressions with optional
//! conversion flags (`!s`, `!r`, `!a`) and format specifications.

use std::{fmt, fmt::Write, iter, iter::Peekable, str, str::FromStr};

use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunError, SimpleException},
    expressions::ExprLoc,
    heap::HeapData,
    intern::StringId,
    resource::{ResourceTracker, check_repeat_size},
    types::{LongInt, PyTrait, Type, long_int::check_bits_str_digits_limit},
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

/// Thousands-grouping specifier for numeric formats (`,` and `_`).
///
/// Selects which separator is inserted between digit groups. The *group
/// size* is not stored here — it's 3 for decimal and float presentations
/// but 4 for the binary/octal/hex integer presentations (which only accept
/// `_`), so it's derived from the type char at format time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Grouping {
    /// `,` — comma separator. Only valid for decimal/float presentations.
    Comma,
    /// `_` — underscore separator. Valid for decimal/float (groups of 3) and
    /// for binary/octal/hex (groups of 4).
    Underscore,
}

impl Grouping {
    /// The separator character inserted between digit groups.
    fn separator(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Underscore => '_',
        }
    }
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
    /// Alternate form (`#`): adds the `0b`/`0o`/`0x` base prefix to
    /// binary/octal/hex integers and forces a decimal point on floats.
    /// Validity against the presentation type is checked at format time.
    pub alternate: bool,
    /// Negative-zero coercion flag (`z`, Python 3.11+): a negative value that
    /// rounds to zero at the chosen precision is emitted as positive zero
    /// (`f"{-0.001:z.1f}"` → `0.0`). Valid only for floating-point
    /// presentations (checked at format time). Like [`Self::frac_grouping`] it
    /// has no spare encoding bit, so [`encode_format_spec`] forces specs that
    /// use it onto the dynamic-spec path.
    pub z: bool,
    /// Whether to zero-pad numbers.
    pub zero_pad: bool,
    /// Minimum field width.
    pub width: usize,
    /// Thousands-grouping separator (`,` or `_`) for the *integer* digits, or
    /// `None` if not specified. Validity against the presentation type is
    /// checked at format time.
    pub grouping: Option<Grouping>,
    /// Grouping separator (`,` or `_`) for the *fractional* digits — the
    /// Python 3.14 `[.precision][grouping]` extension (`f"{x:.6_f}"`). Groups
    /// every 3 digits *from the left* of the fraction. `None` if absent. Specs
    /// that set this can't use the compact bit encoding (no spare bits), so
    /// [`encode_format_spec`] forces them onto the dynamic-spec path.
    pub frac_grouping: Option<Grouping>,
    /// Precision for floats or max width for strings.
    pub precision: Option<usize>,
    /// Type character, or `None` if not specified (defaults are type-dependent).
    pub type_char: Option<TypeChar>,
}

/// Reason a [`ParsedFormatSpec`] couldn't be built from its source text.
///
/// Lets callers distinguish CPython-style invalid specs ([`Self::Malformed`])
/// from specs whose width or precision exceeds [`usize`]
/// ([`Self::NumberOverflow`]). The `Display` impl on [`ParseFormatSpecError`]
/// turns each variant into a human-readable message; runtime callers append
/// `" for object of type 'T'"` to mirror CPython's error style.
#[derive(Debug, Clone)]
pub enum ParseFormatSpecReason {
    /// Spec doesn't match the format mini-language grammar — what CPython
    /// itself raises `ValueError: Invalid format specifier` for. Reserved for
    /// genuinely malformed specs (two or more trailing characters after the
    /// type field, e.g. `kk`/`.2fz`); a *single* unrecognised trailing char is
    /// [`Self::UnknownFormatCode`] instead, matching CPython's split.
    Malformed,
    /// A width or precision decimal integer overflows [`usize`] (e.g.
    /// 22 nines in a row). Without this we'd silently truncate to 0 — see
    /// [`consume_decimal_usize`].
    NumberOverflow,
    /// A `.` with no following precision digits *and* no fractional grouping
    /// option (`.f`, `.`, `.d`) — CPython's `ValueError: Format specifier
    /// missing precision`. A `.` followed by a grouping char (`._f`, `.,`) is
    /// valid and not this error.
    MissingPrecision,
    /// The single trailing char isn't a recognised presentation type
    /// (`5k`, `k`). Carries the offending char; the runtime caller appends
    /// `" for object of type 'T'"` to produce CPython's `Unknown format code
    /// 'k' for object of type 'int'`.
    UnknownFormatCode(char),
    /// An integer grouping option (`,`/`_`) sits next to something it can't
    /// coexist with — the other grouping char (`,_`) or an unrecognised
    /// trailing char (`,k`). Holds CPython's exact, self-contained wording
    /// (`Cannot specify ',' with 'k'.`), which carries no type suffix.
    GroupingConflict(String),
}

impl ParseFormatSpecReason {
    /// Builds the [`Self::GroupingConflict`] wording for an integer grouping
    /// `g` immediately followed by the unrecognised trailing char `c`,
    /// reproducing CPython's two distinct messages: `Cannot specify both ','
    /// and '_'.` when `c` is the *other* grouping char, otherwise `Cannot
    /// specify '<g>' with '<c>'.`.
    fn grouping_conflict(g: Grouping, c: char) -> Self {
        let msg = if (c == ',' || c == '_') && c != g.separator() {
            "Cannot specify both ',' and '_'.".to_owned()
        } else {
            format!("Cannot specify '{}' with '{c}'.", g.separator())
        };
        Self::GroupingConflict(msg)
    }
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

    /// Whether the runtime should append `" for object of type 'T'"` to the
    /// `Display` message, matching CPython's wording.
    ///
    /// CPython suffixes the type for `Invalid format specifier` and `Unknown
    /// format code` but not for `Format specifier missing precision` or the
    /// `Cannot specify …` grouping conflicts, which are self-contained.
    pub fn needs_type_suffix(&self) -> bool {
        matches!(
            self.reason,
            ParseFormatSpecReason::Malformed
                | ParseFormatSpecReason::NumberOverflow
                | ParseFormatSpecReason::UnknownFormatCode(_)
        )
    }

    /// Whether this error should be deferred to the runtime (dynamic-spec) path
    /// rather than raised as a compile-time `SyntaxError`.
    ///
    /// CPython raises these as *runtime* `ValueError`s whose exact wording is
    /// value-type-dependent (`Unknown format code`) or otherwise only resolvable
    /// at format time, so the compiler emits the literal spec for the VM to
    /// re-parse and raise the matching error. Genuinely-malformed specs and
    /// `usize` overflow stay compile-time errors (the latter is a deliberate,
    /// documented divergence).
    pub fn defer_to_runtime(&self) -> bool {
        matches!(
            self.reason,
            ParseFormatSpecReason::MissingPrecision
                | ParseFormatSpecReason::UnknownFormatCode(_)
                | ParseFormatSpecReason::GroupingConflict(_)
        )
    }
}

impl fmt::Display for ParseFormatSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reason {
            ParseFormatSpecReason::Malformed => write!(f, "Invalid format specifier '{}'", self.spec),
            ParseFormatSpecReason::NumberOverflow => {
                write!(
                    f,
                    "Invalid format specifier '{}': width or precision overflows usize",
                    self.spec
                )
            }
            ParseFormatSpecReason::MissingPrecision => f.write_str("Format specifier missing precision"),
            ParseFormatSpecReason::UnknownFormatCode(c) => write!(f, "Unknown format code '{c}'"),
            ParseFormatSpecReason::GroupingConflict(msg) => f.write_str(msg),
        }
    }
}

impl FromStr for ParsedFormatSpec {
    type Err = ParseFormatSpecError;

    /// Parses a format specification string into its components.
    ///
    /// Returns a [`ParseFormatSpecError`] for malformed specs, specs that
    /// rely on flags Monty doesn't implement yet (`#`), or specs whose
    /// width/precision overflows [`usize`].
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
        let mut fill_specified = false;
        if let Some(align) = spec.chars().nth(1).and_then(Align::from_char) {
            result.fill = chars.next().unwrap_or(' ');
            fill_specified = true;
            chars.next();
            result.align = Some(align);
        } else {
            result.align = chars.next_if_map(|c| Align::from_char(c).ok_or(c));
        }

        result.sign = chars.next_if_map(|c| Sign::from_char(c).ok_or(c));

        // `z` (negative-zero coercion) sits between the sign and `#` in the
        // grammar. A `z` in *fill* position (`z>8`) was already consumed by the
        // fill/align step above, so reaching here means the flag. Whether it's
        // legal for the presentation type is checked at format time.
        result.z = chars.next_if_eq(&'z').is_some();

        // `#` (alternate form). Whether it's legal for the chosen presentation
        // type is decided at format time (the value type isn't known here for
        // type-less specs), so it's validated in `format_with_spec`.
        result.alternate = chars.next_if_eq(&'#').is_some();

        // Parse the zero-padding flag (must come before width). Per CPython, a
        // leading `0` sets the fill to `'0'` (unless a fill char was already
        // given) and implies sign-aware (`=`) padding *only when no explicit
        // alignment was specified*. With an explicit align (`<05`, `^05`,
        // `*<05`) the `0` is merely a fill character, so `zero_pad`/`=` stay
        // off and the chosen alignment is honoured.
        if chars.next_if_eq(&'0').is_some() {
            if !fill_specified {
                result.fill = '0';
            }
            if result.align.is_none() {
                result.zero_pad = true;
            }
        }

        // Parse width
        result.width = consume_decimal_usize(&mut chars)
            .map_err(|()| ParseFormatSpecError::new(spec, ParseFormatSpecReason::NumberOverflow))?
            .unwrap_or(0);

        // Grouping option (`,` or `_` thousands separator). Whether it's
        // legal for the chosen presentation type can't be decided here (the
        // value type isn't known until format time for type-less specs), so
        // it's validated in `format_with_spec`.
        result.grouping = chars.next_if(|c| matches!(c, ',' | '_')).map(|c| match c {
            ',' => Grouping::Comma,
            _ => Grouping::Underscore,
        });

        // Parse precision and an optional fractional grouping option:
        // `.[precision][grouping]` (Python 3.14). The grouping char after the
        // precision (`.6_`, or `._` with no precision) groups the fractional
        // digits; its legality for the presentation type is checked at format
        // time alongside the integer grouping.
        if chars.next_if_eq(&'.').is_some() {
            result.precision = consume_decimal_usize(&mut chars)
                .map_err(|()| ParseFormatSpecError::new(spec, ParseFormatSpecReason::NumberOverflow))?;
            result.frac_grouping = chars.next_if(|c| matches!(c, ',' | '_')).map(|c| match c {
                ',' => Grouping::Comma,
                _ => Grouping::Underscore,
            });
            // A `.` must introduce either precision digits or a fractional
            // grouping option; `.f`/`.`/`.d` are CPython's "missing precision".
            if result.precision.is_none() && result.frac_grouping.is_none() {
                return Err(ParseFormatSpecError::new(spec, ParseFormatSpecReason::MissingPrecision));
            }
        }

        // The presentation type is the spec's final character. Consume it
        // unconditionally — CPython treats *any* single trailing char as the
        // type field — so an unrecognised char becomes a specific "Unknown
        // format code"/grouping-conflict error rather than the generic
        // "Invalid format specifier". Two or more chars after the parsed fields
        // are genuinely malformed.
        let type_pos = chars.next();
        if chars.peek().is_some() {
            return Err(ParseFormatSpecError::new(spec, ParseFormatSpecReason::Malformed));
        }
        if let Some(c) = type_pos {
            if let Some(tc) = TypeChar::from_char(c) {
                result.type_char = Some(tc);
            } else {
                // An unrecognised type char: an integer grouping option present
                // alongside it makes CPython report the grouping conflict (which
                // takes precedence), otherwise it's an unknown format code.
                let reason = match result.grouping {
                    Some(g) => ParseFormatSpecReason::grouping_conflict(g, c),
                    None => ParseFormatSpecReason::UnknownFormatCode(c),
                };
                return Err(ParseFormatSpecError::new(spec, reason));
            }
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

    // Bool is an `int` subclass: every integer/float presentation applies to it
    // using the numeric value (`1`/`0`), so treat it as an int for dispatch
    // while keeping `value_type` (= bool) for error wording. The exception is a
    // *directive-free* spec — `str(bool)` wins there (`format(True, "")` is
    // `"True"`, not `"1"`). The compiler already elides a literal empty spec
    // `f"{b:}"`, but a *runtime* empty spec (`f"{b:{s}}"`, `s == ""`) still
    // arrives here as an all-default `ParsedFormatSpec`, so this guard is what
    // keeps `format(True, "")` → `"True"`.
    let coerced_bool;
    let value = if let Value::Bool(b) = value
        && spec_has_directives(spec)
    {
        coerced_bool = Value::Int(i64::from(*b));
        &coerced_bool
    } else {
        value
    };

    // `spec.width` is the minimum field width; every formatter below pads the
    // value out to it with `spec.fill` via `pad_string`/`iter::repeat_n`, which
    // build a native `String` through the global allocator — invisible to the
    // resource tracker until the finished string reaches the heap. A literal
    // width is clamped to 16 bits by the bytecode encoding, but a *dynamic*
    // width (`f"{v:>{w}}"`, `w` a runtime value) is not, so an over-large `w`
    // would materialize gigabytes of padding before the post-construction
    // check, OOM-ing or aborting the host. Reject an over-budget width here,
    // up front, using the same guard sequence repeats and `str.ljust`/`zfill`
    // already use. The check is free below `LARGE_RESULT_THRESHOLD`.
    check_repeat_size(spec.fill.len_utf8(), spec.width, vm.heap.tracker())?;

    // `spec.precision` on the float formats is rendered as that many decimal
    // digits. `fmt_float_fixed` / `fmt_float_exp` synthesise the digits beyond
    // `MAX_FMT_PRECISION` by appending raw `'0'` chars to an untracked Rust
    // `String`, so an attacker-chosen precision (`f"{v:.{p}f}"`, `p` a runtime
    // value) would allocate gigabytes before `allocate_string` accounts for the
    // result. Precision is parsed as an unrestricted `usize`; bound it by the
    // active resource tracker the same way the width check above does. Skip for
    // non-finite floats since the helpers ignore precision in that case.
    //
    // This applies to `f`/`e`/`%` always, and to the `g`-family
    // (`g`/`G`/`n`/type-less-with-precision) *only* under alternate form (`#`):
    // plain `g` strips trailing zeros and caps internally, but `#g` keeps every
    // zero so its digit count scales with precision just like `f`.
    let precision_scales_output = matches!(
        spec.type_char,
        Some(TypeChar::F | TypeChar::FUpper | TypeChar::E | TypeChar::EUpper | TypeChar::Percent)
    ) || (spec.alternate
        && matches!(
            spec.type_char,
            None | Some(TypeChar::G | TypeChar::GUpper | TypeChar::N)
        ));
    if let Some(precision) = spec.precision
        && precision_scales_output
    {
        let numeric_finite = match value {
            Value::Int(_) => true,
            Value::Float(f) => f.is_finite(),
            // A big integer formatted as a float is first converted to `f64`,
            // so an attacker-chosen precision applies to it too — guard it.
            Value::Ref(id) => matches!(vm.heap.get(*id), HeapData::LongInt(_)),
            _ => false,
        };
        if numeric_finite {
            // Fractional grouping (`f"{v:.{p}_f}"`) weaves in one separator per
            // three emitted digits, so the native string reaches ~4/3 × precision
            // before `allocate_string` accounts for it; budget the separators too.
            let separators = if spec.frac_grouping.is_some() { precision / 3 } else { 0 };
            check_repeat_size(precision.saturating_add(separators), 1, vm.heap.tracker())?;
        }
    }

    // A `str` value is formatted entirely through the string mini-language:
    // `validate_string_spec` rejects (in CPython's precedence order) every flag
    // that is meaningless for text, then `format_string` applies precision,
    // width, and fill. Handling strings here in one place keeps them off the
    // numeric validation/dispatch below and makes the VM's `!s`/`!r`/`!a`
    // conversion paths — which reuse the very same two functions — reject
    // identical specs (`f"{x:#}"` and `f"{x!s:#}"` raise the same error).
    if value_type == Type::Str {
        validate_string_spec(spec)?;
        let s = value.py_str(vm)?;
        return Ok(format_string(&s, spec)?);
    }

    // A grouping option (`,`/`_`) is only legal for certain presentation
    // types; reject the illegal combinations with CPython's message before
    // dispatching to a formatter that would otherwise ignore the flag.
    if let Some(grouping) = spec.grouping {
        validate_grouping(grouping, spec.type_char, value_type)?;
    }

    // Precision is meaningless for integer presentations. CPython rejects it
    // inside the integer formatter — *after* the parse-time grouping check
    // above but *before* the `#`/sign checks below, so this ordering matters
    // (`{0:#.0c}` reports the precision error, not the alternate-form one).
    if spec.precision.is_some() && formats_as_integer(spec.type_char, value_type) {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "Precision not allowed in integer format specifier".to_owned(),
        )
        .into());
    }

    // If the presentation type is not valid for this value, CPython reports
    // "Unknown format code" *before* the alternate-form check (the type chooses
    // the formatter, so an invalid type errors first): `f"{3.14:#c}"` is
    // "Unknown format code 'c' for object of type 'float'", not the `#`-with-`c`
    // message. The dispatch `match` below also catches this as a safety net.
    if let Some(c) = spec.type_char
        && !type_valid_for_value(spec.type_char, value_type)
    {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!(
                "Unknown format code '{}' for object of type '{value_type}'",
                c.as_char()
            ),
        )
        .into());
    }

    // `z` (negative-zero coercion) is only meaningful for floating-point
    // presentations. CPython rejects it elsewhere with a presentation-specific
    // message, *after* the "Unknown format code" check (so a bad type wins) and
    // before the alternate-form check (`f"{5:z#}"` reports the `z` error).
    // Strings are handled by `validate_string_spec`; here the value is numeric or
    // a non-numeric type-less spec (string-formatted, like `validate_alternate`).
    if spec.z && !formats_as_float(spec.type_char, value_type) {
        let kind = if formats_as_integer(spec.type_char, value_type) {
            "integer"
        } else {
            "string"
        };
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("Negative zero coercion (z) not allowed in {kind} format specifier"),
        )
        .into());
    }

    // A sign flag (`+`/`-`/space) is meaningless for the `c` (character)
    // presentation, which has no numeric value to sign. CPython rejects it in
    // the integer formatter *after* the `z` check (`f"{0: zc}"` reports the `z`
    // error, not this one) and before the alternate-form check. The
    // `Int`/`Bool` guard keeps a `c` on any other type routing to the
    // "Unknown format code" error above instead.
    if spec.sign.is_some() && spec.type_char == Some(TypeChar::C) && matches!(value_type, Type::Int | Type::Bool) {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "Sign not allowed with integer format specifier 'c'".to_owned(),
        )
        .into());
    }

    // The alternate form (`#`) is likewise illegal for `c`/`s` presentations.
    if spec.alternate {
        validate_alternate(spec.type_char, value_type)?;
    }

    // Big integers (`LongInt`) live on the heap; route them through the
    // arbitrary-precision formatter, which mirrors the `i64` integer/float
    // paths. The validations above (grouping, precision, sign-with-`c`,
    // alternate) already ran against `value_type == Type::Int`, so only the
    // value dispatch remains.
    if let Value::Ref(id) = value
        && let HeapData::LongInt(li) = vm.heap.get(*id)
    {
        return format_long_int(li, value_type, spec, vm.heap.tracker());
    }

    match (value, spec.type_char) {
        // Integer formatting. `n` (locale number) formats an integer like `d`;
        // Monty has no locale, so the C-locale form — no grouping — applies.
        (Value::Int(n), None | Some(TypeChar::D | TypeChar::N)) => Ok(format_int(*n, spec)),
        (Value::Int(n), Some(TypeChar::B)) => Ok(format_int_base(*n, 2, false, spec)?),
        (Value::Int(n), Some(TypeChar::O)) => Ok(format_int_base(*n, 8, false, spec)?),
        (Value::Int(n), Some(TypeChar::X)) => Ok(format_int_base(*n, 16, false, spec)?),
        (Value::Int(n), Some(TypeChar::XUpper)) => Ok(format_int_base(*n, 16, true, spec)?),
        (Value::Int(n), Some(TypeChar::C)) => Ok(format_char(*n, spec)?),

        // Float formatting. A type-less spec with no precision uses the repr
        // (shortest) digits, *not* `g` — only an explicit `g`/`G` or a type-less
        // spec that carries a precision selects the `g` algorithm.
        (Value::Float(f), None) if spec.precision.is_none() => Ok(format_float_default(*f, spec)),
        // `n` on a float behaves like `g` (locale-aware in CPython; C-locale
        // here, so plain `g`).
        (Value::Float(f), None | Some(TypeChar::G | TypeChar::GUpper | TypeChar::N)) => Ok(format_float_g(*f, spec)),
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

        // No type specifier on a non-string value: convert to string and
        // format. (`str` values are handled by the short-circuit above.)
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

/// Validates a format spec for a `str` value, raising the same `ValueError`s
/// CPython does for flags that are meaningless when formatting text.
///
/// This is the single source of truth for "is this spec legal for a `str`": the
/// string branch of [`format_with_spec`] and the VM's `!s`/`!r`/`!a` conversion
/// paths (which convert to a string and then format it) both route through it,
/// so a value and its converted forms reject identical specs. Checks run in
/// CPython's precedence order — the first violation wins:
/// 1. a grouping option (`,`/`_`) — there is nothing to group in text
/// 2. a presentation type other than `s`
/// 3. a sign flag (`+`/`-`/space)
/// 4. the alternate form (`#`)
/// 5. `=` (sign-aware) alignment
pub fn validate_string_spec(spec: &ParsedFormatSpec) -> Result<(), RunError> {
    if let Some(grouping) = spec.grouping {
        validate_grouping(grouping, spec.type_char, Type::Str)?;
    }
    if let Some(c) = spec.type_char
        && c != TypeChar::S
    {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("Unknown format code '{}' for object of type 'str'", c.as_char()),
        )
        .into());
    }
    if let Some(sign) = spec.sign {
        let msg = match sign {
            Sign::Space => "Space not allowed in string format specifier",
            Sign::Plus | Sign::Minus => "Sign not allowed in string format specifier",
        };
        return Err(SimpleException::new_msg(ExcType::ValueError, msg.to_owned()).into());
    }
    if spec.z {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "Negative zero coercion (z) not allowed in string format specifier".to_owned(),
        )
        .into());
    }
    if spec.alternate {
        validate_alternate(spec.type_char, Type::Str)?;
    }
    if spec.align == Some(Align::SignAware) {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "'=' alignment not allowed in string format specifier".to_owned(),
        )
        .into());
    }
    Ok(())
}

/// Checks that a grouping option is valid for the chosen presentation type,
/// returning CPython's `Cannot specify ',' with 'x'.` error otherwise.
///
/// `,` is only allowed for decimal/float presentations; `_` additionally
/// allows the binary/octal/hex integer presentations (grouped in fours). For
/// a type-less spec the presentation is taken from the value: numeric values
/// permit grouping, everything else is string-formatted and reported as `s`.
fn validate_grouping(grouping: Grouping, type_char: Option<TypeChar>, value_type: Type) -> Result<(), RunError> {
    let allowed = match type_char {
        None => matches!(value_type, Type::Int | Type::Bool | Type::Float),
        Some(TypeChar::B | TypeChar::O | TypeChar::X | TypeChar::XUpper) => grouping == Grouping::Underscore,
        // `c`/`s` have nothing to group; `n` does its own locale grouping and so
        // forbids an explicit one (`Cannot specify ',' with 'n'.`).
        Some(TypeChar::C | TypeChar::S | TypeChar::N) => false,
        // D, E/EUpper, F/FUpper, G/GUpper, Percent all accept both `,` and `_`.
        Some(_) => true,
    };
    if allowed {
        Ok(())
    } else {
        // For a type-less spec the value is being string-formatted, so the
        // presentation char CPython reports is `s`.
        let presentation = type_char.map_or('s', TypeChar::as_char);
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("Cannot specify '{}' with '{presentation}'.", grouping.separator()),
        )
        .into())
    }
}

/// Whether the presentation `type_char` is valid for a value of `value_type`,
/// i.e. a formatter exists for the pair. A `false` result is CPython's "Unknown
/// format code" case.
///
/// Integer codes (`b`/`c`/`d`/`o`/`x`/`X`) need an `int`/`bool`; float codes
/// (`e`/`E`/`f`/`F`/`g`/`G`/`%`) and `n` accept any number (an `int` is widened
/// to a float); `s` needs a `str`; a type-less spec (`None`) is valid for every
/// value (it falls back to `str()`).
fn type_valid_for_value(type_char: Option<TypeChar>, value_type: Type) -> bool {
    let is_int = matches!(value_type, Type::Int | Type::Bool);
    let is_num = is_int || value_type == Type::Float;
    match type_char {
        None => true,
        Some(TypeChar::D | TypeChar::B | TypeChar::O | TypeChar::X | TypeChar::XUpper | TypeChar::C) => is_int,
        Some(
            TypeChar::E
            | TypeChar::EUpper
            | TypeChar::F
            | TypeChar::FUpper
            | TypeChar::G
            | TypeChar::GUpper
            | TypeChar::Percent
            | TypeChar::N,
        ) => is_num,
        Some(TypeChar::S) => value_type == Type::Str,
    }
}

/// Whether `(type_char, value_type)` selects an *integer* presentation, i.e.
/// the value will be rendered via the integer formatter.
///
/// True for the explicit integer codes (`d`/`b`/`o`/`x`/`X`/`c`) and for a
/// type-less spec on an `int`/`bool` value. Crucially it keys off `value_type`
/// (not the `Value` variant) so big integers (`LongInt`, stored as a heap ref
/// but typed `int`) are included. A float or string with one of those codes is
/// *not* an integer presentation — it's a type error handled at dispatch — so
/// this returns false there, letting the "Unknown format code" message win.
fn formats_as_integer(type_char: Option<TypeChar>, value_type: Type) -> bool {
    let int_value = matches!(value_type, Type::Int | Type::Bool);
    match type_char {
        // `n` selects the integer formatter for an int value (so precision is
        // rejected there) but the float formatter for a float (precision ok).
        Some(TypeChar::D | TypeChar::B | TypeChar::O | TypeChar::X | TypeChar::XUpper | TypeChar::C | TypeChar::N) => {
            int_value
        }
        None => int_value,
        _ => false,
    }
}

/// Whether `(type_char, value_type)` selects a *float* presentation, i.e. the
/// value will be rendered via one of the `format_float_*` functions.
///
/// The float codes (`e`/`E`/`f`/`F`/`g`/`G`/`%`) always format as a float
/// (an `int` is widened); `n` and a type-less spec do so only for an actual
/// float value. Used to gate the `z` (negative-zero coercion) flag, which is
/// legal only for float presentations.
fn formats_as_float(type_char: Option<TypeChar>, value_type: Type) -> bool {
    match type_char {
        Some(
            TypeChar::E
            | TypeChar::EUpper
            | TypeChar::F
            | TypeChar::FUpper
            | TypeChar::G
            | TypeChar::GUpper
            | TypeChar::Percent,
        ) => true,
        Some(TypeChar::N) | None => value_type == Type::Float,
        _ => false,
    }
}

/// Whether a spec carries any directive — i.e. it is more than the bare,
/// directive-free spec that is equivalent to `str(value)`.
///
/// Used to decide how a `bool` formats: a directive-free spec yields
/// `str(bool)` (`"True"`), while any directive switches to numeric `int`
/// formatting (`format(True, " ")` → `" 1"`). Equivalent to "the original spec
/// string was empty", since every spec character sets at least one field.
fn spec_has_directives(spec: &ParsedFormatSpec) -> bool {
    spec.align.is_some()
        || spec.sign.is_some()
        || spec.alternate
        || spec.z
        || spec.zero_pad
        || spec.width != 0
        || spec.grouping.is_some()
        || spec.precision.is_some()
        || spec.type_char.is_some()
}

/// Checks that the alternate form (`#`) is valid for the chosen presentation
/// type, returning CPython's wording otherwise.
///
/// `#` is a no-op for decimal integers and meaningful for binary/octal/hex
/// integers and all float presentations, but is rejected for the character
/// (`c`) and string (`s`) presentations — the latter also covers a type-less
/// spec applied to a non-numeric value (which is string-formatted).
fn validate_alternate(type_char: Option<TypeChar>, value_type: Type) -> Result<(), RunError> {
    let message = match type_char {
        Some(TypeChar::C) => Some("Alternate form (#) not allowed with integer format specifier 'c'"),
        Some(TypeChar::S) => Some("Alternate form (#) not allowed in string format specifier"),
        None if !matches!(value_type, Type::Int | Type::Bool | Type::Float) => {
            Some("Alternate form (#) not allowed in string format specifier")
        }
        _ => None,
    };
    match message {
        Some(msg) => Err(SimpleException::new_msg(ExcType::ValueError, msg.to_owned()).into()),
        None => Ok(()),
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
/// - bits 60-61: [`Grouping`] (0=none, 1=Comma, 2=Underscore)
/// - bit 62: alternate form (`#`)
///
/// Bit 62 is the last free bit (bit 63 is the sign bit, which must stay 0); a
/// future flag would need the packing reworked rather than another bit added.
pub fn encode_format_spec(spec: &ParsedFormatSpec) -> Option<i64> {
    // Fractional grouping and the `z` flag have no spare encoding bit (bit 62,
    // alternate, is the last free one), so any spec that uses either falls back
    // to the dynamic path where the literal is re-parsed at runtime.
    if spec.frac_grouping.is_some() || spec.z {
        return None;
    }
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
    let grouping: i64 = spec.grouping.map_or(0, |g| match g {
        Grouping::Comma => 1,
        Grouping::Underscore => 2,
    });
    let alternate = i64::from(spec.alternate);

    // Every field occupies bits 0..63, so the sign bit is never set and the
    // shifts/ORs stay within well-defined i64 territory.
    Some(
        fill | (align << 8)
            | (sign << 11)
            | (zero_pad << 13)
            | (width << 14)
            | (precision << 34)
            | (type_char << 55)
            | (grouping << 60)
            | (alternate << 62),
    )
}

/// Decodes an [`i64`] back into a [`ParsedFormatSpec`].
///
/// Reverses the bit-packing done by [`encode_format_spec`]. Used by the VM
/// when executing `FormatValue` with the `FORMAT_VALUE_STATIC_SPEC` flag to
/// recover the pre-parsed spec from the constant pool entry.
pub fn decode_format_spec(encoded: i64) -> ParsedFormatSpec {
    // The valid encoding sits in bits 0..63 so `cast_unsigned` is a no-op
    // reinterpret — the sign bit (bit 63) is always 0 here.
    let encoded = encoded.cast_unsigned();
    let fill = (encoded & 0xFF) as u8 as char;
    let align_bits = (encoded >> 8) & 0x07;
    let sign_bits = (encoded >> 11) & 0x03;
    let zero_pad = ((encoded >> 13) & 0x01) != 0;
    let width = ((encoded >> 14) & 0xF_FFFF) as usize;
    let precision_raw = ((encoded >> 34) & 0x1F_FFFF) as usize;
    let type_bits = ((encoded >> 55) & 0x1F) as u8;
    let grouping_bits = (encoded >> 60) & 0x03;
    let alternate = ((encoded >> 62) & 0x01) != 0;

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

    let grouping = match grouping_bits {
        1 => Some(Grouping::Comma),
        2 => Some(Grouping::Underscore),
        _ => None,
    };

    ParsedFormatSpec {
        fill,
        align,
        sign,
        alternate,
        // Specs using the `z` flag are never encoded (see `encode_format_spec`),
        // so a decoded spec never carries one.
        z: false,
        zero_pad,
        width,
        grouping,
        // Specs with fractional grouping are never encoded (see
        // `encode_format_spec`), so a decoded spec never carries one.
        frac_grouping: None,
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
    let sign = numeric_sign(is_negative, &abs_str, spec);
    pad_signed_numeric(sign, "", &abs_str, spec)
}

/// Formats an integer in binary (base 2), octal (base 8), or hexadecimal (base 16).
///
/// Used for format types `b`, `o`, `x`, and `X`. The sign is prepended for
/// negative numbers. The alternate form (`#`) adds the `0b`/`0o`/`0x` base
/// prefix, which sits after the sign and counts toward the field width.
///
/// `uppercase` (the `X` type) uppercases the digits **and** the `0x` prefix
/// (`0X`) — but *not* the fill/padding, so it must be applied to the digits
/// here rather than via `to_uppercase()` on the padded result (which would also
/// uppercase an alphabetic fill, e.g. `f"{180:a>8X}"` → `aaaaaaB4`).
/// Returns an error for invalid base values.
pub fn format_int_base(n: i64, base: u32, uppercase: bool, spec: &ParsedFormatSpec) -> Result<String, FormatError> {
    let is_negative = n < 0;
    let abs_val = n.unsigned_abs();

    let (abs_str, base_prefix) = match (base, uppercase) {
        (2, _) => (format!("{abs_val:b}"), "0b"),
        (8, _) => (format!("{abs_val:o}"), "0o"),
        (16, false) => (format!("{abs_val:x}"), "0x"),
        (16, true) => (format!("{abs_val:X}"), "0X"),
        _ => return Err(FormatError::ValueError("Invalid base".to_owned())),
    };
    let prefix = if spec.alternate { base_prefix } else { "" };

    let sign = numeric_sign(is_negative, &abs_str, spec);
    Ok(pad_signed_numeric(sign, prefix, &abs_str, spec))
}

/// Formats a big integer ([`LongInt`]) through the numeric mini-language,
/// mirroring the `i64` paths ([`format_int`] / [`format_int_base`] /
/// `format_float_*`) but sourcing digits from the arbitrary-precision `BigInt`.
///
/// Integer presentations (`d`/`b`/`o`/`x`/`X` and the default) convert the
/// magnitude to the requested radix and reuse [`pad_signed_numeric`] for sign,
/// grouping, prefix, and padding. Float presentations (`f`/`e`/`g`/`%`) convert
/// to `f64` first, raising `OverflowError` when the value is too large to
/// represent — exactly as CPython does. `c` overflows the code-point range
/// (a `LongInt` is always `> i64::MAX`) and `s` is an unknown code, matching
/// CPython's two distinct errors. `value_type` only feeds the `s` message and
/// is always `Type::Int` here.
///
/// `tracker` guards the radix conversion: `BigInt::to_str_radix` materializes
/// the full digit string on the (untracked) Rust heap before `allocate_string`
/// runs, so a huge `LongInt` formatted as `:b`/`:o`/`:x` could allocate gigabytes
/// outside the resource limit (`f"{1 << n:b}"`). Each radix render is size-checked
/// up front against `tracker`.
fn format_long_int(
    li: &LongInt,
    value_type: Type,
    spec: &ParsedFormatSpec,
    tracker: &impl ResourceTracker,
) -> Result<String, RunError> {
    let sign = if li.is_negative() {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    };
    let magnitude = li.abs();
    // `uppercase` (the `X` type) uppercases the hex digits and the `0x` prefix
    // (`0X`) but never the fill, so it's applied to the digits *before* padding.
    let radix = |base: u32, base_prefix: &'static str, uppercase: bool| -> Result<String, RunError> {
        // Pre-check the digit-string size: a base-`b` render of an `n`-bit
        // magnitude is at most `n / log2(b)` ASCII digits — `log2(b)` is
        // `base.trailing_zeros()` for the power-of-two bases here; base 10 uses
        // `1` (conservative) but is already bounded by `check_bits_str_digits_limit`.
        let max_digits = li.bits() / u64::from(base.trailing_zeros().max(1));
        check_repeat_size(
            1,
            usize::try_from(max_digits).unwrap_or(usize::MAX).saturating_add(2),
            tracker,
        )?;
        let mut digits = magnitude.inner().to_str_radix(base);
        let prefix = if !spec.alternate {
            ""
        } else if uppercase {
            // Only hex is ever uppercased, so `0x` → `0X`.
            "0X"
        } else {
            base_prefix
        };
        if uppercase {
            digits.make_ascii_uppercase();
        }
        Ok(pad_signed_numeric(sign, prefix, &digits, spec))
    };
    let as_float = || match li.to_f64() {
        Some(f) if f.is_finite() => Ok(f),
        _ => Err(RunError::from(SimpleException::new_msg(
            ExcType::OverflowError,
            "int too large to convert to float".to_owned(),
        ))),
    };

    match spec.type_char {
        // `n` formats a big int as decimal (C locale, no grouping), like `d`.
        None | Some(TypeChar::D | TypeChar::N) => {
            // Only base-10 conversion is bounded by CPython's int_max_str_digits.
            check_bits_str_digits_limit(li.bits())?;
            radix(10, "", false)
        }
        Some(TypeChar::B) => radix(2, "0b", false),
        Some(TypeChar::O) => radix(8, "0o", false),
        Some(TypeChar::X) => radix(16, "0x", false),
        Some(TypeChar::XUpper) => radix(16, "0x", true),
        Some(TypeChar::F | TypeChar::FUpper) => Ok(format_float_f(as_float()?, spec)),
        Some(TypeChar::E) => Ok(format_float_e(as_float()?, spec, false)),
        Some(TypeChar::EUpper) => Ok(format_float_e(as_float()?, spec, true)),
        Some(TypeChar::G | TypeChar::GUpper) => Ok(format_float_g(as_float()?, spec)),
        Some(TypeChar::Percent) => Ok(format_float_percent(as_float()?, spec)),
        // A `LongInt` is always out of the C-long range `c` converts through.
        Some(TypeChar::C) => Err(SimpleException::new_msg(
            ExcType::OverflowError,
            "Python int too large to convert to C long".to_owned(),
        )
        .into()),
        // `s` is not an integer presentation; surface CPython's "Unknown format
        // code" wording (the `i64` path reaches the same message via its
        // dispatch fall-through).
        Some(TypeChar::S) => Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("Unknown format code 's' for object of type '{value_type}'"),
        )
        .into()),
    }
}

/// Formats an integer as a Unicode character (format type `c`).
///
/// Converts the integer to its corresponding Unicode code point. Valid range is 0 to 0x10FFFF.
/// Returns `Overflow` error if out of range, `ValueError` if not a valid Unicode scalar value
/// (e.g., surrogate code points). Right-aligned by default: `c` is an integer
/// presentation, so it follows the numeric default (`format(65, '5c')` → `'    A'`).
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
    let align = match spec.align.unwrap_or(Align::Right) {
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
    let is_negative = f.is_sign_negative() && !f.is_nan();
    let uppercase = spec.type_char == Some(TypeChar::FUpper);
    let abs_str = if let Some(word) = non_finite_repr(f, uppercase) {
        word.to_owned()
    } else {
        let abs_val = f.abs();
        let abs_str = fmt_float_fixed(abs_val, spec.precision.unwrap_or(6));
        maybe_alternate_point(abs_str, abs_val, spec)
    };
    let sign = numeric_sign(is_negative, &abs_str, spec);
    pad_signed_numeric(sign, "", &abs_str, spec)
}

/// Formats a float in exponential/scientific notation (format types `e` and `E`).
///
/// Produces output like `1.234568e+03` with `precision` digits after decimal (default 6).
/// The `uppercase` parameter controls whether to use `E` or `e` for the exponent marker.
/// Exponent is always formatted with a sign and at least 2 digits (Python convention).
pub fn format_float_e(f: f64, spec: &ParsedFormatSpec, uppercase: bool) -> String {
    let is_negative = f.is_sign_negative() && !f.is_nan();
    let abs_str = if let Some(word) = non_finite_repr(f, uppercase) {
        word.to_owned()
    } else {
        let abs_val = f.abs();
        let abs_str = fmt_float_exp(abs_val, spec.precision.unwrap_or(6), uppercase);
        // Fix exponent format to match Python (e+03 not e3)
        let abs_str = fix_exp_format(&abs_str);
        maybe_alternate_point(abs_str, abs_val, spec)
    };
    let sign = numeric_sign(is_negative, &abs_str, spec);
    pad_signed_numeric(sign, "", &abs_str, spec)
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
    let is_negative = f.is_sign_negative() && !f.is_nan();
    // `G` (and only `G`) uppercases the exponent marker and `inf`/`nan`.
    let uppercase = spec.type_char == Some(TypeChar::GUpper);
    // Non-finite values short-circuit before the `log10`/exponent maths below,
    // which would be meaningless for `inf`/`nan`.
    if let Some(word) = non_finite_repr(f, uppercase) {
        // `z` never coerces a non-finite value (`word` has no digits), so the
        // sign is `"-"` for `-inf` as usual.
        let sign = numeric_sign(is_negative, word, spec);
        return pad_signed_numeric(sign, "", word, spec);
    }

    let precision = spec.precision.unwrap_or(6).max(1);
    let abs_val = f.abs();

    // Python's g format uses exponential when the decimal exponent is `< -4` or
    // `>= precision`. The exponent must be taken *after* rounding to `precision`
    // significant figures: a value that rounds up across a power-of-ten
    // boundary (`9.99` at 1 sig fig → `10`) gains an exponent and must switch to
    // scientific (`f"{9.99:.1g}"` → `'1e+01'`, not `'10'`). Rust's `{:e}` rounds
    // for us, so read the exponent back from the rounded scientific form rather
    // than from `log10` of the unrounded value.
    let exp = if abs_val == 0.0 {
        0
    } else {
        let mantissa_digits = precision.saturating_sub(1).min(MAX_FMT_PRECISION_EXP);
        let sci = format!("{abs_val:.mantissa_digits$e}");
        sci[sci.find('e').map_or(sci.len(), |i| i + 1)..]
            .parse::<i32>()
            .unwrap_or(0)
    };

    // precision is typically small (default 6), safe to convert to i32
    let prec_i32 = i32::try_from(precision).unwrap_or(i32::MAX);
    // A type-less spec that reaches here carries a precision (the no-precision
    // case goes through `format_float_default`). CPython's type-less-with-
    // precision form is `g`-like but diverges in two ways: it switches to
    // scientific one exponent *earlier* (`exp >= precision - 1`, vs `g`'s
    // `>= precision`) and it appends `.0` when the result has no point/exponent
    // (the `Py_DTSF_ADD_DOT_0` flag) — e.g. `f"{100.0:.3}"` is `'1e+02'` (not
    // `g`'s `'100'`) and `f"{1.0:.2}"` is `'1.0'` (not `'1'`).
    let is_default = spec.type_char.is_none();
    let sci_threshold = if is_default { prec_i32 - 1 } else { prec_i32 };
    // The alternate form (`#`) keeps the trailing zeros that `g` normally strips
    // and forces a decimal point. This applies whenever the `g` algorithm runs
    // — an explicit `g`/`G` *or* a type-less spec that carries a precision.
    let alternate_g = spec.alternate;
    let abs_str = if exp < -4 || exp >= sci_threshold {
        // Use exponential notation
        let exp_prec = precision.saturating_sub(1);
        if alternate_g {
            // `#` preserves every mantissa zero, so the output scales with
            // precision: let `fmt_float_exp` synthesise the digits beyond Rust's
            // formatter cap, then normalise the exponent (`e+06`). Bounded by the
            // precision pre-check in `format_with_spec`.
            fix_exp_format(&fmt_float_exp(abs_val, exp_prec, uppercase))
        } else {
            // Plain `g` strips trailing zeros, so mantissa digits beyond the cap
            // would be dropped anyway — cap to avoid generating them.
            strip_trailing_zeros_exp(&fmt_float_exp(abs_val, exp_prec.min(MAX_FMT_PRECISION_EXP), uppercase))
        }
    } else {
        // Use fixed notation - result is non-negative due to .max(0)
        let sig_digits_i32 = (prec_i32 - exp - 1).max(0);
        let sig_digits = usize::try_from(sig_digits_i32).expect("sig_digits guaranteed non-negative");
        if alternate_g {
            // `#` keeps the trailing zeros, so the digit count scales with
            // precision; `fmt_float_fixed` synthesises any beyond the formatter
            // cap (bounded by the precision pre-check in `format_with_spec`).
            fmt_float_fixed(abs_val, sig_digits)
        } else {
            // Plain `g` strips trailing zeros, so digits beyond the cap would be
            // dropped anyway — cap before formatting.
            let cap = sig_digits.min(MAX_FMT_PRECISION);
            strip_trailing_zeros(&format!("{abs_val:.cap$}"))
        }
    };
    let abs_str = if alternate_g {
        maybe_alternate_point(abs_str, abs_val, spec)
    } else {
        abs_str
    };
    // `Py_DTSF_ADD_DOT_0`: the type-less form always shows a decimal point, so
    // a result with no `.`/`e` gets a trailing `.0` (`'1234'` → `'1234.0'`).
    let abs_str = if is_default && !abs_str.contains(['.', 'e', 'E']) {
        format!("{abs_str}.0")
    } else {
        abs_str
    };

    let sign = numeric_sign(is_negative, &abs_str, spec);
    pad_signed_numeric(sign, "", &abs_str, spec)
}

/// Formats a float with the *default* presentation — no type char and no
/// precision — applying sign, grouping, and field padding around the shortest
/// round-tripping digits ([`FormatFloat`]).
///
/// CPython's type-less float format uses repr mode here, **not** `g` with the
/// default precision 6: `f"{1234567.0:>12}"` is `"   1234567.0"`, not
/// `"  1.23457e+06"`. A *precision* on a type-less float (`f"{x:.3}"`) does
/// select the `g` algorithm and so stays in [`format_float_g`].
///
/// Unlike the repr/str path this needs an owned `String` (the result is
/// post-processed by `maybe_alternate_point` and then padded), so it
/// materializes [`FormatFloat`] via `to_string` rather than streaming.
fn format_float_default(f: f64, spec: &ParsedFormatSpec) -> String {
    let is_negative = f.is_sign_negative() && !f.is_nan();
    let abs_val = f.abs();
    // The alternate form (`#`) still forces a decimal point on the repr digits,
    // even in scientific notation (`format(1e20, '#')` → `'1.e+20'`).
    let abs_str = maybe_alternate_point(FormatFloat(abs_val).to_string(), abs_val, spec);
    let sign = numeric_sign(is_negative, &abs_str, spec);
    pad_signed_numeric(sign, "", &abs_str, spec)
}

/// A [`Display`](fmt::Display) adapter that writes a float exactly as CPython's
/// `repr()`/`str()` (identical for floats in Python 3): the shortest decimal
/// string that round-trips, switching to scientific notation when the base-10
/// exponent is `< -4` or `>= 16`, and always keeping at least one fractional
/// digit (`1.0`, never `1`) — `1e16` → `"1e+16"`, `1234.5` → `"1234.5"`,
/// `inf`/`nan` lowercased.
///
/// This is the default rendering for a bare `f"{x}"`, `str(x)`, `repr(x)` and
/// floats inside container reprs — *not* the format mini-language (that's
/// [`format_float_g`] et al). Rust can't do this directly: its `f64` `Display`
/// never uses scientific notation (`1e16` prints as `10000000000000000`) and
/// renders NaN as `"NaN"`.
///
/// As a `Display` adapter it writes straight to the caller's sink with **no
/// heap allocation**: it borrows Rust's *shortest-digits* guarantee via `{:e}`
/// into a small stack buffer (an `f64` `{:e}` is ASCII and ≤ 24 bytes) and
/// re-lays-out those digits per CPython's rules.
pub struct FormatFloat(pub f64);

impl fmt::Display for FormatFloat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        if v.is_nan() {
            return f.write_str("nan");
        }
        if v.is_sign_negative() {
            f.write_char('-')?;
        }
        if v.is_infinite() {
            return f.write_str("inf");
        }
        // Rust's shortest scientific form gives minimal round-tripping digits
        // plus the base-10 exponent (`1234.5` → `"1.2345e3"`, `0.0` → `"0e0"`),
        // captured in a stack buffer so nothing touches the heap.
        let mut sci = StackStr::new();
        write!(sci, "{:e}", v.abs())?;
        let sci = sci.as_str();
        let (mantissa, exp_str) = sci.split_once('e').ok_or(fmt::Error)?;
        // `{:e}` always emits a single leading digit, so the integer part is one
        // char and the fraction (if any) follows the `.`.
        let (int_part, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let exp10: i32 = exp_str.parse().map_err(|_| fmt::Error)?;
        let ndigits = int_part.len() + frac.len();
        // `decpt` = number of digits to the left of the decimal point.
        let decpt = exp10 + 1;

        if !(-4..16).contains(&exp10) {
            // Scientific: leading digit, optional fraction, then `e±NN`.
            f.write_str(int_part)?;
            if !frac.is_empty() {
                f.write_char('.')?;
                f.write_str(frac)?;
            }
            let exp_sign = if exp10 < 0 { '-' } else { '+' };
            write!(f, "e{exp_sign}{:02}", exp10.unsigned_abs())
        } else if decpt <= 0 {
            // `0.00…digits` — `-decpt` leading zeros after the point.
            f.write_str("0.")?;
            for _ in 0..-decpt {
                f.write_char('0')?;
            }
            f.write_str(int_part)?;
            f.write_str(frac)
        } else {
            let decpt = usize::try_from(decpt).expect("decpt is positive in this branch");
            if decpt >= ndigits {
                // Integer-valued: digits, zeros up to the point, then `.0`.
                f.write_str(int_part)?;
                f.write_str(frac)?;
                for _ in 0..decpt - ndigits {
                    f.write_char('0')?;
                }
                f.write_str(".0")
            } else {
                // Point falls inside the digit run. `int_part` is a single digit
                // and `decpt >= 1`, so the split always lands within `frac`.
                f.write_str(int_part)?;
                let split = decpt - int_part.len();
                f.write_str(&frac[..split])?;
                f.write_char('.')?;
                f.write_str(&frac[split..])
            }
        }
    }
}

/// A fixed-capacity [`fmt::Write`] sink backed by a stack array, used to capture
/// a bounded `{:e}` rendering without a heap allocation.
///
/// 32 bytes comfortably holds any `f64` `{:e}` output (the longest is ~24 ASCII
/// bytes, e.g. `2.2250738585072014e-308`). A write that would overflow returns
/// [`fmt::Error`] rather than panicking — unreachable for the bounded `f64`
/// case, but it keeps the type panic-free for any future caller.
struct StackStr {
    buf: [u8; 32],
    len: usize,
}

impl StackStr {
    fn new() -> Self {
        Self { buf: [0; 32], len: 0 }
    }

    fn as_str(&self) -> &str {
        // Only `{:e}` of an `f64` is written here, which is always valid ASCII.
        str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl fmt::Write for StackStr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        let slot = self.buf.get_mut(self.len..end).ok_or(fmt::Error)?;
        slot.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
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
    let percent_val = f * 100.0;
    let is_negative = percent_val.is_sign_negative() && !percent_val.is_nan();
    // The `%` presentation has no uppercase variant, so `inf`/`nan` stay lower.
    let abs_str = if let Some(word) = non_finite_repr(percent_val, false) {
        format!("{word}%")
    } else {
        let abs_val = percent_val.abs();
        let abs_str = format!("{}%", fmt_float_fixed(abs_val, spec.precision.unwrap_or(6)));
        // `#` forces the point before the `%` (`#.0%` → `50.%`).
        maybe_alternate_point(abs_str, abs_val, spec)
    };
    let sign = numeric_sign(is_negative, &abs_str, spec);
    pad_signed_numeric(sign, "", &abs_str, spec)
}

// ============================================================================
// Helper functions
// ============================================================================

/// CPython's spelling of a non-finite float for a given presentation *case*.
///
/// Rust renders NaN as `"NaN"` (mixed case) and always lowercases infinity;
/// CPython instead uses `"nan"`/`"inf"` for the lowercase presentations
/// (`f`, `e`, `g`, `%`, and the default) and `"NAN"`/`"INF"` for the uppercase
/// ones (`F`, `E`, `G`). Returns `None` for a finite value so the caller falls
/// through to its normal digit formatting. The sign (for `-inf`) is applied by
/// the caller, so this returns the unsigned word only.
fn non_finite_repr(value: f64, uppercase: bool) -> Option<&'static str> {
    if value.is_nan() {
        Some(if uppercase { "NAN" } else { "nan" })
    } else if value.is_infinite() {
        Some(if uppercase { "INF" } else { "inf" })
    } else {
        None
    }
}

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

/// Computes the sign prefix for a formatted numeric magnitude, applying the `z`
/// flag's negative-zero coercion.
///
/// The standard sign helper for every numeric formatter (`format_int`,
/// `format_int_base`, `format_float_*`). It is `"-"` for a negative value and
/// otherwise [`positive_sign_prefix`], *except* that the `z` flag
/// (`f"{-0.001:z.1f}"` → `0.0`) coerces a negative value that *rounds to zero at
/// the chosen precision* to positive zero. The rounding is already baked into
/// `abs_str`, so "rounds to zero" is detected structurally via
/// [`is_rounded_zero`]. `z` is only ever set for float presentations (it is
/// rejected for integer/string ones in `format_with_spec`), so the integer
/// callers get the plain sign behaviour.
fn numeric_sign(is_negative: bool, abs_str: &str, spec: &ParsedFormatSpec) -> &'static str {
    if is_negative && !(spec.z && is_rounded_zero(abs_str)) {
        "-"
    } else {
        positive_sign_prefix(spec.sign)
    }
}

/// Whether a formatted magnitude string represents zero — it has at least one
/// digit and every digit is `0` (e.g. `"0.00"`, `"0.000000e+00"`, `"0%"`).
///
/// Requiring a digit excludes `inf`/`nan` (which carry none), so the `z` flag
/// never coerces the sign of a non-finite value.
fn is_rounded_zero(abs_str: &str) -> bool {
    let mut saw_digit = false;
    for b in abs_str.bytes() {
        if b.is_ascii_digit() {
            saw_digit = true;
            if b != b'0' {
                return false;
            }
        }
    }
    saw_digit
}

/// Applies the alternate form's (`#`) "always show a decimal point" rule to a
/// finite float string, returning it unchanged when `#` is absent.
///
/// The point is inserted before the exponent (`e`/`E`) or percent (`%`) marker,
/// or at the end for plain fixed-point — so `#.0f` of 1.0 → `1.`, `#.0e` →
/// `1.e+00`, `#.0%` of 0.5 → `50.%`. A no-op when a `.` is already present (any
/// non-zero precision) and skipped for non-finite values (`inf`/`nan` have no
/// point).
fn maybe_alternate_point(abs_str: String, abs_val: f64, spec: &ParsedFormatSpec) -> String {
    if !spec.alternate || !abs_val.is_finite() {
        return abs_str;
    }
    let marker = abs_str.find(['e', 'E', '%']).unwrap_or(abs_str.len());
    if abs_str[..marker].contains('.') {
        abs_str
    } else {
        format!("{}.{}", &abs_str[..marker], &abs_str[marker..])
    }
}

/// Pads `sign + prefix + abs_str` to `spec.width` with the right alignment
/// semantics for a signed numeric value.
///
/// `prefix` is the alternate-form base marker (`0x`/`0o`/`0b`) or `""`. It
/// sits immediately after the sign and before any internal padding, and counts
/// toward the field width — `format(255, '#06x')` → `0x00ff`.
///
/// Numeric formatters all share three padding modes:
/// - `zero_pad` (`0` flag): insert `'0'` between the prefix and the digits.
/// - `Align::SignAware` (`=`): insert `spec.fill` between the prefix and the
///   digits.
/// - Anything else: glue `sign` + `prefix` + `abs_str` together and let
///   [`pad_string`] place fill outside the value.
///
/// Without this helper each formatter that wants sign-aware behaviour had
/// to inline the same conditional, and the ones that *didn't* (the
/// non-decimal integer bases, all the float formats except `:f`) silently
/// dropped width for `=` — see `parse_errors.rs::format_spec_…` tests.
/// Default alignment is right because all callers are numeric formats;
/// `format_char` (default left, no sign) needs separate handling.
///
/// When `spec.grouping` is set the thousands separator is woven through the
/// integer digits before padding — see [`pad_signed_grouped`]. When
/// `spec.frac_grouping` is set, the fractional digits are grouped first (a
/// no-op for values with no fractional part, e.g. integers).
fn pad_signed_numeric(sign: &str, prefix: &str, abs_str: &str, spec: &ParsedFormatSpec) -> String {
    // Fractional grouping is applied up front so the integer-grouping/padding
    // below treats the grouped fraction as an opaque suffix.
    let frac_grouped;
    let abs_str = if let Some(g) = spec.frac_grouping {
        frac_grouped = insert_frac_grouping(abs_str, g.separator());
        frac_grouped.as_str()
    } else {
        abs_str
    };
    let align = spec.align.unwrap_or(Align::Right);
    match spec.grouping {
        None => pad_signed_ungrouped(sign, prefix, abs_str, align, spec),
        Some(grouping) => pad_signed_grouped(sign, prefix, abs_str, align, grouping, spec),
    }
}

/// Inserts `sep` into the fractional digit run every three digits *from the
/// left*, implementing Python 3.14's `[.precision][grouping]` fractional
/// grouping (`f"{x:.6_f}"` → `…123_457`).
///
/// The fractional run is the maximal `[0-9]*` immediately after the first `.`;
/// the integer part and any exponent/`%` suffix are left untouched. A no-op
/// when the string has no fractional part (e.g. an integer), so it is safe to
/// call unconditionally from [`pad_signed_numeric`]. Output size is bounded by
/// the (already tracked) input length plus one separator per three digits.
fn insert_frac_grouping(s: &str, sep: char) -> String {
    let Some(dot) = s.find('.') else {
        return s.to_owned();
    };
    let after = dot + 1;
    let frac_len = s[after..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(s.len() - after);
    let mut out = String::with_capacity(s.len() + frac_len / 3);
    out.push_str(&s[..after]);
    for (i, c) in s[after..after + frac_len].chars().enumerate() {
        if i > 0 && i.is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out.push_str(&s[after + frac_len..]);
    out
}

/// Padding for a signed numeric with no thousands grouping (the common case).
///
/// Handles the three modes documented on [`pad_signed_numeric`]: `0`-flag
/// zero-padding, `=` sign-aware fill, and ordinary outside padding. `prefix`
/// (the `#` base marker or `""`) is emitted right after the sign in every mode.
fn pad_signed_ungrouped(sign: &str, prefix: &str, abs_str: &str, align: Align, spec: &ParsedFormatSpec) -> String {
    if spec.zero_pad || align == Align::SignAware {
        let fill = if spec.zero_pad { '0' } else { spec.fill };
        let total_len = sign.len() + prefix.len() + abs_str.len();
        if spec.width > total_len {
            let padding = spec.width - total_len;
            let pad_str: String = iter::repeat_n(fill, padding).collect();
            format!("{sign}{prefix}{pad_str}{abs_str}")
        } else {
            format!("{sign}{prefix}{abs_str}")
        }
    } else {
        let value = format!("{sign}{prefix}{abs_str}");
        pad_string(&value, spec.width, align, spec.fill)
    }
}

/// Padding for a signed numeric with a thousands separator (`,` or `_`).
///
/// The separator only ever lands between *integer* digits, so `abs_str` is
/// first split into its leading digit run and any trailing part (fraction,
/// exponent, `%`). Binary/octal/hex presentations group in fours and never
/// carry a suffix; decimal/float presentations group in threes. `prefix` (the
/// `#` base marker or `""`) sits after the sign and is never grouped. The `0`
/// flag is special: its leading zeros become part of the number and are
/// themselves grouped to fill the field width (`format(1234, '08,')` →
/// `'0,001,234'`), whereas `=` fill and outside padding wrap an already-grouped
/// value.
fn pad_signed_grouped(
    sign: &str,
    prefix: &str,
    abs_str: &str,
    align: Align,
    grouping: Grouping,
    spec: &ParsedFormatSpec,
) -> String {
    let sep = grouping.separator();
    let is_base = matches!(
        spec.type_char,
        Some(TypeChar::B | TypeChar::O | TypeChar::X | TypeChar::XUpper)
    );
    let group_size = if is_base { 4 } else { 3 };
    let (int_digits, suffix) = if is_base {
        (abs_str, "")
    } else {
        // Decimal digits are 0-9 only, so the first non-digit marks the start
        // of the fraction/exponent/`%` suffix.
        let end = abs_str.find(|c: char| !c.is_ascii_digit()).unwrap_or(abs_str.len());
        abs_str.split_at(end)
    };

    // Non-finite values (`inf`/`nan`/`INF`/`NAN`) have no integer digits, so
    // there is nothing to group: CPython drops the separator entirely and
    // zero-pads the bare word ungrouped (`format(inf, '020,') == '…00inf'`,
    // no commas). Delegating here also avoids `insert_grouping` underflowing
    // on an empty digit string.
    if int_digits.is_empty() {
        return pad_signed_ungrouped(sign, prefix, abs_str, align, spec);
    }

    if spec.zero_pad {
        // Grow the integer part with grouped leading zeros until the whole
        // field (sign + prefix + grouped digits + suffix) reaches the width.
        let reserved = sign.len() + prefix.len() + suffix.len();
        let min_int_width = spec.width.saturating_sub(reserved);
        let grouped = insert_grouping(int_digits, group_size, sep, min_int_width);
        format!("{sign}{prefix}{grouped}{suffix}")
    } else if align == Align::SignAware {
        // `=` with a non-`0` fill: group normally, then insert fill (never
        // grouped) between the prefix and the value.
        let body = format!("{}{suffix}", insert_grouping(int_digits, group_size, sep, 0));
        let total_len = sign.len() + prefix.len() + body.len();
        if spec.width > total_len {
            let pad_str: String = iter::repeat_n(spec.fill, spec.width - total_len).collect();
            format!("{sign}{prefix}{pad_str}{body}")
        } else {
            format!("{sign}{prefix}{body}")
        }
    } else {
        let grouped = insert_grouping(int_digits, group_size, sep, 0);
        let value = format!("{sign}{prefix}{grouped}{suffix}");
        pad_string(&value, spec.width, align, spec.fill)
    }
}

/// Inserts `sep` between every `group_size` digits of `digits`, counting from
/// the right, optionally left-padding with `'0'` first so the grouped result
/// is at least `min_width` characters wide.
///
/// `min_width` drives the `0`-flag interaction: CPython grows the zero-padded
/// integer one digit at a time until digits-plus-separators reach the field
/// width, so the result can overshoot `min_width` by one when the final digit
/// also introduces a separator (`format(1234, '08,')` → `'0,001,234'`, 9
/// wide for a width of 8). Callers must pass a non-empty `digits` (numeric
/// formatters always emit at least one digit, and [`pad_signed_grouped`]
/// routes non-finite values away before reaching here); the arithmetic is
/// nonetheless hardened against an empty string so it can never panic. Output
/// size is bounded by `min_width` (already capped against the resource tracker
/// via `check_repeat_size` in [`format_with_spec`]), so a plain `String` is
/// safe here.
fn insert_grouping(digits: &str, group_size: usize, sep: char, min_width: usize) -> String {
    // `digits` are ASCII (decimal or hex), so byte length == char count.
    let ndigits = digits.len();
    // Grow the total digit count until digits + separators reach `min_width`.
    // `saturating_sub` guards the `total == 0` case defensively: callers must
    // pass a non-empty `digits`, but an empty string would otherwise underflow
    // here, and an underflow on attacker-controlled format specs is a host
    // panic (a sandbox DoS) we must never risk.
    let mut total = ndigits;
    while total + total.saturating_sub(1) / group_size < min_width {
        total += 1;
    }
    let zeros = total - ndigits;

    let mut out = String::with_capacity(total + total.saturating_sub(1) / group_size);
    let mut digit_chars = digits.chars();
    for i in 0..total {
        if i > 0 && (total - i).is_multiple_of(group_size) {
            out.push(sep);
        }
        // The first `zeros` positions are synthesised leading zeros; the rest
        // consume the original digit string left-to-right.
        out.push(if i < zeros {
            '0'
        } else {
            digit_chars.next().expect("digit_chars yields exactly ndigits items")
        });
    }
    out
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
