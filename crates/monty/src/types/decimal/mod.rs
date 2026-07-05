//! Python `decimal.Decimal` — a Rust port of CPython's `_pydecimal.py`
//! numeric core, running under a **fixed context**.
//!
//! The representation mirrors `_pydecimal` field-for-field so the algorithms
//! port line-for-line: a value is `(-1)**sign · coeff · 10**exp`, with
//! specials tagged by [`Special`] (CPython tags them in `_exp` as
//! `'F'`/`'n'`/`'N'`). Every arithmetic result is finalised by [`fix::fix`],
//! the port of `_pydecimal._fix`.
//!
//! **Fixed context.** Monty has no mutable `decimal.Context`; every operation
//! runs under CPython's *default* context, hard-coded here: `prec = 28`
//! ([`DEFAULT_PREC`]), rounding `ROUND_HALF_EVEN`, `Emax = 999999`, `Emin =
//! -999999`, `capitals = 1`, `clamp = 0`. Methods that accept a per-call
//! `rounding=` argument in CPython (`quantize`, `to_integral_value`) accept it
//! here too. See `limitations/decimal.md`.
//!
//! **Signals.** CPython's default traps are `{InvalidOperation,
//! DivisionByZero, Overflow}`, and Monty's trap set is frozen there, so the
//! `_raise_error` machinery collapses to a fixed mapping applied at each port
//! site:
//!
//! | `_pydecimal` signal | here |
//! |---|---|
//! | `ConversionSyntax` | `Err(ExcType::decimal_conversion_syntax())` |
//! | `InvalidOperation` (incl. sNaN operands) | `Err(ExcType::decimal_invalid_operation())` |
//! | `DivisionByZero` | `Err(ExcType::decimal_division_by_zero())` |
//! | `DivisionUndefined` / `DivisionImpossible` | their dedicated helpers |
//! | `Overflow` | `Err(ExcType::decimal_overflow())` |
//! | `Clamped`/`Inexact`/`Rounded`/`Subnormal`/`Underflow`/`FloatOperation` | untrapped → the call site is simply omitted |
//!
//! **Sandbox guards.** CPython's algorithms are bounded *given* a bounded
//! operand size, so the port adds explicit guards where CPython relies on
//! "memory is finite":
//!
//! 1. every constructor caps the coefficient (and NaN payload) at
//!    [`DECIMAL_MAX_DIGITS`] digits and the exponent at the C module's literal
//!    bounds (`parse.rs`);
//! 2. `10**k` materialisations whose `k` derives from a value's exponent
//!    pre-check with `resource::check_pow_size` (`int(d)`, hashing, division
//!    alignment, the pow/transcendental kernels);
//! 3. the `extra += 3` roundability-refinement loops and Newton iterations
//!    poll `check_time` each pass and carry a hard iteration cap;
//! 4. [`fix::rescale`] defends its zero-padding length even though every
//!    caller pre-checks.

mod arith;
mod cmp;
mod fix;
mod format;
mod methods;
mod parse;
mod pow;
mod trans;

use std::{borrow::Cow, fmt};

pub(crate) use arith::{BinOp, abs, binary_op_value, divmod, neg, pos};
pub(crate) use cmp::cmp_value;
pub(crate) use format::{format_decimal, write_decimal};
pub(crate) use methods::{ceil_to_int, floor_to_int, round_to_int, round_with_digits, to_float, to_int, trunc_to_int};
use num_bigint::BigInt;
use num_traits::{Pow, Zero};
pub(crate) use parse::init;
use pow::power_modulo;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunError, RunResult},
    hash::HashValue,
    heap::{DropWithHeap, HeapData, HeapId, HeapItem, HeapRead},
    intern::StaticStrings,
    resource::{ResourceTracker, check_pow_size},
    types::{LazyHeapSet, PyTrait, Type, str::allocate_string},
    value::{EitherStr, Value},
};

/// CPython's default `context.prec` — the working precision every arithmetic
/// result is rounded to. Fixed: Monty has no mutable context.
pub(crate) const DEFAULT_PREC: usize = 28;
/// [`DEFAULT_PREC`] as the `i64` the exponent arithmetic works in (kept as a
/// separate literal so neither direction needs a lossy cast).
const PREC: i64 = 28;
/// CPython's default `Emax` — the largest allowed adjusted exponent of an
/// arithmetic result. A result beyond it raises `decimal.Overflow`.
const EMAX: i64 = 999_999;
/// CPython's default `Emin` (`-Emax`).
const EMIN: i64 = -999_999;
/// `Etiny = Emin - prec + 1` — the smallest allowed result exponent; a result
/// below it is rounded (subnormally) up to this exponent, underflowing to a
/// signed zero when nothing survives.
const ETINY: i64 = EMIN - PREC + 1;
/// `Etop = Emax - prec + 1` — the largest allowed exponent of a full-precision
/// result coefficient.
const ETOP: i64 = EMAX - PREC + 1;

/// Sandbox cap on a `Decimal` coefficient (and NaN payload) in decimal digits,
/// applied by every constructor — the same philosophy as
/// [`long_int::INT_MAX_STR_DIGITS`](crate::types::long_int). Post-[`fix::fix`]
/// values carry ≤ 28 digits, so the cap only bites *unfixed* constructor
/// operands, where it keeps every downstream algorithm's intermediate `BigInt`s
/// small; CPython accepts arbitrarily long literals (documented divergence).
pub(crate) const DECIMAL_MAX_DIGITS: usize = 4300;

/// The C `decimal` module's largest literal exponent (`MAX_EMAX` on 64-bit
/// builds): a literal whose adjusted exponent exceeds this raises
/// `InvalidOperation` at construction, in CPython and here alike.
const MAX_LITERAL_EXP: i64 = 999_999_999_999_999_999;
/// The C module's smallest literal exponent (`MIN_ETINY = MIN_EMIN -
/// (MAX_PREC - 1)`).
const MIN_LITERAL_EXP: i64 = -1_999_999_999_999_999_997;

/// The non-finite kinds a `Decimal` can carry. `Finite` covers zeros and
/// ordinary values; the NaN kinds keep their payload in [`Decimal::coeff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Special {
    Finite,
    /// `±Infinity` — `coeff` is zero and `exp` is `0`.
    Inf,
    /// Quiet NaN (CPython `_exp == 'n'`) — `coeff` holds the payload digits
    /// (zero coefficient ⇔ no payload, printed as bare `NaN`).
    Qnan,
    /// Signaling NaN (CPython `_exp == 'N'`) — any arithmetic use raises
    /// `InvalidOperation`; hashing raises `TypeError`.
    Snan,
}

/// `decimal.Decimal` storage, mirroring `_pydecimal`'s `(_sign, _int, _exp,
/// _is_special)` so the ported algorithms track the original line-for-line.
///
/// Invariants: `sign` is `0` or `1`; `coeff` is non-negative (≤
/// [`DECIMAL_MAX_DIGITS`] digits — enforced at construction); for `Inf` the
/// coefficient is zero; for the NaN kinds `coeff` is the payload and `exp` is
/// `0`. A leaf heap type: no heap references, never GC-tracked. Not `Copy`
/// (the coefficient is a `BigInt`), so callers clone out of heap reads.
///
/// The derived `PartialEq` is *structural* (`1.2 != 1.20`); Python equality
/// goes through [`cmp`], never this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decimal {
    sign: u8,
    coeff: BigInt,
    exp: i64,
    special: Special,
}

impl Decimal {
    /// `Decimal('0')`.
    fn zero() -> Self {
        Self::from_triple(0, BigInt::ZERO, 0)
    }

    /// A finite value from raw parts — `_pydecimal._dec_from_triple`. The
    /// coefficient must be non-negative and within [`DECIMAL_MAX_DIGITS`]
    /// (constructors validate; arithmetic results are bounded by `fix`).
    fn from_triple(sign: u8, coeff: BigInt, exp: i64) -> Self {
        debug_assert!(sign <= 1, "sign is 0 or 1");
        debug_assert!(coeff.sign() != num_bigint::Sign::Minus, "coefficient is non-negative");
        Self {
            sign,
            coeff,
            exp,
            special: Special::Finite,
        }
    }

    /// A signed infinity.
    fn infinity(sign: u8) -> Self {
        Self {
            sign,
            coeff: BigInt::ZERO,
            exp: 0,
            special: Special::Inf,
        }
    }

    /// A quiet NaN with the given payload (`BigInt::ZERO` for none).
    fn qnan(sign: u8, payload: BigInt) -> Self {
        Self {
            sign,
            coeff: payload,
            exp: 0,
            special: Special::Qnan,
        }
    }

    /// A signaling NaN with the given payload.
    fn snan(sign: u8, payload: BigInt) -> Self {
        Self {
            sign,
            coeff: payload,
            exp: 0,
            special: Special::Snan,
        }
    }

    /// An exact small-integer value.
    fn from_i64(i: i64) -> Self {
        Self::from_triple(u8::from(i < 0), BigInt::from(i.unsigned_abs()), 0)
    }

    fn is_special(&self) -> bool {
        self.special != Special::Finite
    }

    pub(crate) fn is_finite(&self) -> bool {
        self.special == Special::Finite
    }

    fn is_nan(&self) -> bool {
        matches!(self.special, Special::Qnan | Special::Snan)
    }

    fn is_qnan(&self) -> bool {
        self.special == Special::Qnan
    }

    fn is_snan(&self) -> bool {
        self.special == Special::Snan
    }

    fn is_infinite(&self) -> bool {
        self.special == Special::Inf
    }

    /// True only for a finite zero (`bool(Decimal('NaN'))` is `True`).
    fn is_zero(&self) -> bool {
        self.is_finite() && self.coeff.is_zero()
    }

    /// Whether the sign bit is set (`-0` and `-NaN` included) — CPython's
    /// `is_signed()`.
    fn is_signed(&self) -> bool {
        self.sign == 1
    }

    /// `-1` for `-Infinity`, `1` for `Infinity`, `0` otherwise — CPython's
    /// `_isinfinity()`.
    fn infinity_sign(&self) -> i8 {
        match self.special {
            Special::Inf => {
                if self.sign == 1 {
                    -1
                } else {
                    1
                }
            }
            _ => 0,
        }
    }

    /// The coefficient's decimal digit string — CPython's `_int` (`"0"` for a
    /// zero; the payload digits for a NaN, where CPython uses `""` for an
    /// empty payload — callers that care check [`Self::is_zero`] / payload
    /// emptiness via `coeff.is_zero()` instead of the string).
    fn coeff_str(&self) -> String {
        self.coeff.to_string()
    }

    /// Number of digits in the coefficient — CPython's `len(self._int)`
    /// (`1` for a zero).
    fn digits(&self) -> usize {
        usize::try_from(magnitude_digits(&self.coeff)).expect("digit count fits usize")
    }

    /// `Decimal.adjusted()` — the exponent of the most-significant digit
    /// (`exp + digits − 1`); `0` for specials, matching CPython.
    fn adjusted(&self) -> i64 {
        if self.is_special() {
            0
        } else {
            self.exp + i64::try_from(self.digits()).expect("digit count fits i64") - 1
        }
    }

    /// `copy_abs()` — `|self|` without rounding or signalling (works on sNaN).
    fn copy_abs(&self) -> Self {
        Self {
            sign: 0,
            ..self.clone()
        }
    }

    /// `copy_negate()` — sign flipped without rounding or signalling.
    fn copy_negate(&self) -> Self {
        Self {
            sign: self.sign ^ 1,
            ..self.clone()
        }
    }
}

/// The `_pydecimal._check_nans` port for the fixed context: an sNaN operand
/// raises `InvalidOperation` (the trap is always armed); otherwise a quiet-NaN
/// operand propagates as the result — payload decapitated exactly as
/// `_fix_nan` does — and `None` means "no NaN involved, continue".
fn check_nans(a: &Decimal, b: Option<&Decimal>) -> RunResult<Option<Decimal>> {
    if a.is_snan() || b.is_some_and(Decimal::is_snan) {
        Err(ExcType::decimal_invalid_operation())
    } else if a.is_qnan() {
        Ok(Some(fix::fix_nan(a.clone())))
    } else if let Some(b) = b
        && b.is_qnan()
    {
        Ok(Some(fix::fix_nan(b.clone())))
    } else {
        Ok(None)
    }
}

/// The canonical, lossless, parse-round-trippable string for `d` — the same
/// string used by `str()`, snapshots, and the host boundary. Shared so all
/// four surfaces agree byte-for-byte.
pub(crate) fn canonical_string(d: &Decimal) -> String {
    let mut s = String::new();
    write_decimal(d, true, &mut s).expect("writing to a String is infallible");
    s
}

/// Parses a host/wire canonical decimal string into a heap `Decimal` `Value`.
/// Returns `None` on an unparsable or guard-rejected string — untrusted
/// boundary input must validate. Routes through the same parser as in-sandbox
/// `Decimal(str)`, so the digit cap and exponent bounds apply identically.
pub(crate) fn value_from_canonical_string(s: &str, vm: &mut VM<'_, impl ResourceTracker>) -> Option<Value> {
    let d = parse::parse_str(s).ok()?;
    allocate(d, vm).ok()
}

/// Truthiness of a canonical decimal string for the host boundary: only zero
/// is falsy. An unparsable string defaults to truthy.
#[must_use]
pub(crate) fn string_is_truthy(s: &str) -> bool {
    parse::parse_str(s).map_or(true, |d| !d.is_zero())
}

/// Three-argument `pow(base, exp, mod)` with `Decimal` operands: when *any* of
/// the three is a `Decimal`, all must promote (int / `LongInt` / `Decimal`) and
/// the result is `power_modulo`. `Ok(None)` when no operand is a `Decimal`, or
/// when one is a `float` — CPython's `float.__pow__` then wins with the
/// integers-only TypeError the `pow()` builtin's fallthrough produces. Any
/// other unpromotable operand (`str`, `list`, …) raises CPython's
/// three-operand `unsupported operand type(s)` TypeError.
pub(crate) fn pow3(
    base: &Value,
    exp: &Value,
    modulus: &Value,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<Value>> {
    let any_decimal = [base, exp, modulus]
        .into_iter()
        .any(|v| matches!(v, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Decimal(_))));
    if !any_decimal {
        return Ok(None);
    }
    let (Some(a), Some(b), Some(m)) = (
        arith::promote(base, vm.heap)?,
        arith::promote(exp, vm.heap)?,
        arith::promote(modulus, vm.heap)?,
    ) else {
        // A `float` operand reproduces CPython's slot order: `float.__pow__`
        // raises the integers-only message before the ternary fallback fires.
        return if [base, exp, modulus].into_iter().any(|v| matches!(v, Value::Float(_))) {
            Ok(None)
        } else {
            Err(ExcType::pow3_type_error(
                base.py_type_name(vm),
                exp.py_type_name(vm),
                modulus.py_type_name(vm),
            ))
        };
    };
    power_modulo(a, b, m, vm).map(Some)
}

impl Serialize for Decimal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The canonical string is lossless (sign, payload, trailing zeros and
        // exponent identity all survive) and shared with the host boundary.
        serializer.serialize_str(&canonical_string(self))
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <Cow<'_, str>>::deserialize(deserializer)?;
        // Snapshots are untrusted: re-parse through the same validating parser
        // as `Decimal(str)`, so a corrupt or out-of-bounds coefficient is a
        // serde error (surfaced as a rejected load), never a bad value.
        parse::parse_str(&s).map_err(|_| de::Error::custom(format!("invalid decimal {s:?}")))
    }
}

/// `10**k` as a `BigInt`, *unguarded*: only for exponents structurally
/// bounded by operand digit counts (≤ ~2·[`DECIMAL_MAX_DIGITS`] — each call
/// site documents its bound). Never pass an attacker-scaled exponent —
/// value-exponent-derived powers must use [`pow10`] instead so the tracker
/// vets the materialisation.
fn pow10_bounded(k: u64) -> BigInt {
    BigInt::from(10u8).pow(k)
}

/// `10**e` for `e >= 0` under the sandbox pre-check (guard 2 in the module
/// docs): every power of ten whose exponent derives from a value's exponent
/// field routes through here, so `check_pow_size` vets the materialisation
/// even where a ported CPython bound already caps it.
fn pow10(e: i64, tracker: &impl ResourceTracker) -> RunResult<BigInt> {
    let magnitude = u64::try_from(e).map_err(|_| pow10_bound_error())?;
    check_pow_size(4, magnitude, tracker)?;
    let small = u32::try_from(magnitude).map_err(|_| pow10_bound_error())?;
    Ok(BigInt::from(10).pow(small))
}

/// Guard error for a [`pow10`] exponent outside `0..=u32::MAX` — unreachable
/// through the current callers (each is bounded by a ported CPython check or
/// by `check_pow_size` under any real memory limit); an internal error rather
/// than a Python exception because reaching it means a caller lost its bound.
fn pow10_bound_error() -> RunError {
    RunError::internal("decimal pow10 exponent out of bounds")
}

/// Python's `len(str(abs(n)))` — the decimal digit count of `n`'s magnitude
/// (`1` for zero). The single home for the "stringify is cheap" safety
/// argument: every coefficient this sees is bounded by the constructor digit
/// cap (≤ [`DECIMAL_MAX_DIGITS`], ~2× for arithmetic intermediates) or by the
/// kernels' precision-derived sizes, so the transient string stays a few KB.
fn magnitude_digits(n: &BigInt) -> i64 {
    i64::try_from(n.magnitude().to_string().len()).expect("digit count fits i64")
}

/// Allocates a `Decimal` on the heap — the single finalize point for
/// construction and arithmetic results.
fn allocate(d: Decimal, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    Ok(Value::Ref(vm.heap.allocate(HeapData::Decimal(d))?))
}

/// The eight CPython `ROUND_*` modes: each interned name paired with the
/// [`RoundMode`] it selects. Single source of truth for the set — the
/// `decimal` module registers each name as a `ROUND_*` string constant, and
/// per-call `rounding=` arguments resolve against the same table.
pub(crate) const ROUNDING_MODES: [(StaticStrings, RoundMode); 8] = [
    (StaticStrings::RoundCeiling, RoundMode::Ceiling),
    (StaticStrings::RoundFloor, RoundMode::Floor),
    (StaticStrings::RoundUp, RoundMode::Up),
    (StaticStrings::RoundDown, RoundMode::Down),
    (StaticStrings::RoundHalfUp, RoundMode::HalfUp),
    (StaticStrings::RoundHalfDown, RoundMode::HalfDown),
    (StaticStrings::RoundHalfEven, RoundMode::HalfEven),
    (StaticStrings::Round05Up, RoundMode::Zero05Up),
];

/// The rounding rule applied when dropping digits. All eight CPython modes are
/// supported; the fixed context's default is [`RoundMode::HalfEven`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoundMode {
    Down,
    Up,
    Floor,
    Ceiling,
    HalfUp,
    HalfDown,
    HalfEven,
    /// `ROUND_05UP` — round away from zero only if the digit left of the cut
    /// is `0` or `5`, otherwise toward zero.
    Zero05Up,
}

impl HeapItem for Decimal {
    fn py_estimate_size(&self) -> usize {
        // The struct plus the coefficient's out-of-line limbs (mirrors
        // `LongInt`'s accounting).
        size_of::<Self>() + usize::try_from(self.coeff.bits().div_ceil(8)).unwrap_or(usize::MAX)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

/// `HeapRead`-based dispatch for `Decimal`, letting `HeapReadOutput` delegate
/// `PyTrait` calls to heap-resident decimals. Operations clone the value out
/// of the heap read (one small `BigInt` allocation — post-`fix` coefficients
/// are ≤ 28 digits) so the heap borrow ends before the VM is re-borrowed.
impl<'h> PyTrait<'h> for HeapRead<'h, Decimal> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Decimal
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        // Exact CPython equality against the numeric tower (`int` / `bool` /
        // `float` / `LongInt` / `Decimal`). `Ok(None)` (`NotImplemented`) for
        // any non-number so `Value::py_eq` can try the reflected comparison. A
        // quiet NaN compares unequal to everything; an sNaN raises.
        cmp::eq_value(self.get(vm.heap), other, vm.heap)
    }

    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        cmp::hash_decimal(self.get(vm.heap), vm.heap.tracker()).map(Some)
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        // Only zero is falsy — `bool(Decimal('NaN'))` and `bool(Decimal('Inf'))`
        // are both `True`.
        !self.get(vm.heap).is_zero()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl fmt::Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        f.write_str("Decimal('")?;
        write_decimal(self.get(vm.heap), true, f)?;
        f.write_str("')")?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        Ok(allocate_string(canonical_string(self.get(vm.heap)), vm.heap)?)
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(vm);
            return Err(ExcType::attribute_error(Type::Decimal, attr.as_str(vm.interns)));
        };
        let d = self.get(vm.heap).clone();
        // Every zero-argument method (predicate or value-producing) MUST
        // validate its argument count *before* computing: the value-producing
        // ones allocate a heap result that would leak — and panic under
        // `memory-model-checks` — if a spurious argument were only rejected
        // afterwards. `zero_arg!` enforces that order.
        macro_rules! zero_arg {
            ($value:expr) => {{
                methods::check_no_args(args, attr, vm)?;
                Ok(CallResult::Value($value))
            }};
        }
        match method {
            // Zero-argument predicates (return bool).
            StaticStrings::IsNan => zero_arg!(Value::Bool(d.is_nan())),
            StaticStrings::IsQnan => zero_arg!(Value::Bool(d.is_qnan())),
            StaticStrings::IsSnan => zero_arg!(Value::Bool(d.is_snan())),
            StaticStrings::IsInfinite => zero_arg!(Value::Bool(d.is_infinite())),
            StaticStrings::IsFinite => zero_arg!(Value::Bool(d.is_finite())),
            StaticStrings::IsZero => zero_arg!(Value::Bool(d.is_zero())),
            StaticStrings::IsSigned => zero_arg!(Value::Bool(d.is_signed())),
            // Zero-argument methods returning a Decimal (or int, for `adjusted`).
            StaticStrings::Sqrt => zero_arg!(trans::sqrt(d, vm)?),
            StaticStrings::Ln => zero_arg!(trans::ln(&d, vm)?),
            StaticStrings::Log10 => zero_arg!(trans::log10(&d, vm)?),
            StaticStrings::Exp => zero_arg!(trans::exp(d, vm)?),
            StaticStrings::Normalize => zero_arg!(methods::normalize(d, vm)?),
            StaticStrings::CopyAbs => zero_arg!(allocate(d.copy_abs(), vm)?),
            StaticStrings::CopyNegate => zero_arg!(allocate(d.copy_negate(), vm)?),
            StaticStrings::AsTuple => zero_arg!(methods::as_tuple(&d, vm)?),
            StaticStrings::Adjusted => zero_arg!(Value::Int(d.adjusted())),
            // Methods with arguments (each validates its own arity/kwargs).
            StaticStrings::Quantize => methods::quantize_method(d, args, vm),
            StaticStrings::ToIntegralValue => methods::to_integral_value_method(d, args, vm),
            StaticStrings::CopySign => methods::copy_sign_method(d, args, vm),
            _ => {
                args.drop_with_heap(vm);
                Err(ExcType::attribute_error(Type::Decimal, attr.as_str(vm.interns)))
            }
        }
    }
}
