//! Python `complex` implementation.
//!
//! A complex number is two `f64`s, too wide for an immediate `Value`, so it lives on the
//! heap as a leaf `HeapData::Complex` entry (no references, never GC-tracked). Arithmetic
//! follows CPython's `complexobject.c`: the C99 Annex G recovery of infinities in products
//! and quotients, the small-integer power fast path, and the 3.14 mixed-mode rules, under
//! which a real operand only touches the part it combines with (`1j * inf` is `nan+infj`
//! but `(1+1j) * inf` is `inf+infj`).

use std::fmt::Write;

use monty_types::FormatComplex;

use crate::{
    args::{ArgValues, FromArgs, FromValue, FromValueFail},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, hash_f64},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    types::{
        LazyHeapSet, PyTrait, Type,
        str::{StringRepr, allocate_string},
    },
    value::{EitherStr, Value, eq_f64},
};

/// A Python `complex`: real and imaginary IEEE doubles.
///
/// Plain data with no heap references; every operation returns a fresh value
/// because the type is immutable.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Complex {
    /// The real part.
    #[serde(rename = "R")]
    pub real: f64,
    /// The imaginary part.
    #[serde(rename = "I")]
    pub imag: f64,
}

/// The multiplicative identity, `1+0j`.
const ONE: Complex = Complex::new(1.0, 0.0);

/// Exponents with an integral real part up to this magnitude use repeated
/// squaring (CPython's `c_powi`), which is exact for small powers.
const POWI_MAX_EXPONENT: f64 = 100.0;

impl Complex {
    /// Builds a complex from its parts.
    #[must_use]
    pub const fn new(real: f64, imag: f64) -> Self {
        Self { real, imag }
    }

    /// Allocates this complex on the heap, handing back the owning value.
    pub(crate) fn into_value(self, heap: &Heap) -> Value {
        Value::Ref(heap.allocate(HeapData::Complex(self)))
    }

    /// `-z`.
    fn neg(self) -> Self {
        Self::new(-self.real, -self.imag)
    }

    /// `z.conjugate()`.
    fn conjugate(self) -> Self {
        Self::new(self.real, -self.imag)
    }

    /// Whether both parts are zero, ignoring the sign of zero.
    fn is_zero(self) -> bool {
        self.real == 0.0 && self.imag == 0.0
    }

    /// Python `==` between two complexes: part-wise IEEE equality, so `nan`
    /// is unequal to itself and `-0.0` equals `0.0`.
    fn eq(self, other: Self) -> bool {
        self.real == other.real && self.imag == other.imag
    }

    /// `abs(z)`, CPython's `_Py_c_abs`: `inf` if either part is infinite (even
    /// alongside `nan`), `nan` if either is `nan`, else `hypot`, which raises
    /// `OverflowError` when finite parts overflow.
    pub(crate) fn abs(self) -> RunResult<f64> {
        if self.real.is_infinite() || self.imag.is_infinite() {
            Ok(f64::INFINITY)
        } else if self.real.is_nan() || self.imag.is_nan() {
            Ok(f64::NAN)
        } else {
            let result = self.real.hypot(self.imag);
            if result.is_infinite() {
                Err(ExcType::overflow_error("absolute value too large"))
            } else {
                Ok(result)
            }
        }
    }

    /// `z * w`, CPython's `_Py_c_prod`, recovering an infinite result that the
    /// naive formula turns into `nan+nanj` (C11 Annex G.5.1).
    fn mul(self, other: Self) -> Self {
        let (mut a, mut b, mut c, mut d) = (self.real, self.imag, other.real, other.imag);
        let (ac, bd, ad, bc) = (a * c, b * d, a * d, b * c);
        let result = Self::new(ac - bd, ad + bc);
        if !(result.real.is_nan() && result.imag.is_nan()) {
            return result;
        }
        let mut recalc = false;
        if a.is_infinite() || b.is_infinite() {
            // Box the infinity and zero the nans in the other factor.
            a = boxed_infinity(a);
            b = boxed_infinity(b);
            c = nan_to_zero(c);
            d = nan_to_zero(d);
            recalc = true;
        }
        if c.is_infinite() || d.is_infinite() {
            c = boxed_infinity(c);
            d = boxed_infinity(d);
            a = nan_to_zero(a);
            b = nan_to_zero(b);
            recalc = true;
        }
        if !recalc && (ac.is_infinite() || bd.is_infinite() || ad.is_infinite() || bc.is_infinite()) {
            // An intermediate product overflowed: drop the nans and rescale.
            a = nan_to_zero(a);
            b = nan_to_zero(b);
            c = nan_to_zero(c);
            d = nan_to_zero(d);
            recalc = true;
        }
        if recalc {
            Self::new(f64::INFINITY * (a * c - b * d), f64::INFINITY * (a * d + b * c))
        } else {
            result
        }
    }

    /// `z / w`, CPython's `_Py_c_quot`: `None` for a zero divisor (C's `EDOM`),
    /// otherwise Smith's scaled division with the Annex G.5.2 recovery of
    /// infinities and zeros.
    fn div(self, den: Self) -> Option<Self> {
        let num = self;
        let abs_breal = den.real.abs();
        let abs_bimag = den.imag.abs();
        let mut result = if abs_breal >= abs_bimag {
            if abs_breal == 0.0 {
                return None;
            }
            let ratio = den.imag / den.real;
            let denom = den.real + den.imag * ratio;
            Self::new(
                (num.real + num.imag * ratio) / denom,
                (num.imag - num.real * ratio) / denom,
            )
        } else if abs_bimag >= abs_breal {
            let ratio = den.real / den.imag;
            let denom = den.real * ratio + den.imag;
            Self::new(
                (num.real * ratio + num.imag) / denom,
                (num.imag * ratio - num.real) / denom,
            )
        } else {
            // At least one part of the divisor is nan.
            Self::new(f64::NAN, f64::NAN)
        };
        if result.real.is_nan() && result.imag.is_nan() {
            if (num.real.is_infinite() || num.imag.is_infinite()) && den.real.is_finite() && den.imag.is_finite() {
                let x = boxed_infinity(num.real);
                let y = boxed_infinity(num.imag);
                result = Self::new(
                    f64::INFINITY * (x * den.real + y * den.imag),
                    f64::INFINITY * (y * den.real - x * den.imag),
                );
            } else if (abs_breal.is_infinite() || abs_bimag.is_infinite())
                && num.real.is_finite()
                && num.imag.is_finite()
            {
                let x = boxed_infinity(den.real);
                let y = boxed_infinity(den.imag);
                result = Self::new(0.0 * (num.real * x + num.imag * y), 0.0 * (num.imag * x - num.real * y));
            }
        }
        Some(result)
    }

    /// `r / z` for a real dividend, CPython 3.14's `_Py_rc_quot`: the scaled
    /// division of [`div`](Self::div) with the dividend's zero imaginary part
    /// never entering the arithmetic, which changes the sign of zero results.
    fn div_into(real: f64, den: Self) -> Option<Self> {
        let abs_breal = den.real.abs();
        let abs_bimag = den.imag.abs();
        let mut result = if abs_breal >= abs_bimag {
            if abs_breal == 0.0 {
                return None;
            }
            let ratio = den.imag / den.real;
            let denom = den.real + den.imag * ratio;
            Self::new(real / denom, (-real * ratio) / denom)
        } else if abs_bimag >= abs_breal {
            let ratio = den.real / den.imag;
            let denom = den.real * ratio + den.imag;
            Self::new((real * ratio) / denom, -real / denom)
        } else {
            // At least one part of the divisor is nan.
            Self::new(f64::NAN, f64::NAN)
        };
        // Unlike the complex quotient, only an infinite divisor is recovered;
        // an infinite real dividend stays `nan+nanj`.
        if result.real.is_nan()
            && result.imag.is_nan()
            && (abs_breal.is_infinite() || abs_bimag.is_infinite())
            && real.is_finite()
        {
            let x = boxed_infinity(den.real);
            let y = boxed_infinity(den.imag);
            result = Self::new(0.0 * (real * x), 0.0 * (-real * y));
        }
        Some(result)
    }

    /// `z ** w`, CPython's `complex_pow`: a small integral exponent uses exact
    /// repeated squaring, anything else the polar form. A zero base with a
    /// negative or non-real exponent raises `ZeroDivisionError`; an infinite
    /// component in the result (C's `ERANGE`) raises `OverflowError`.
    pub(crate) fn pow(self, exp: Self) -> RunResult<Self> {
        let result = if exp.imag == 0.0 && exp.real.fract() == 0.0 && exp.real.abs() <= POWI_MAX_EXPONENT {
            // The bound keeps the cast exact.
            #[expect(clippy::cast_possible_truncation)]
            let n = exp.real as i64;
            self.powi(n)
        } else {
            self.pow_general(exp)
        }
        .ok_or_else(ExcType::zero_division_complex_power)?;
        if result.real.is_infinite() || result.imag.is_infinite() {
            Err(ExcType::overflow_error("complex exponentiation"))
        } else {
            Ok(result)
        }
    }

    /// `c_powi`: `c_powu` for a non-negative exponent, its reciprocal otherwise
    /// (`None` when that reciprocal divides by zero).
    fn powi(self, n: i64) -> Option<Self> {
        if n >= 0 {
            Some(self.powu(n.unsigned_abs()))
        } else {
            ONE.div(self.powu(n.unsigned_abs()))
        }
    }

    /// `c_powu`: binary exponentiation by repeated squaring.
    fn powu(self, n: u64) -> Self {
        let mut r = ONE;
        let mut p = self;
        let mut mask = 1u64;
        while mask > 0 && n >= mask {
            if n & mask != 0 {
                r = r.mul(p);
            }
            mask <<= 1;
            p = p.mul(p);
        }
        r
    }

    /// `_Py_c_pow`: the polar-form power, `None` for a zero base with a
    /// negative or complex exponent.
    fn pow_general(self, exp: Self) -> Option<Self> {
        if exp.is_zero() {
            Some(ONE)
        } else if self.is_zero() {
            if exp.imag != 0.0 || exp.real < 0.0 {
                None
            } else {
                Some(Self::new(0.0, 0.0))
            }
        } else {
            let vabs = self.real.hypot(self.imag);
            let mut len = vabs.powf(exp.real);
            let at = self.imag.atan2(self.real);
            let mut phase = at * exp.real;
            if exp.imag != 0.0 {
                len *= (-at * exp.imag).exp();
                phase += exp.imag * vabs.ln();
            }
            Some(Self::new(len * phase.cos(), len * phase.sin()))
        }
    }
}

/// Replaces an infinity by a unit of the same sign and anything else by a
/// signed zero — the "boxing" step of the Annex G recovery formulas.
fn boxed_infinity(value: f64) -> f64 {
    let unit: f64 = if value.is_infinite() { 1.0 } else { 0.0 };
    unit.copysign(value)
}

/// Replaces `nan` by a signed zero so it drops out of a recovery product.
fn nan_to_zero(value: f64) -> f64 {
    if value.is_nan() { 0.0f64.copysign(value) } else { value }
}

/// The other operand of a complex arithmetic operation.
///
/// CPython 3.14's mixed-mode rules treat a real operand differently from
/// `complex(x, 0)`, so the two are kept apart rather than widened up front.
#[derive(Clone, Copy)]
enum Operand {
    Real(f64),
    Complex(Complex),
}

/// Classifies an operand, widening ints (an oversized big int raises
/// `OverflowError`); `None` for a non-number so the operation reports
/// `NotImplemented` and the reflected form gets its turn.
fn operand(value: &Value, vm: &VM<'_>) -> RunResult<Option<Operand>> {
    Ok(match value {
        Value::Float(f) => Some(Operand::Real(*f)),
        Value::Int(i) => Some(Operand::Real(*i as f64)),
        Value::Bool(b) => Some(Operand::Real(f64::from(*b))),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Complex(c) => Some(Operand::Complex(*c)),
            HeapData::LongInt(li) => Some(Operand::Real(li.to_f64_checked()?)),
            _ => None,
        },
        _ => None,
    })
}

/// Widens an operand to a complex for the operations with no mixed-mode form (`**`).
fn widen(operand: Operand) -> Complex {
    match operand {
        Operand::Real(r) => Complex::new(r, 0.0),
        Operand::Complex(c) => c,
    }
}

/// Implements the `complex()` constructor.
///
/// A lone positional argument may be a string to parse, a number, or a
/// complex (returned as is); otherwise `real` and `imag` are each a number or
/// a complex, combined as `real + imag*1j`.
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    if let ArgValues::One(value) = args {
        return init_single(value, vm);
    }
    let ComplexArgs { real, imag } = ComplexArgs::from_args(args, vm)?;
    let real = real.unwrap_or(Part::Real(0.0));
    let mut result = Complex::new(real.real(), 0.0);
    match imag {
        // `complex(z)` with a complex `real` and no `imag` keeps `z.imag`.
        None => {
            if let Part::Complex(r) = real {
                result.imag = r.imag;
            }
        }
        // Otherwise the parts of a complex `real` and `imag` combine as `real + imag*1j`.
        Some(imag) => {
            result.imag = imag.real();
            if let Part::Complex(i) = imag {
                result.real -= i.imag;
            }
            if let Part::Complex(r) = real {
                result.imag += r.imag;
            }
        }
    }
    Ok(result.into_value(vm.heap))
}

/// `complex(x)`: the one form that also accepts a string.
fn init_single(value: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    if let Value::Ref(id) = &value
        && matches!(vm.heap.get(*id), HeapData::Complex(_))
    {
        return Ok(value);
    }
    defer_drop!(value, vm);
    let parsed = match value.as_either_str(vm.heap) {
        Some(text) => Some(parse(text.as_str(vm.interns))?),
        None => operand(value, vm)?.map(widen),
    };
    match parsed {
        Some(c) => Ok(c.into_value(vm.heap)),
        None => Err(ExcType::type_error(format!(
            "complex() argument must be a string or a number, not {}",
            value.py_type_name(vm)
        ))),
    }
}

/// Argument shape for `complex(real=0, imag=0)`; a string is only accepted
/// through the single-positional form handled by [`init`].
#[derive(FromArgs)]
#[from_args(name = "complex", at_most_total, bad_arg_named)]
struct ComplexArgs {
    #[from_args(default)]
    real: Option<Part>,
    #[from_args(default)]
    imag: Option<Part>,
}

/// One `complex(...)` constructor argument: a real number or a complex.
#[derive(Clone, Copy)]
enum Part {
    Real(f64),
    Complex(Complex),
}

impl Part {
    /// The real component of the part.
    fn real(self) -> f64 {
        match self {
            Self::Real(r) => r,
            Self::Complex(c) => c.real,
        }
    }
}

impl FromValue for Part {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("a real number");

    fn from_value(value: Value, vm: &mut VM<'_>) -> Result<Self, FromValueFail> {
        let result = match operand(&value, vm) {
            Ok(Some(Operand::Real(r))) => Ok(Self::Real(r)),
            Ok(Some(Operand::Complex(c))) => Ok(Self::Complex(c)),
            Ok(None) => Err(FromValueFail::WrongType),
            Err(err) => Err(FromValueFail::Raise(err)),
        };
        value.drop_with(vm);
        result
    }
}

/// `complex.from_number(x)`: a number or complex, never a string.
pub(crate) fn class_from_number(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("complex.from_number", vm.heap)?;
    if let Value::Ref(id) = &value
        && matches!(vm.heap.get(*id), HeapData::Complex(_))
    {
        return Ok(value);
    }
    defer_drop!(value, vm);
    match operand(value, vm)? {
        Some(op) => Ok(widen(op).into_value(vm.heap)),
        None => Err(ExcType::type_error(format!(
            "must be real number, not {}",
            value.py_type_name(vm)
        ))),
    }
}

/// Parses the string form CPython's `complex()` accepts: `<float>`,
/// `<float>j`, `<float><signed-float>j`, plus the legacy `<float><sign>j`,
/// `<sign>j` and `j`, optionally parenthesised and whitespace-padded.
///
/// Underscores are validated as digit separators over the whole text first,
/// which is why a misplaced one gets a different message from other errors.
fn parse(text: &str) -> RunResult<Complex> {
    let malformed = || ExcType::value_error("complex() arg is a malformed string");
    let stripped;
    let text = if text.contains('_') {
        stripped = strip_underscores(text).ok_or_else(|| {
            ExcType::value_error(format!("could not convert string to complex: {}", StringRepr(text)))
        })?;
        stripped.as_str()
    } else {
        text
    };
    let mut s = text.trim_start();
    let bracketed = s.starts_with('(');
    if bracketed {
        s = s[1..].trim_start();
    }
    let (mut real, mut imag) = (0.0, 0.0);
    if let Some((z, len)) = float_prefix(s) {
        s = &s[len..];
        if s.starts_with(['+', '-']) {
            real = z;
            if let Some((y, len)) = float_prefix(s) {
                imag = y;
                s = &s[len..];
            } else {
                imag = if s.starts_with('+') { 1.0 } else { -1.0 };
                s = &s[1..];
            }
            s = s.strip_prefix(['j', 'J']).ok_or_else(malformed)?;
        } else if let Some(rest) = s.strip_prefix(['j', 'J']) {
            imag = z;
            s = rest;
        } else {
            real = z;
        }
    } else {
        // Not starting with a float: `<sign>j` or `j`.
        imag = 1.0;
        if let Some(rest) = s.strip_prefix(['+', '-']) {
            imag = if s.starts_with('+') { 1.0 } else { -1.0 };
            s = rest;
        }
        s = s.strip_prefix(['j', 'J']).ok_or_else(malformed)?;
    }
    s = s.trim_start();
    if bracketed {
        s = s.strip_prefix(')').ok_or_else(malformed)?.trim_start();
    }
    if s.is_empty() {
        Ok(Complex::new(real, imag))
    } else {
        Err(malformed())
    }
}

/// Removes digit-separator underscores, or returns `None` when one is not
/// strictly between two digits (CPython's `_Py_string_to_number_with_underscores`).
fn strip_underscores(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        if c == '_' {
            let before = i.checked_sub(1).map(|j| bytes[j]);
            let after = bytes.get(i + 1);
            if !(before.is_some_and(|b| b.is_ascii_digit()) && after.is_some_and(u8::is_ascii_digit)) {
                return None;
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// The longest float literal at the start of `s`, as CPython's
/// `PyOS_string_to_double` consumes it: an optional sign, then `inf`,
/// `infinity`, `nan` (any case) or decimal digits with an optional fraction
/// and exponent. Returns the value and the byte length consumed.
fn float_prefix(s: &str) -> Option<(f64, usize)> {
    let bytes = s.as_bytes();
    let mut i = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
    let rest = &s[i..];
    let word_len = ["infinity", "inf", "nan"].iter().find_map(|word| {
        rest.get(..word.len())
            .filter(|p| p.eq_ignore_ascii_case(word))
            .map(|_| word.len())
    });
    if let Some(len) = word_len {
        i += len;
    } else {
        let digits = |i: usize| bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        let int_digits = digits(i);
        i += int_digits;
        let mut frac_digits = 0;
        if bytes.get(i) == Some(&b'.') {
            frac_digits = digits(i + 1);
            i += 1 + frac_digits;
        }
        if int_digits + frac_digits == 0 {
            return None;
        }
        if matches!(bytes.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(bytes.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            let exp_digits = digits(j);
            // An exponent marker with no digits is not part of the number.
            if exp_digits > 0 {
                i = j + exp_digits;
            }
        }
    }
    s[..i].parse().ok().map(|value| (value, i))
}

impl HeapItem for Complex {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Complex> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Complex
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    /// Equal to another complex part-wise, and to a real number when the
    /// imaginary part is zero and the real part matches it exactly.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let c = *self.get(vm.heap);
        Ok(match other {
            Value::Ref(id) if let HeapData::Complex(o) = vm.heap.get(*id) => Some(c.eq(*o)),
            _ => eq_f64(c.real, other, vm).map(|real_eq| real_eq && c.imag == 0.0),
        })
    }

    /// Combines the part hashes as CPython does, so a complex with a zero
    /// imaginary part hashes like its real part (and so like an equal int).
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let c = *self.get(vm.heap);
        let combined = hash_f64(c.real)
            .raw()
            .wrapping_add(hash_f64(c.imag).raw().wrapping_mul(1_000_003));
        Ok(Some(HashValue::new(combined)))
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(!self.get(vm.heap).is_zero())
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let c = self.get(vm.heap);
        Ok(write!(
            f,
            "{}",
            FormatComplex {
                real: c.real,
                imag: c.imag
            }
        )?)
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let c = self.get(vm.heap);
        let text = FormatComplex {
            real: c.real,
            imag: c.imag,
        }
        .to_string();
        Ok(allocate_string(text, vm.heap))
    }

    fn py_neg_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(Some(self.get(vm.heap).neg().into_value(vm.heap)))
    }

    fn py_pos_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(Some(self.clone_value(vm.heap)))
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let result = match operand(other, vm)? {
            Some(Operand::Real(r)) => Complex::new(c.real + r, c.imag),
            Some(Operand::Complex(o)) => Complex::new(c.real + o.real, c.imag + o.imag),
            None => return Ok(None),
        };
        Ok(Some(result.into_value(vm.heap)))
    }

    fn py_radd_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        // Addition is symmetric part-wise, so the reflected form is the direct one.
        self.py_add_impl(other, vm)
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let result = match operand(other, vm)? {
            Some(Operand::Real(r)) => Complex::new(c.real - r, c.imag),
            Some(Operand::Complex(o)) => Complex::new(c.real - o.real, c.imag - o.imag),
            None => return Ok(None),
        };
        Ok(Some(result.into_value(vm.heap)))
    }

    fn py_rsub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let result = match operand(other, vm)? {
            Some(Operand::Real(r)) => Complex::new(r - c.real, -c.imag),
            Some(Operand::Complex(o)) => Complex::new(o.real - c.real, o.imag - c.imag),
            None => return Ok(None),
        };
        Ok(Some(result.into_value(vm.heap)))
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let result = match operand(other, vm)? {
            Some(Operand::Real(r)) => Complex::new(c.real * r, c.imag * r),
            Some(Operand::Complex(o)) => c.mul(o),
            None => return Ok(None),
        };
        Ok(Some(result.into_value(vm.heap)))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        // Scaling by a real is symmetric part-wise, so the reflected form is the direct one.
        self.py_mul_impl(other, vm)
    }

    fn py_truediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let result = match operand(other, vm)? {
            Some(Operand::Real(r)) => {
                if r == 0.0 {
                    None
                } else {
                    Some(Complex::new(c.real / r, c.imag / r))
                }
            }
            Some(Operand::Complex(o)) => c.div(o),
            None => return Ok(None),
        };
        result
            .map(|r| r.into_value(vm.heap))
            .ok_or_else(|| ExcType::zero_division().into())
            .map(Some)
    }

    fn py_rtruediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let quotient = match operand(other, vm)? {
            Some(Operand::Real(r)) => Complex::div_into(r, c),
            Some(Operand::Complex(o)) => o.div(c),
            None => return Ok(None),
        };
        quotient
            .map(|r| r.into_value(vm.heap))
            .ok_or_else(|| ExcType::zero_division().into())
            .map(Some)
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let Some(exp) = operand(other, vm)? else {
            return Ok(None);
        };
        if modulus.is_some() {
            return Err(ExcType::value_error_complex_modulo());
        }
        Ok(Some(c.pow(widen(exp))?.into_value(vm.heap)))
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let c = *self.get(vm.heap);
        let Some(base) = operand(other, vm)? else {
            return Ok(None);
        };
        if modulus.is_some() {
            return Err(ExcType::value_error_complex_modulo());
        }
        Ok(Some(widen(base).pow(c)?.into_value(vm.heap)))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        if attr.static_string(vm.interns) == Some(StaticStrings::Conjugate) {
            let c = *self.get(vm.heap);
            args.check_zero_args("complex.conjugate", vm.heap)?;
            return Ok(CallResult::Value(c.conjugate().into_value(vm.heap)));
        }
        Err(ExcType::attribute_error_method(Type::Complex, attr, args, vm))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let c = *self.get(vm.heap);
        Ok(match attr.static_string(vm.interns) {
            Some(StaticStrings::Real) => Some(CallResult::Value(Value::Float(c.real))),
            Some(StaticStrings::Imag) => Some(CallResult::Value(Value::Float(c.imag))),
            _ => None,
        })
    }

    /// `real` and `imag` exist but are read-only, which CPython words differently
    /// from a missing attribute.
    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        value.drop_with(vm);
        Err(match name.static_string(vm.interns) {
            Some(StaticStrings::Real | StaticStrings::Imag) => ExcType::attribute_error_readonly(),
            _ => ExcType::attribute_error_no_setattr(&self.py_type_name(vm), name.as_str(vm.interns)),
        })
    }
}
