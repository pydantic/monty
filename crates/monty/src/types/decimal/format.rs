//! `Decimal.__format__`: the format-spec mini-language, as `_pydecimal` implements it.
//!
//! The spec grammar is `[[fill]align][sign][z][#][0][width][,|_][.[precision][,|_]][type]`
//! with types `eEfFgGn%`. Every malformed spec raises C `_decimal`'s
//! `ValueError: invalid format string`. `n` formats as `g` with the C locale's
//! separators (none), which is what CPython uses unless `locale.setlocale` was called.
//! Output and every zero padding are built through [`StringBuilder`], since a width,
//! precision or exponent can each expand a short value into a huge string.

use std::str;

use super::{Coeff, Ctx, Decimal, context::Rounding, round_direction};
use crate::{
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    string_builder::StringBuilder,
};

impl Decimal {
    /// `format(self, spec)`. Rounding uses the context's rounding mode; a malformed
    /// spec raises `ValueError: invalid format string`.
    pub(crate) fn format_spec(&self, spec: &str, ctx: &mut Ctx<'_>) -> RunResult<String> {
        let spec = FormatSpec::parse(spec)?;

        // Special values ignore the type and precision.
        if self.is_special() {
            let sign = format_sign(self.sign(), &spec);
            let mut body = self.copy_abs().to_sci_string(false, ctx.settings.capitals);
            if spec.kind == Some('%') {
                body.push('%');
            }
            return format_align(sign, &body, &spec, ctx);
        }

        let kind = spec.kind.unwrap_or(if ctx.settings.capitals { 'G' } else { 'g' });
        let mut value = Parts {
            sign: self.sign(),
            coeff: self.coeff().clone(),
            exp: i128::from(self.exponent()),
        };
        if kind == '%' {
            value.exp += 2;
        }

        // Round if necessary, taking the rounding mode from the context.
        let rounding = ctx.settings.rounding;
        if let Some(precision) = spec.precision {
            value = match kind {
                'e' | 'E' => value.round(precision + 1, rounding, ctx)?,
                'f' | 'F' | '%' => value.rescale(-precision, rounding, ctx)?,
                'g' | 'G' if value.digits() > precision => value.round(precision, rounding, ctx)?,
                _ => value,
            };
        }
        // Zeros with a positive exponent can't be shown in fixed point: rescale to 0e0.
        if value.is_zero() && value.exp > 0 && matches!(kind, 'f' | 'F' | '%') {
            value = value.rescale(0, rounding, ctx)?;
        }
        let negative = value.sign && !(value.is_zero() && spec.no_neg_0);

        // Where the decimal point goes.
        let coeff = value.coeff.to_string();
        let len = len_i128(&coeff);
        let leftdigits = value.exp + len;
        let dotplace = match kind {
            'e' | 'E' => match spec.precision {
                Some(precision) if value.is_zero() => 1 - precision,
                _ => 1,
            },
            'f' | 'F' | '%' => leftdigits,
            _ => {
                if value.exp <= 0 && leftdigits > -6 {
                    leftdigits
                } else {
                    1
                }
            }
        };

        // The digits before and after the point, and the exponent.
        let (intpart, fracpart) = if dotplace < 0 {
            let mut frac = zeros(-dotplace, ctx)?;
            frac.push_str(&coeff);
            ("0".to_owned(), frac)
        } else if dotplace > len {
            let mut int = coeff;
            int.push_str(&zeros(dotplace - len, ctx)?);
            (int, String::new())
        } else {
            let split = usize::try_from(dotplace).expect("dotplace is within the coefficient");
            let int = if split == 0 { "0" } else { &coeff[..split] };
            (int.to_owned(), coeff[split..].to_owned())
        };
        let exp = leftdigits - dotplace;
        format_number(negative, &intpart, &fracpart, exp, kind, &spec, ctx)
    }
}

/// A finite value as `(sign, coefficient, exponent)`, with an exponent wide enough
/// for a spec's precision to shift it without overflow.
struct Parts {
    sign: bool,
    coeff: Coeff,
    exp: i128,
}

impl Parts {
    /// Whether the value is zero.
    fn is_zero(&self) -> bool {
        self.coeff.is_zero()
    }

    /// The number of coefficient digits.
    fn digits(&self) -> i128 {
        i128::from(self.coeff.digits())
    }

    /// `_rescale(exp, rounding)`: the same value with exponent `exp`, padded with
    /// zeros or rounded. Quiet: raises no conditions.
    fn rescale(self, exp: i128, rounding: Rounding, ctx: &Ctx<'_>) -> RunResult<Self> {
        if self.is_zero() {
            return Ok(Self { exp, ..self });
        }
        if self.exp >= exp {
            let pad = pad_count(self.exp - exp, ctx)?;
            ctx.check_digits(pad.saturating_add(self.coeff.digits()))?;
            return Ok(Self {
                sign: self.sign,
                coeff: self.coeff.shl(pad),
                exp,
            });
        }
        // Too many digits: round them away. A shift past `u64` drops every digit either way.
        let shift = u64::try_from(exp - self.exp).unwrap_or(u64::MAX);
        let (mut kept, discarded) = self.coeff.shr(shift);
        if round_direction(&kept, discarded, self.sign, rounding) == 1 {
            kept.increment();
        }
        Ok(Self {
            sign: self.sign,
            coeff: kept,
            exp,
        })
    }

    /// `_round(places, rounding)`: rounds a non-zero value to `places` significant digits.
    fn round(self, places: i128, rounding: Rounding, ctx: &Ctx<'_>) -> RunResult<Self> {
        if self.is_zero() {
            return Ok(self);
        }
        let adjusted = self.adjusted();
        let answer = self.rescale(adjusted + 1 - places, rounding, ctx)?;
        // Rounding up can add a digit (99.97 to three places); rescale once more.
        if answer.adjusted() == adjusted {
            Ok(answer)
        } else {
            let adjusted = answer.adjusted();
            answer.rescale(adjusted + 1 - places, rounding, ctx)
        }
    }

    /// The exponent of the most significant digit.
    fn adjusted(&self) -> i128 {
        self.exp + self.digits() - 1
    }
}

/// A parsed Decimal format spec (`_parse_format_specifier`'s dict).
struct FormatSpec {
    fill: char,
    /// One of `<`, `>`, `=`, `^`.
    align: char,
    /// One of `+`, `-`, ` `.
    sign: char,
    /// `z`: format negative zero as zero.
    no_neg_0: bool,
    /// `#`: always show the decimal point.
    alt: bool,
    zeropad: bool,
    minimumwidth: i128,
    /// Separator for the integer digits (empty for none).
    thousands_sep: &'static str,
    /// Digits per thousands group: `Some(3)`, or `None` for the `n` type, which uses
    /// the C locale's empty grouping.
    group_len: Option<i128>,
    /// Separator for the fractional digits (empty for none).
    frac_sep: &'static str,
    precision: Option<i128>,
    /// One of `eEfFgG%`; `n` is stored as `g`, `None` means unspecified.
    kind: Option<char>,
}

impl FormatSpec {
    /// Parses a spec, rejecting anything outside the grammar with C `_decimal`'s error.
    fn parse(text: &str) -> RunResult<Self> {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        let is_align = |c: char| matches!(c, '<' | '>' | '=' | '^');
        let (fill, align) = if chars.len() >= 2 && is_align(chars[1]) {
            pos = 2;
            (Some(chars[0]), Some(chars[1]))
        } else if chars.first().is_some_and(|&c| is_align(c)) {
            pos = 1;
            (None, Some(chars[0]))
        } else {
            (None, None)
        };
        let next_if = |pos: &mut usize, pred: fn(char) -> bool| {
            let c = chars.get(*pos).copied().filter(|&c| pred(c));
            if c.is_some() {
                *pos += 1;
            }
            c
        };
        let sign = next_if(&mut pos, |c| matches!(c, '-' | '+' | ' ')).unwrap_or('-');
        let no_neg_0 = next_if(&mut pos, |c| c == 'z').is_some();
        let alt = next_if(&mut pos, |c| c == '#').is_some();
        let zeropad = next_if(&mut pos, |c| c == '0').is_some();
        let width_start = pos;
        while next_if(&mut pos, |c| c.is_ascii_digit()).is_some() {}
        let minimumwidth = parse_count(&chars[width_start..pos])?.unwrap_or(0);
        let thousands_sep = next_if(&mut pos, |c| matches!(c, ',' | '_'));
        let mut precision = None;
        let mut frac_sep = None;
        if chars.get(pos) == Some(&'.')
            && chars
                .get(pos + 1)
                .is_some_and(|&c| c.is_ascii_digit() || c == ',' || c == '_')
        {
            pos += 1;
            let start = pos;
            while next_if(&mut pos, |c| c.is_ascii_digit()).is_some() {}
            precision = parse_count(&chars[start..pos])?;
            frac_sep = next_if(&mut pos, |c| matches!(c, ',' | '_'));
        }
        let mut kind = chars.get(pos).copied().filter(|c| "eEfFgGn%".contains(*c));
        if kind.is_some() {
            pos += 1;
        }
        // Zero padding excludes an explicit fill or alignment.
        if pos != chars.len() || (zeropad && (fill.is_some() || align.is_some())) {
            return Err(invalid_format());
        }

        // A precision of 0 makes no sense for `g`, `n` or no type: use 1.
        if precision == Some(0) && matches!(kind, None | Some('g' | 'G' | 'n')) {
            precision = Some(1);
        }
        let group_len = if kind == Some('n') { None } else { Some(3) };
        if group_len.is_none() {
            if thousands_sep.is_some() {
                return Err(invalid_format());
            }
            kind = Some('g');
        }
        Ok(Self {
            fill: fill.unwrap_or(' '),
            align: align.unwrap_or('>'),
            sign,
            no_neg_0,
            alt,
            zeropad,
            minimumwidth,
            thousands_sep: separator(thousands_sep),
            group_len,
            frac_sep: separator(frac_sep),
            precision,
            kind,
        })
    }
}

/// The separator string for a parsed `,` / `_`.
fn separator(c: Option<char>) -> &'static str {
    match c {
        Some(',') => ",",
        Some('_') => "_",
        _ => "",
    }
}

/// Parses a width or precision; C `_decimal` rejects values beyond `Py_ssize_t`.
fn parse_count(digits: &[char]) -> RunResult<Option<i128>> {
    if digits.is_empty() {
        return Ok(None);
    }
    let text: String = digits.iter().collect();
    let value = text.parse::<isize>().ok().and_then(|n| i64::try_from(n).ok());
    match value {
        Some(n) => Ok(Some(i128::from(n))),
        None => Err(ExcType::overflow_error("cannot fit 'int' into an index-sized integer")),
    }
}

/// C `_decimal`'s error for any malformed spec.
fn invalid_format() -> RunError {
    ExcType::value_error("invalid format string")
}

/// `_format_sign`.
fn format_sign(negative: bool, spec: &FormatSpec) -> &'static str {
    match (negative, spec.sign) {
        (true, _) => "-",
        (false, '+') => "+",
        (false, ' ') => " ",
        _ => "",
    }
}

/// `_format_number`: inserts separators, appends the exponent and `%`, zero-pads
/// and aligns.
fn format_number(
    negative: bool,
    intpart: &str,
    fracpart: &str,
    exp: i128,
    kind: char,
    spec: &FormatSpec,
    ctx: &Ctx<'_>,
) -> RunResult<String> {
    let sign = format_sign(negative, spec);
    let mut frac = StringBuilder::new(ctx.tracker);
    if !fracpart.is_empty() || spec.alt {
        frac.push('.')?;
    }
    if spec.frac_sep.is_empty() {
        frac.push_str(fracpart)?;
    } else {
        for (i, chunk) in fracpart.as_bytes().chunks(3).enumerate() {
            if i > 0 {
                frac.push_str(spec.frac_sep)?;
            }
            frac.push_str(str::from_utf8(chunk).expect("digits are ASCII"))?;
        }
    }
    if exp != 0 || matches!(kind, 'e' | 'E') {
        frac.push(if matches!(kind, 'E' | 'G') { 'E' } else { 'e' })?;
        if exp >= 0 {
            frac.push('+')?;
        }
        frac.push_str(&exp.to_string())?;
    }
    if kind == '%' {
        frac.push('%')?;
    }
    let frac = frac.finish_raw()?;

    let min_width = if spec.zeropad {
        spec.minimumwidth - len_i128(&frac) - len_i128(sign)
    } else {
        0
    };
    let mut body = insert_thousands_sep(intpart, spec, min_width, ctx)?;
    body.push_str(&frac);
    format_align(sign, &body, spec, ctx)
}

/// `_insert_thousands_sep`: groups the integer digits, zero-padding them to at
/// least `min_width` characters (adding one more `0` rather than lead with a separator).
///
/// Only the leftmost group can be shorter than the grouping length, so the group
/// count is computed directly instead of iterating a width-sized loop.
fn insert_thousands_sep(digits: &str, spec: &FormatSpec, min_width: i128, ctx: &Ctx<'_>) -> RunResult<String> {
    let n = len_i128(digits);
    // `(group length, leftmost group length, groups before the leftmost)`.
    let (group, last, full) = if let Some(group) = spec.group_len {
        let step = group + len_i128(spec.thousands_sep);
        let by_digits = if n > group { ceil_div(n - group, group) } else { 0 };
        let by_width = if min_width > group {
            ceil_div(min_width - group, step)
        } else {
            0
        };
        let full = by_digits.max(by_width);
        let remaining = (n - full * group).max(0);
        let width_left = min_width - full * step;
        (group, remaining.max(width_left).max(1), full)
    } else {
        // The C locale's grouping is empty: one group, zero-padded to the width.
        (1, n.max(min_width).max(1), 0)
    };
    let padded_len = full * group + last;
    let separators = if spec.thousands_sep.is_empty() { 0 } else { full };
    let total = padded_len + separators * len_i128(spec.thousands_sep);
    ctx.check_digits(u64::try_from(total).unwrap_or(u64::MAX))?;
    let mut out = StringBuilder::new(ctx.tracker);
    let pad = usize::try_from(padded_len - n).map_err(|_| capacity_error(ctx))?;
    // Every group after the leftmost has `group` digits: a separator precedes
    // each position `last + k * group` of the padded digit string.
    let last = usize::try_from(last).map_err(|_| capacity_error(ctx))?;
    let group = usize::try_from(group).expect("small group length");
    let padded = (0..pad).map(|_| '0').chain(digits.chars());
    for (i, c) in padded.enumerate() {
        if i >= last && (i - last) % group == 0 && !spec.thousands_sep.is_empty() {
            ctx.tracker.check_time_every(i)?;
            out.push_str(spec.thousands_sep)?;
        }
        out.push(c)?;
    }
    out.finish_raw()
}

/// `_format_align`: pads `sign + body` to the minimum width with the fill character.
fn format_align(sign: &str, body: &str, spec: &FormatSpec, ctx: &Ctx<'_>) -> RunResult<String> {
    let used = len_i128(sign) + i128::try_from(body.chars().count()).expect("length fits");
    let padding = (spec.minimumwidth - used).max(0);
    let padding = usize::try_from(padding).map_err(|_| capacity_error(ctx))?;
    let mut out = StringBuilder::new(ctx.tracker);
    match spec.align {
        '<' => {
            out.push_str(sign)?;
            out.push_str(body)?;
            out.push_repeated(spec.fill, padding)?;
        }
        '=' => {
            out.push_str(sign)?;
            out.push_repeated(spec.fill, padding)?;
            out.push_str(body)?;
        }
        '^' => {
            let half = padding / 2;
            out.push_repeated(spec.fill, half)?;
            out.push_str(sign)?;
            out.push_str(body)?;
            out.push_repeated(spec.fill, padding - half)?;
        }
        _ => {
            out.push_repeated(spec.fill, padding)?;
            out.push_str(sign)?;
            out.push_str(body)?;
        }
    }
    out.finish_raw()
}

/// A string of `count` zeros, built through the tracker.
fn zeros(count: i128, ctx: &Ctx<'_>) -> RunResult<String> {
    let count = usize::try_from(count.max(0)).map_err(|_| capacity_error(ctx))?;
    ctx.check_digits(u64::try_from(count).unwrap_or(u64::MAX))?;
    let mut out = StringBuilder::new(ctx.tracker);
    out.push_repeated('0', count)?;
    out.finish_raw()
}

/// A zero-padding length, failing with `MemoryError` for one no coefficient could hold.
fn pad_count(count: i128, ctx: &Ctx<'_>) -> RunResult<u64> {
    u64::try_from(count.max(0)).map_err(|_| capacity_error(ctx))
}

/// The `MemoryError` for a result longer than any string can be.
fn capacity_error(ctx: &Ctx<'_>) -> RunError {
    match ctx.tracker.check_allocation(usize::MAX) {
        Err(err) => err.into(),
        Ok(()) => SimpleException::new_none(ExcType::MemoryError).into(),
    }
}

/// A string's byte length as `i128`.
fn len_i128(s: &str) -> i128 {
    i128::try_from(s.len()).expect("length fits in i128")
}

/// `ceil(a / b)` for positive operands.
fn ceil_div(a: i128, b: i128) -> i128 {
    (a + b - 1) / b
}
