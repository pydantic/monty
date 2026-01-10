//! F-string type definitions.
//!
//! This module contains the AST types for f-strings (formatted string literals).
//! F-strings can contain literal text and interpolated expressions with optional
//! conversion flags (`!s`, `!r`, `!a`) and format specifications.
//!
//! Runtime evaluation of f-strings is handled by the bytecode VM.

use std::str::FromStr;

use crate::{expressions::ExprLoc, intern::StringId};

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
