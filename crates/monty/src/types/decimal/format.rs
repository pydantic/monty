//! `Decimal` string rendering: [`write_decimal`], the GDA *to-scientific-
//! string* conversion behind `str`/`repr`/snapshots/the host boundary, and
//! [`format_decimal`], the `Decimal.__format__` port driving f-strings and
//! `format()`.
//!
//! Both render from the value's **native digit string** (never an `f64`
//! round-trip, never `normalize`d), so significant trailing zeros and
//! exponent identity survive exactly as CPython requires
//! (`f'{Decimal("1.10"):.20f}' == '1.10000000000000000000'`,
//! `str(Decimal('1E+5')) == '1E+5'`).

use std::fmt;

use super::{Decimal, EMAX, PREC, RoundMode, Special, canonical_string, fix};
use crate::{
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    fstring::{ParsedFormatSpec, TypeChar, numeric_sign, pad_signed_numeric},
    resource::{ResourceError, ResourceTracker},
    string_builder::StringBuilder,
};

/// Writes the Python canonical string form of `d` into `f` — the
/// *to-scientific-string* conversion from the General Decimal Arithmetic
/// specification, exactly as CPython's `Decimal.__str__` implements it.
///
/// This is the single source of truth for `str`, `repr` (wrapped in
/// `Decimal('…')`), the snapshot / host-boundary canonical string, and the
/// empty-format-spec fast path. `capitals` selects the exponent marker `E`
/// (true — CPython's default `capitals=1`) vs `e`.
///
/// Specials use CPython's exact spellings, sign included: `Infinity` /
/// `-Infinity`, `NaN` / `-NaN`, `sNaN` / `-sNaN`, with a NaN payload's digits
/// appended (`sNaN123`) and an empty payload printing none (`Decimal('NaN0')`
/// is just `NaN` — the parser strips a zero payload to empty).
pub(crate) fn write_decimal(d: &Decimal, capitals: bool, f: &mut impl fmt::Write) -> fmt::Result {
    let sign = if d.sign == 1 { "-" } else { "" };
    match d.special {
        Special::Inf => write!(f, "{sign}Infinity"),
        Special::Qnan | Special::Snan => {
            let word = if d.special == Special::Snan { "sNaN" } else { "NaN" };
            write!(f, "{sign}{word}")?;
            // A zero coefficient means "no payload": bare `NaN`, never `NaN0`.
            if !d.coeff_is_zero() {
                write!(f, "{}", d.coeff)?;
            }
            Ok(())
        }
        Special::Finite => write_finite(d, sign, capitals, f),
    }
}

/// The finite branch of [`write_decimal`]: the coefficient digits placed
/// around the decimal point per the GDA `dotplace` rule, with an explicit
/// `E±exp` marker whenever the point moved. Every zero run synthesised here
/// is tiny by construction: fewer than 6 on the left (the `leftdigits > -6`
/// bound) and none on the right (plain notation requires `exp <= 0`).
fn write_finite(d: &Decimal, sign: &str, capitals: bool, f: &mut impl fmt::Write) -> fmt::Result {
    // Coefficient digits (`"0"` for a zero — the spec's coefficient string is
    // a single `"0"`) and the decimal exponent. Exponents are bounded by the
    // constructor's literal bounds (~±2e18) and digit counts by the sandbox
    // cap, so the i64 position arithmetic below cannot overflow.
    let int_str = d.coeff_str();
    let len_int = i64::try_from(int_str.len()).expect("digit count fits i64");
    // `leftdigits` is the position of the decimal point measured from the
    // start of the coefficient (CPython's `self._exp + len(self._int)`).
    let leftdigits = d.exp + len_int;
    // Plain notation when the exponent is non-positive and the point is not
    // too far left; scientific (one digit before the point) otherwise.
    let dotplace = if d.exp <= 0 && leftdigits > -6 { leftdigits } else { 1 };

    if dotplace <= 0 {
        // 0.<-dotplace zeros><coefficient>
        write!(f, "{sign}0.")?;
        for _ in 0..-dotplace {
            f.write_char('0')?;
        }
        f.write_str(&int_str)?;
    } else if dotplace >= len_int {
        // <coefficient><trailing zeros>, no fractional part
        write!(f, "{sign}{int_str}")?;
        for _ in 0..dotplace - len_int {
            f.write_char('0')?;
        }
    } else {
        // Split the coefficient around the point (ASCII digits, byte-
        // indexable); `dotplace` is in `1..len_int` here, so the conversion
        // never fails.
        let dp = usize::try_from(dotplace).expect("dotplace > 0 in this branch");
        write!(f, "{sign}{}.{}", &int_str[..dp], &int_str[dp..])?;
    }

    if leftdigits != dotplace {
        let marker = if capitals { 'E' } else { 'e' };
        write!(f, "{marker}{:+}", leftdigits - dotplace)?;
    }
    Ok(())
}

/// Formats a `Decimal` against a parsed format-spec, faithfully mirroring
/// CPython's `Decimal.__format__`: the value's native digit string drives the
/// output, so significant trailing zeros survive exactly as CPython requires
/// (`f'{Decimal("1.20"):g}' == '1.20'`). Sign placement, grouping, padding
/// and alignment are layered on by the shared [`pad_signed_numeric`].
///
/// The spec is validated up front — every combination CPython rejects (an
/// integer/string presentation code like `d`/`x`/`s`, or a grouping option
/// with `n`) raises the single [`ExcType::decimal_invalid_format_string`]
/// message, *before* the special-value shortcut, because CPython parses (and
/// rejects) the spec even for infinities and NaNs.
///
/// All rounding is `ROUND_HALF_EVEN`: CPython's `__format__` rounds under the
/// context's `rounding`, and Monty's fixed context pins it to the default
/// (`f'{Decimal("2.675"):.2f}'` is `'2.68'`).
pub(crate) fn format_decimal(
    d: &Decimal,
    spec: &ParsedFormatSpec,
    tracker: &impl ResourceTracker,
) -> RunResult<String> {
    let presentation = resolve_presentation(spec)?;

    // The sign applies to specials too: `format(Decimal('-NaN'), '10')` is
    // `'      -NaN'` (CPython's `_format_sign` runs before the special word).
    let is_negative = d.is_signed();
    let magnitude = d.copy_abs();

    if !magnitude.is_finite() {
        // Special values render as their canonical word; only `%` appends a
        // suffix. CPython ignores the `0` *flag* for specials —
        // `format(Decimal('inf'), '010')` is `'  Infinity'`, space-filled —
        // but honors an *explicit* fill/alignment (`0>10`, `0=10`, `*>10`).
        // The spec parser sets `zero_pad` only for the flag form (an explicit
        // align keeps it false), so when it is set the promoted `fill = '0'`
        // must be reset alongside it, or the default right-align path would
        // still pad with zeros.
        let mut word = canonical_string(&magnitude);
        if presentation.percent {
            word.push('%');
        }
        let mut special_spec = spec.clone();
        if special_spec.zero_pad {
            special_spec.zero_pad = false;
            special_spec.fill = ' ';
            special_spec.align = None;
        }
        let sign = numeric_sign(is_negative, &word, &special_spec);
        return Ok(pad_signed_numeric(sign, "", &word, &special_spec));
    }

    let abs_str = if spec.type_char.is_none() && spec.precision.is_none() && !spec.alternate {
        // An empty spec equals CPython's default `G` presentation, which is in
        // turn identical to the canonical (`str`) form — same trailing-zero
        // and exponent rules — so reuse it directly.
        canonical_string(&magnitude)
    } else {
        render_decimal_body(&magnitude, &presentation, spec, tracker)?
    };
    Ok(pad_signed_numeric(
        numeric_sign(is_negative, &abs_str, spec),
        "",
        &abs_str,
        spec,
    ))
}

/// Where the decimal point sits relative to the coefficient's digits, per
/// CPython's `Decimal.__format__` (`dotplace` logic). `Fixed` never shows an
/// exponent, `Scientific` always does, `General` chooses between them.
enum Placement {
    Fixed,
    Scientific,
    General,
}

/// A `Decimal` presentation resolved from a spec's type char: the point
/// placement, whether the exponent marker / default form is uppercase, and
/// whether the `%` suffix (and ×100 pre-scale) applies.
struct DecimalPresentation {
    placement: Placement,
    uppercase: bool,
    percent: bool,
}

/// Resolves a format spec to a [`DecimalPresentation`], rejecting every spec
/// CPython's `Decimal.__format__` refuses. An empty (type-less) spec defaults
/// to the uppercase `G` CPython uses when `capitals` is set (its default).
fn resolve_presentation(spec: &ParsedFormatSpec) -> RunResult<DecimalPresentation> {
    // `n` does its own locale grouping, so an explicit grouping option with
    // `n` is rejected; the integer/string codes have no `Decimal` formatter at
    // all.
    if spec.grouping.is_some() && spec.type_char == Some(TypeChar::N) {
        return Err(ExcType::decimal_invalid_format_string());
    }
    let (placement, uppercase, percent) = match spec.type_char {
        None => (Placement::General, true, false),
        // `f`/`F` differ only for the non-finite word, which is always cased
        // the same, so they share the finite renderer.
        Some(TypeChar::F | TypeChar::FUpper) => (Placement::Fixed, false, false),
        Some(TypeChar::E) => (Placement::Scientific, false, false),
        Some(TypeChar::EUpper) => (Placement::Scientific, true, false),
        Some(TypeChar::G | TypeChar::N) => (Placement::General, false, false),
        Some(TypeChar::GUpper) => (Placement::General, true, false),
        Some(TypeChar::Percent) => (Placement::Fixed, false, true),
        Some(_) => return Err(ExcType::decimal_invalid_format_string()),
    };
    Ok(DecimalPresentation {
        placement,
        uppercase,
        percent,
    })
}

/// Cap on the zeros the fixed-point presentations (`f`/`F`/`%`) synthesise
/// from a value's *exponent* (`format(Decimal('1E+999999'), 'f')` writes
/// 999999 trailing zeros into an untracked Rust `String`). Post-`fix` values
/// have `Etiny <= exp <= Emax`, so anything a fixed-context computation can
/// produce fits (`+ 2` covers the `%` pre-scale); only raw constructor
/// literals, whose exponents reach ~±2e18, exceed it. CPython attempts the
/// multi-exabyte string and dies with `MemoryError`; Monty raises
/// [`fixed_pad_limit_error`] before allocating (see `limitations/decimal.md`).
const FIXED_PAD_LIMIT: i64 = EMAX + PREC + 2;

/// The Monty-specific `ValueError` for a fixed-point rendering whose
/// exponent-driven zero padding exceeds [`FIXED_PAD_LIMIT`]. Only reachable
/// for unfixed constructor literals with `|exp| > Emax`-ish magnitudes.
fn fixed_pad_limit_error() -> RunError {
    SimpleException::new_msg(
        ExcType::ValueError,
        "decimal exponent out of range for fixed-point formatting".to_owned(),
    )
    .into()
}

/// Renders the unsigned body of a finite, non-negative `magnitude` for the
/// resolved `presentation`, following CPython's `__format__` /
/// `_format_number`: round per the type, place the decimal point
/// (`dotplace`), then split the coefficient digit string into integer /
/// fraction parts and emit `intpart[.fracpart][e±exp][%]`.
fn render_decimal_body(
    magnitude: &Decimal,
    presentation: &DecimalPresentation,
    spec: &ParsedFormatSpec,
    tracker: &impl ResourceTracker,
) -> RunResult<String> {
    let precision = spec.precision;
    // `%` multiplies by 100 by raising the exponent — coefficient preserved,
    // so `0.5 → 50`, not `50.0`. Literal exponents are ≤ ~2e18: no overflow.
    let value = if presentation.percent {
        Decimal {
            exp: magnitude.exp + 2,
            ..magnitude.clone()
        }
    } else {
        magnitude.clone()
    };

    // Round per presentation under the fixed context's ROUND_HALF_EVEN.
    // Rounding here only ever *drops* digits — zero-padding up to a requested
    // precision happens on the digit string below, so an attacker-chosen
    // precision can't inflate the value itself (CPython's `_round`/`_rescale`
    // pad the coefficient instead; the rendered output is identical, and the
    // string padding is bounded by the resource guard in `format_with_spec`).
    let rounded = match presentation.placement {
        // `e` with precision `p` keeps one digit before the point and `p`
        // after it: `p + 1` significant digits (CPython: `_round(precision+1)`).
        Placement::Scientific => match precision {
            Some(p) => round_significant(value, p.saturating_add(1))?,
            None => value,
        },
        Placement::Fixed => match precision {
            Some(p) => round_fraction(value, p)?,
            None => value,
        },
        // `g` with a precision rounds to that many significant digits (≥ 1 —
        // CPython converts a `.0` precision to 1 for the g family), but only
        // when the value actually has more; otherwise its own digits
        // (trailing zeros included) are kept.
        Placement::General => match precision {
            Some(p) => round_significant(value, p.max(1))?,
            None => value,
        },
    };

    // A zero with a positive exponent has no fixed-point form; CPython
    // rescales it to `0E0` for the fixed presentations, so
    // `format(Decimal('0'), '%')` is `'0%'`, not `'000%'`.
    let zero_pos_exp_fixed = rounded.is_zero() && matches!(presentation.placement, Placement::Fixed) && rounded.exp > 0;
    let (coeff, exp) = if zero_pos_exp_fixed {
        ("0".to_owned(), 0)
    } else {
        (rounded.coeff_str(), rounded.exp)
    };
    let coeff_len = i64::try_from(coeff.len()).expect("coefficient digit count fits i64");
    // CPython's `leftdigits = self._exp + len(self._int)`.
    let leftdigits = exp + coeff_len;

    let dotplace = match presentation.placement {
        // Scientific puts one digit before the point; a zero with an explicit
        // precision shifts the exponent by that precision (CPython:
        // `1 - precision`, so `format(Decimal('0'), '.3e')` is `0.000e+3`).
        Placement::Scientific if rounded.is_zero() => match precision {
            Some(p) => 1 - i64::try_from(p).unwrap_or(i64::MAX),
            None => 1,
        },
        Placement::Scientific => 1,
        Placement::Fixed => leftdigits,
        // General is fixed when the value has no positive exponent and isn't
        // tiny, else scientific — CPython's `self._exp <= 0 and leftdigits > -6`.
        Placement::General if exp <= 0 && leftdigits > -6 => leftdigits,
        Placement::General => 1,
    };

    // Only the fixed presentations can synthesise exponent-many zeros (the
    // general form is bounded by its `leftdigits > -6` / `exp <= 0` gate and
    // scientific pads at most `precision`, which the tracker already vetted);
    // cap them so a wild constructor literal can't OOM the host.
    if matches!(presentation.placement, Placement::Fixed)
        && (-dotplace > FIXED_PAD_LIMIT || dotplace - coeff_len > FIXED_PAD_LIMIT)
    {
        return Err(fixed_pad_limit_error());
    }

    let out_exp = leftdigits - dotplace;

    // Emit `intpart[.fracpart][e±exp][%]` directly into a tracker-reserved
    // builder (the `StringBuilder` rule): the exponent-driven zero pads can
    // reach `FIXED_PAD_LIMIT` (~1 MB) and the precision pad is
    // attacker-sized, so both must be charged as they grow rather than
    // assembled on the untracked Rust heap. The pads fit `usize` on every
    // target: exponent-driven pads are capped by `FIXED_PAD_LIMIT` above, and
    // precision-driven ones are bounded by the spec's `usize` precision.
    let mut builder = StringBuilder::new(tracker);

    // Integer part: the coefficient digits left of `dotplace` (`"0"` when the
    // point is at or before the first digit), zero-padded up to the point
    // when it falls beyond the last digit.
    let int_split = if dotplace <= 0 {
        0
    } else {
        usize::try_from(dotplace)
            .expect("positive dotplace fits usize")
            .min(coeff.len())
    };
    if int_split == 0 {
        builder.push('0')?;
    } else {
        builder.push_str(&coeff[..int_split])?;
    }
    if dotplace > coeff_len {
        push_zeros(
            &mut builder,
            usize::try_from(dotplace - coeff_len).expect("positive dotplace gap fits usize"),
        )?;
    }

    // Fraction: a leading zero pad when the point sits left of the first
    // digit, the remaining coefficient digits, then zero-fill up to an
    // explicit `f`/`e`/`%` precision (CPython's `_rescale`/`_round` guarantee
    // the count; `g` does not pad). The alternate form (`#`) keeps a trailing
    // point even with no fraction (`f'{Decimal("5"):#g}' == '5.'`).
    let lead_pad = usize::try_from(-dotplace.min(0)).expect("negative dotplace fits usize");
    let base_frac_len = lead_pad + (coeff.len() - int_split);
    let target_frac_len = match precision {
        Some(p) if matches!(presentation.placement, Placement::Fixed | Placement::Scientific) => base_frac_len.max(p),
        _ => base_frac_len,
    };
    if target_frac_len > 0 || spec.alternate {
        builder.push('.')?;
        push_zeros(&mut builder, lead_pad)?;
        builder.push_str(&coeff[int_split..])?;
        push_zeros(&mut builder, target_frac_len - base_frac_len)?;
    }

    // The exponent marker shows for scientific always, and for general only
    // when the point actually moved (CPython: `exp != 0 or type in 'eE'`).
    if out_exp != 0 || matches!(presentation.placement, Placement::Scientific) {
        builder.push(if presentation.uppercase { 'E' } else { 'e' })?;
        // CPython always shows an explicit exponent sign (`e+3`, `e-2`).
        builder.push(if out_exp < 0 { '-' } else { '+' })?;
        builder.push_str(&out_exp.unsigned_abs().to_string())?;
    }
    if presentation.percent {
        builder.push('%')?;
    }
    builder.finish_raw()
}

/// Appends `n` `'0'`s to a tracker-reserved builder.
fn push_zeros(builder: &mut StringBuilder<'_, impl ResourceTracker>, n: usize) -> Result<(), ResourceError> {
    for _ in 0..n {
        builder.push('0')?;
    }
    Ok(())
}

/// Rounds `value` to at most `places` significant digits (HALF_EVEN), leaving
/// it untouched when it already has no more — CPython's `_round` would *pad*
/// the coefficient up to `places` instead, which renders identically because
/// [`render_decimal_body`] pads the digit string to the requested precision
/// anyway (and coefficient padding could exceed the sandbox digit cap for an
/// attacker-sized precision). Callers guarantee `places >= 1`, as
/// `_pydecimal._round` requires.
fn round_significant(value: Decimal, places: usize) -> RunResult<Decimal> {
    if !value.is_zero() && value.digits() > places {
        fix::round_sig(&value, places, RoundMode::HalfEven)
    } else {
        Ok(value)
    }
}

/// Rounds `value` to at most `places` fractional digits (HALF_EVEN), never
/// padding up: `rescale` is only entered when digits are actually dropped
/// (`exp < -places` — it *would* pad the coefficient when the exponent
/// exceeds the target, which for a huge requested precision could blow the
/// sandbox digit cap).
fn round_fraction(value: Decimal, places: usize) -> RunResult<Decimal> {
    let target = -i64::try_from(places).unwrap_or(i64::MAX);
    if value.exp < target {
        fix::rescale(&value, target, RoundMode::HalfEven)
    } else {
        Ok(value)
    }
}
