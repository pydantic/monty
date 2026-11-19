//! Implementation of Python's `math` module.
//!
//! Provides mathematical functions and constants matching CPython 3.14 behavior
//! and error messages. All functions are pure computations that don't require
//! host involvement, so they return `Value` directly rather than `CallResult`.
//!
//! ## Implemented functions
//!
//! **Rounding**: `floor`, `ceil`, `trunc`
//! **Roots & powers**: `sqrt`, `isqrt`, `cbrt`, `pow`, `exp`, `exp2`, `expm1`
//! **Logarithms**: `log`, `log2`, `log10`, `log1p`
//! **Trigonometric**: `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`
//! **Hyperbolic**: `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`
//! **Angular**: `degrees`, `radians`
//! **Float properties**: `fabs`, `isnan`, `isinf`, `isfinite`, `copysign`, `isclose`,
//!   `nextafter`, `ulp`
//! **Integer math**: `factorial`, `gcd`, `lcm`, `comb`, `perm`
//! **Modular**: `fmod`, `remainder`, `modf`, `frexp`, `ldexp`
//! **Special**: `gamma`, `lgamma`, `erf`, `erfc`
//! **Summation & products**: `hypot`, `dist`, `fsum`, `prod`, `sumprod`, `fma`
//!
//! ## Constants
//!
//! `pi`, `e`, `tau`, `inf`, `nan`

use std::f64::consts;

use monty_macros::py_signature;
use monty_types::ResourceTracker;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource_checks::{check_mult_size, check_product_size},
    types::{LongInt, Module, PyTrait, allocate_tuple, decimal, long_int::bigint_to_f64_checked},
    value::Value,
};

mod aggregate;
pub(crate) use aggregate::{fsum, sumprod};

// ==========================
// Shared constants and error helpers
// ==========================

/// Returns a `ValueError` with the standard CPython "math domain error" message.
fn math_domain_error() -> RunError {
    SimpleException::new_msg(ExcType::ValueError, "math domain error").into()
}

/// Returns an `OverflowError` with the standard CPython "math range error" message.
fn math_range_error() -> RunError {
    ExcType::overflow_math_range()
}

/// Checks whether a computation overflowed (finite input produced infinite result).
///
/// Returns `Err(OverflowError("math range error"))` if `result` is infinite
/// but `input` was finite.
fn check_range_error(result: f64, input: f64) -> RunResult<()> {
    if result.is_infinite() && input.is_finite() {
        Err(math_range_error())
    } else {
        Ok(())
    }
}

/// Checks that a value is in the `[-1, 1]` range, raising `ValueError` if not.
///
/// NaN passes through (it will propagate through the subsequent math operation).
/// Used by `math.asin` and `math.acos`.
fn require_unit_range(f: f64) -> RunResult<()> {
    if !f.is_nan() && !(-1.0..=1.0).contains(&f) {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected a number in range from -1 up to 1, got {f:?}"),
        )
        .into())
    } else {
        Ok(())
    }
}

/// Checks for non-positive integer arguments (poles of the Gamma function).
///
/// These are the finite non-positive integers where Gamma diverges to ±∞.
/// Does NOT reject `-inf` — callers that need to reject it (like `math.gamma`)
/// must do so separately, since `lgamma(-inf)` is valid and returns `inf`.
#[expect(
    clippy::float_cmp,
    reason = "exact comparison detects integer poles of gamma function"
)]
fn check_gamma_pole(f: f64) -> RunResult<()> {
    if f <= 0.0 && f == f.floor() && f.is_finite() {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected a noninteger or positive integer, got {f:?}"),
        )
        .into())
    } else {
        Ok(())
    }
}

/// Math module functions — each variant corresponds to a Python-visible function.
#[monty_macros::explicit_discriminants(checksum = 0x0fdd_acb7_255d_bb02)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum MathFunctions {
    // Rounding
    Floor = 0,
    Ceil = 1,
    Trunc = 2,
    // Roots & powers
    Sqrt = 3,
    Isqrt = 4,
    Cbrt = 5,
    Pow = 6,
    Exp = 7,
    Exp2 = 8,
    Expm1 = 9,
    // Logarithms
    Log = 10,
    Log1p = 11,
    Log2 = 12,
    Log10 = 13,
    // Float properties
    Fabs = 14,
    Isnan = 15,
    Isinf = 16,
    Isfinite = 17,
    Copysign = 18,
    Isclose = 19,
    Nextafter = 20,
    Ulp = 21,
    // Trigonometric
    Sin = 22,
    Cos = 23,
    Tan = 24,
    Asin = 25,
    Acos = 26,
    Atan = 27,
    Atan2 = 28,
    // Hyperbolic
    Sinh = 29,
    Cosh = 30,
    Tanh = 31,
    Asinh = 32,
    Acosh = 33,
    Atanh = 34,
    // Angular conversion
    Degrees = 35,
    Radians = 36,
    // Integer math
    Factorial = 37,
    Gcd = 38,
    Lcm = 39,
    Comb = 40,
    Perm = 41,
    // Modular / decomposition
    Fmod = 42,
    Remainder = 43,
    Modf = 44,
    Frexp = 45,
    Ldexp = 46,
    // Special functions
    Gamma = 47,
    Lgamma = 48,
    Erf = 49,
    Erfc = 50,
    // Summation and products
    Hypot = 51,
    Dist = 52,
    Fsum = 53,
    Prod = 54,
    Sumprod = 55,
    Fma = 56,
}

/// Creates the `math` module.
///
/// Registers all math functions and constants (`pi`, `e`, `tau`, `inf`, `nan`)
/// matching CPython's `math` module. Functions are registered as
/// `ModuleFunctions::Math` variants.
pub fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Math, vm.interns);

    // Register all math functions
    for (name, func) in MATH_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Math(*func)), vm);
    }

    // Constants
    module.set_attr(StaticStrings::Pi, Value::Float(consts::PI), vm);
    module.set_attr(StaticStrings::AsciiLowerE, Value::Float(consts::E), vm);
    module.set_attr(StaticStrings::Tau, Value::Float(consts::TAU), vm);
    module.set_attr(StaticStrings::MathInf, Value::Float(f64::INFINITY), vm);
    module.set_attr(StaticStrings::MathNan, Value::Float(f64::NAN), vm);

    module
}

/// Static mapping of attribute names to math functions for module creation.
const MATH_FUNCTIONS: &[(StaticStrings, MathFunctions)] = &[
    // Rounding
    (StaticStrings::Floor, MathFunctions::Floor),
    (StaticStrings::Ceil, MathFunctions::Ceil),
    (StaticStrings::Trunc, MathFunctions::Trunc),
    // Roots & powers
    (StaticStrings::Sqrt, MathFunctions::Sqrt),
    (StaticStrings::Isqrt, MathFunctions::Isqrt),
    (StaticStrings::Cbrt, MathFunctions::Cbrt),
    (StaticStrings::Pow, MathFunctions::Pow),
    (StaticStrings::Exp, MathFunctions::Exp),
    (StaticStrings::Exp2, MathFunctions::Exp2),
    (StaticStrings::Expm1, MathFunctions::Expm1),
    // Logarithms
    (StaticStrings::Log, MathFunctions::Log),
    (StaticStrings::Log1p, MathFunctions::Log1p),
    (StaticStrings::Log2, MathFunctions::Log2),
    (StaticStrings::Log10, MathFunctions::Log10),
    // Float properties
    (StaticStrings::Fabs, MathFunctions::Fabs),
    (StaticStrings::Isnan, MathFunctions::Isnan),
    (StaticStrings::Isinf, MathFunctions::Isinf),
    (StaticStrings::Isfinite, MathFunctions::Isfinite),
    (StaticStrings::Copysign, MathFunctions::Copysign),
    (StaticStrings::Isclose, MathFunctions::Isclose),
    (StaticStrings::Nextafter, MathFunctions::Nextafter),
    (StaticStrings::Ulp, MathFunctions::Ulp),
    // Trigonometric
    (StaticStrings::Sin, MathFunctions::Sin),
    (StaticStrings::Cos, MathFunctions::Cos),
    (StaticStrings::Tan, MathFunctions::Tan),
    (StaticStrings::Asin, MathFunctions::Asin),
    (StaticStrings::Acos, MathFunctions::Acos),
    (StaticStrings::Atan, MathFunctions::Atan),
    (StaticStrings::Atan2, MathFunctions::Atan2),
    // Hyperbolic
    (StaticStrings::Sinh, MathFunctions::Sinh),
    (StaticStrings::Cosh, MathFunctions::Cosh),
    (StaticStrings::Tanh, MathFunctions::Tanh),
    (StaticStrings::Asinh, MathFunctions::Asinh),
    (StaticStrings::Acosh, MathFunctions::Acosh),
    (StaticStrings::Atanh, MathFunctions::Atanh),
    // Angular conversion
    (StaticStrings::Degrees, MathFunctions::Degrees),
    (StaticStrings::Radians, MathFunctions::Radians),
    // Integer math
    (StaticStrings::Factorial, MathFunctions::Factorial),
    (StaticStrings::Gcd, MathFunctions::Gcd),
    (StaticStrings::Lcm, MathFunctions::Lcm),
    (StaticStrings::Comb, MathFunctions::Comb),
    (StaticStrings::Perm, MathFunctions::Perm),
    // Modular / decomposition
    (StaticStrings::Fmod, MathFunctions::Fmod),
    (StaticStrings::Remainder, MathFunctions::Remainder),
    (StaticStrings::Modf, MathFunctions::Modf),
    (StaticStrings::Frexp, MathFunctions::Frexp),
    (StaticStrings::Ldexp, MathFunctions::Ldexp),
    // Special functions
    (StaticStrings::Gamma, MathFunctions::Gamma),
    (StaticStrings::Lgamma, MathFunctions::Lgamma),
    (StaticStrings::Erf, MathFunctions::Erf),
    (StaticStrings::Erfc, MathFunctions::Erfc),
    // Summation and products
    (StaticStrings::Hypot, MathFunctions::Hypot),
    (StaticStrings::Dist, MathFunctions::Dist),
    (StaticStrings::Fsum, MathFunctions::Fsum),
    (StaticStrings::Prod, MathFunctions::Prod),
    (StaticStrings::Sumprod, MathFunctions::Sumprod),
    (StaticStrings::Fma, MathFunctions::Fma),
];

/// Dispatches a call to a math module function.
///
/// All math functions are pure computations and return `Value` directly.
pub(super) fn call(vm: &mut VM<'_>, function: MathFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        // Rounding
        MathFunctions::Floor => math_floor(vm, args),
        MathFunctions::Ceil => math_ceil(vm, args),
        MathFunctions::Trunc => math_trunc(vm, args),
        // Roots & powers
        MathFunctions::Sqrt => math_sqrt(vm, args),
        MathFunctions::Isqrt => math_isqrt(vm, args),
        MathFunctions::Cbrt => math_cbrt(vm, args),
        MathFunctions::Pow => math_pow(vm, args),
        MathFunctions::Exp => math_exp(vm, args),
        MathFunctions::Exp2 => math_exp2(vm, args),
        MathFunctions::Expm1 => math_expm1(vm, args),
        // Logarithms
        MathFunctions::Log => math_log(vm, args),
        MathFunctions::Log1p => math_log1p(vm, args),
        MathFunctions::Log2 => math_log2(vm, args),
        MathFunctions::Log10 => math_log10(vm, args),
        // Float properties
        MathFunctions::Fabs => math_fabs(vm, args),
        MathFunctions::Isnan => math_isnan(vm, args),
        MathFunctions::Isinf => math_isinf(vm, args),
        MathFunctions::Isfinite => math_isfinite(vm, args),
        MathFunctions::Copysign => math_copysign(vm, args),
        MathFunctions::Isclose => math_isclose(vm, args),
        MathFunctions::Nextafter => math_nextafter(vm, args),
        MathFunctions::Ulp => math_ulp(vm, args),
        // Trigonometric
        MathFunctions::Sin => math_sin(vm, args),
        MathFunctions::Cos => math_cos(vm, args),
        MathFunctions::Tan => math_tan(vm, args),
        MathFunctions::Asin => math_asin(vm, args),
        MathFunctions::Acos => math_acos(vm, args),
        MathFunctions::Atan => math_atan(vm, args),
        MathFunctions::Atan2 => math_atan2(vm, args),
        // Hyperbolic
        MathFunctions::Sinh => math_sinh(vm, args),
        MathFunctions::Cosh => math_cosh(vm, args),
        MathFunctions::Tanh => math_tanh(vm, args),
        MathFunctions::Asinh => math_asinh(vm, args),
        MathFunctions::Acosh => math_acosh(vm, args),
        MathFunctions::Atanh => math_atanh(vm, args),
        // Angular conversion
        MathFunctions::Degrees => math_degrees(vm, args),
        MathFunctions::Radians => math_radians(vm, args),
        // Integer math
        MathFunctions::Factorial => math_factorial(vm, args),
        MathFunctions::Gcd => math_gcd(vm, args),
        MathFunctions::Lcm => math_lcm(vm, args),
        MathFunctions::Comb => math_comb(vm, args),
        MathFunctions::Perm => math_perm(vm, args),
        // Modular / decomposition
        MathFunctions::Fmod => math_fmod(vm, args),
        MathFunctions::Remainder => math_remainder(vm, args),
        MathFunctions::Modf => math_modf(vm, args),
        MathFunctions::Frexp => math_frexp(vm, args),
        MathFunctions::Ldexp => math_ldexp(vm, args),
        // Special functions
        MathFunctions::Gamma => math_gamma(vm, args),
        MathFunctions::Lgamma => math_lgamma(vm, args),
        MathFunctions::Erf => math_erf(vm, args),
        MathFunctions::Erfc => math_erfc(vm, args),
        MathFunctions::Hypot => aggregate::hypot(vm, args),
        MathFunctions::Dist => aggregate::dist(vm, args),
        MathFunctions::Fsum => aggregate::fsum(vm, args),
        MathFunctions::Prod => aggregate::prod(vm, args),
        MathFunctions::Sumprod => aggregate::sumprod(vm, args),
        MathFunctions::Fma => aggregate::fma(vm, args),
    }
}

// ==========================
// Rounding functions
// ==========================

/// `math.floor(x)` — returns the largest integer less than or equal to x.
///
/// Uses `__floor__`, falling back to numeric float conversion.
/// Raises `OverflowError` for infinity, `ValueError` for NaN.
#[py_signature("floor(x, /)", error_name = "math.floor")]
fn math_floor(vm: &mut VM<'_>, params: &FloorArgs) -> RunResult<Value> {
    if let Some(result) = params.x.py_floor_impl(vm)? {
        Ok(result)
    } else {
        let number = rounding_float(&params.x, vm)?;
        LongInt::value_from_f64(number.floor(), vm.heap)
    }
}

/// `math.ceil(x)` — returns the smallest integer greater than or equal to x.
///
/// Uses `__ceil__`, falling back to numeric float conversion.
/// Raises `OverflowError` for infinity, `ValueError` for NaN.
#[py_signature("ceil(x, /)", error_name = "math.ceil")]
fn math_ceil(vm: &mut VM<'_>, params: &CeilArgs) -> RunResult<Value> {
    if let Some(result) = params.x.py_ceil_impl(vm)? {
        Ok(result)
    } else {
        let number = rounding_float(&params.x, vm)?;
        LongInt::value_from_f64(number.ceil(), vm.heap)
    }
}

/// `math.trunc(x)` — truncates x to the nearest integer toward zero.
///
/// Uses `__trunc__` without falling back to integer or float conversion.
#[py_signature("trunc(x, /)", error_name = "math.trunc")]
fn math_trunc(vm: &mut VM<'_>, params: &TruncArgs) -> RunResult<Value> {
    params
        .x
        .py_trunc_impl(vm)?
        .ok_or_else(|| ExcType::type_error_missing_numeric_dunder(&params.x.py_type_name(vm), "__trunc__"))
}

/// Float fallback for floor and ceil, excluding the string parsing of `float()`.
fn rounding_float(value: &Value, vm: &mut VM<'_>) -> RunResult<f64> {
    value
        .py_float(vm)?
        .ok_or_else(|| ExcType::type_error_not_real_number(&value.py_type_name(vm)))
}

// ==========================
// Roots & powers
// ==========================

/// `math.sqrt(x)` — returns the square root of x.
///
/// Always returns a float. Raises `ValueError` for negative inputs with a
/// descriptive message matching CPython 3.14: "expected a nonnegative input, got <x>".
#[py_signature("sqrt(x: float, /)", error_name = "math.sqrt")]
fn math_sqrt(vm: &mut VM<'_>, params: &SqrtArgs) -> RunResult<Value> {
    if params.x < 0.0 {
        Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected a nonnegative input, got {x:?}", x = params.x),
        )
        .into())
    } else {
        Ok(Value::Float(params.x.sqrt()))
    }
}

/// `math.isqrt(n)` — returns the integer square root of a non-negative integer.
///
/// Returns the largest integer `r` such that `r * r <= n`.
/// Only accepts non-negative integers (and bools).
#[py_signature("isqrt(n: int, /)", error_name = "math.isqrt")]
fn math_isqrt(vm: &mut VM<'_>, params: &IsqrtArgs) -> RunResult<Value> {
    params
        .n
        .view(vm)
        .with_value(
            vm,
            |n| (n >= 0).then(|| Value::Int(isqrt_i64(n))),
            |n| (!n.is_negative()).then(|| LongInt::new(BigInt::from(n.magnitude().sqrt())).into_value(vm.heap)),
        )
        .ok_or_else(ExcType::isqrt_negative_error)
}

/// Integer square root of a non-negative machine integer.
fn isqrt_i64(n: i64) -> i64 {
    if n == 0 {
        return 0;
    }
    // Integer square root via f64 estimate + correction.
    // For i64 inputs, f64 sqrt is accurate to within ±1, so we need to
    // correct both overshoot and undershoot. The cast truncates toward zero,
    // so undershoot is possible for perfect squares near f64 precision limits.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "initial estimate doesn't need to be exact, correction refines it"
    )]
    let mut x = (n as f64).sqrt() as i64;
    // Correct overshoot: use `x > n / x` instead of `x * x > n` to avoid i64 overflow.
    while x > n / x {
        x -= 1;
    }
    // Correct undershoot: check if (x+1)² ≤ n using division to avoid overflow.
    while x < n / (x + 1) {
        x += 1;
    }
    x
}

/// `math.cbrt(x)` — returns the cube root of x.
///
/// Always returns a float. Unlike `sqrt`, works for negative inputs.
#[py_signature("cbrt(x: float, /)", error_name = "math.cbrt")]
fn math_cbrt(vm: &mut VM<'_>, params: &CbrtArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.cbrt()))
}

/// `math.pow(x, y)` — returns x raised to the power y.
///
/// Always returns a float. Unlike the builtin `pow()`, does not support
/// three-argument modular exponentiation. Raises `ValueError` for
/// negative base with non-integer exponent.
#[py_signature("pow(x: float, y: float, /)", error_name = "math.pow")]
fn math_pow(vm: &mut VM<'_>, params: &MathPowArgs) -> RunResult<Value> {
    let result = params.x.powf(params.y);
    // CPython raises ValueError for domain errors: 0**negative, negative**non-integer
    if result.is_nan() && !params.x.is_nan() && !params.y.is_nan() {
        return Err(math_domain_error());
    }
    if result.is_infinite() && params.x.is_finite() && params.y.is_finite() {
        // 0**negative is a domain error (ValueError), not overflow
        if params.x == 0.0 && params.y < 0.0 {
            return Err(math_domain_error());
        }
        return Err(math_range_error());
    }
    Ok(Value::Float(result))
}

/// `math.exp(x)` — returns e raised to the power x.
#[py_signature("exp(x: float, /)", error_name = "math.exp")]
fn math_exp(vm: &mut VM<'_>, params: &ExpArgs) -> RunResult<Value> {
    let result = params.x.exp();
    check_range_error(result, params.x)?;
    Ok(Value::Float(result))
}

/// `math.exp2(x)` — returns 2 raised to the power x.
#[py_signature("exp2(x: float, /)", error_name = "math.exp2")]
fn math_exp2(vm: &mut VM<'_>, params: &Exp2Args) -> RunResult<Value> {
    let result = params.x.exp2();
    check_range_error(result, params.x)?;
    Ok(Value::Float(result))
}

/// `math.expm1(x)` — returns e**x - 1.
///
/// More accurate than `exp(x) - 1` for small values of x.
#[py_signature("expm1(x: float, /)", error_name = "math.expm1")]
fn math_expm1(vm: &mut VM<'_>, params: &Expm1Args) -> RunResult<Value> {
    let result = params.x.exp_m1();
    check_range_error(result, params.x)?;
    Ok(Value::Float(result))
}

// ==========================
// Logarithms
// ==========================

/// `math.log(x[, base])` — returns the logarithm of x.
///
/// With one argument, returns the natural logarithm (base e).
/// With two arguments, returns `log(x) / log(base)`.
#[py_signature("log(x, base=..., /)")]
fn math_log(vm: &mut VM<'_>, params: &LogArgs) -> RunResult<Value> {
    // CPython's arity errors name the bare function: `log expected at most 2 arguments, got 3`.

    let numerator = log_arg(&params.x, f64::ln, vm)?;
    match &params.base {
        Some(base_v) => {
            // `log(1) == 0.0`, so a base of 1 divides by zero as in CPython.
            let denominator = log_arg(base_v, f64::ln, vm)?;
            if denominator == 0.0 {
                Err(ExcType::zero_division().into())
            } else {
                Ok(Value::Float(numerator / denominator))
            }
        }
        None => Ok(Value::Float(numerator)),
    }
}

/// Applies a logarithm to a `math` argument, after CPython's `loghelper`.
///
/// An int beyond the float range is split as `m * 2**e` with `m` in `[0.5, 1)`, so
/// `math.log(10**400)` works where `float(10**400)` overflows. Non-positive inputs
/// raise `ValueError`; only the float message names the offending value.
#[expect(clippy::cast_precision_loss, reason = "the exponent is a bit count, far below 2**53")]
fn log_arg(value: &Value, log: fn(f64) -> f64, vm: &VM<'_>) -> RunResult<f64> {
    let positive_input_error = || SimpleException::new_msg(ExcType::ValueError, "expected a positive input").into();
    match value {
        Value::Int(n) if *n <= 0 => Err(positive_input_error()),
        Value::Int(n) => Ok(log(*n as f64)),
        Value::Bool(true) => Ok(log(1.0)),
        Value::Bool(false) => Err(positive_input_error()),
        _ => match value.as_long_int(vm) {
            // A long int is never zero.
            Some(n) if n.is_negative() => Err(positive_input_error()),
            Some(n) => Ok(if let Some(x) = n.to_f64().filter(|x| x.is_finite()) {
                log(x)
            } else {
                let (mantissa, exponent) = bigint_frexp(n);
                log(mantissa) + log(2.0) * exponent as f64
            }),
            None => {
                let x = value_to_float(value, vm)?;
                if x <= 0.0 {
                    Err(
                        SimpleException::new_msg(ExcType::ValueError, format!("expected a positive input, got {x:?}"))
                            .into(),
                    )
                } else {
                    Ok(log(x))
                }
            }
        },
    }
}

/// Splits a positive integer into `m * 2**e` with `m` in `[0.5, 1)`, after `_PyLong_Frexp`.
///
/// `m` carries the integer's top bits correctly rounded to a float (a sticky bit stands in
/// for everything below the top 64), so it is exact where `to_f64` would overflow.
#[expect(
    clippy::cast_precision_loss,
    reason = "rounding the top 64 bits to a float is the point"
)]
fn bigint_frexp(n: &BigInt) -> (f64, i64) {
    let bits = n.bits();
    let shift = bits.saturating_sub(64);
    let mut top = (n.magnitude() >> shift).to_u64().unwrap_or(u64::MAX);
    // Any set bit below the top 64 breaks a rounding tie upward, as the full value would.
    if n.trailing_zeros().unwrap_or(0) < shift {
        top |= 1;
    }
    let mantissa = libm::ldexp(top as f64, -64);
    let exponent = i64::try_from(bits).unwrap_or(i64::MAX);
    // Rounding can carry the mantissa up to 1.0; renormalise as CPython does.
    if mantissa >= 1.0 {
        (0.5, exponent + 1)
    } else {
        (mantissa, exponent)
    }
}

/// `math.log1p(x)` — returns the natural logarithm of 1 + x.
///
/// More accurate than `log(1 + x)` for small values of x.
/// CPython 3.14 raises ValueError with "expected argument value > -1, got <x>".
#[py_signature("log1p(x: float, /)", error_name = "math.log1p")]
fn math_log1p(vm: &mut VM<'_>, params: &Log1PArgs) -> RunResult<Value> {
    if params.x <= -1.0 {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected argument value > -1, got {x:?}", x = params.x),
        )
        .into());
    }
    Ok(Value::Float(params.x.ln_1p()))
}

/// `math.log2(x)` — returns the base-2 logarithm of x.
///
/// Returns `inf` for positive infinity, `nan` for NaN.
/// Raises `ValueError` for non-positive finite inputs.
#[py_signature("log2(x, /)", error_name = "math.log2")]
fn math_log2(vm: &mut VM<'_>, params: &Log2Args) -> RunResult<Value> {
    Ok(Value::Float(log_arg(&params.x, f64::log2, vm)?))
}

/// `math.log10(x)` — returns the base-10 logarithm of x.
///
/// Returns `inf` for positive infinity, `nan` for NaN.
/// Raises `ValueError` for non-positive finite inputs.
#[py_signature("log10(x, /)", error_name = "math.log10")]
fn math_log10(vm: &mut VM<'_>, params: &Log10Args) -> RunResult<Value> {
    Ok(Value::Float(log_arg(&params.x, f64::log10, vm)?))
}

// ==========================
// Float properties
// ==========================

/// `math.fabs(x)` — returns the absolute value as a float.
///
/// Unlike the builtin `abs()`, always returns a float.
#[py_signature("fabs(x: float, /)", error_name = "math.fabs")]
fn math_fabs(vm: &mut VM<'_>, params: &FabsArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.abs()))
}

/// `math.isnan(x)` — returns True if x is NaN.
#[py_signature("isnan(x: float, /)", error_name = "math.isnan")]
fn math_isnan(vm: &mut VM<'_>, params: &IsnanArgs) -> RunResult<Value> {
    Ok(Value::Bool(params.x.is_nan()))
}

/// `math.isinf(x)` — returns True if x is positive or negative infinity.
#[py_signature("isinf(x: float, /)", error_name = "math.isinf")]
fn math_isinf(vm: &mut VM<'_>, params: &IsinfArgs) -> RunResult<Value> {
    Ok(Value::Bool(params.x.is_infinite()))
}

/// `math.isfinite(x)` — returns True if x is neither infinity nor NaN.
#[py_signature("isfinite(x: float, /)", error_name = "math.isfinite")]
fn math_isfinite(vm: &mut VM<'_>, params: &IsfiniteArgs) -> RunResult<Value> {
    Ok(Value::Bool(params.x.is_finite()))
}

/// `math.copysign(x, y)` — returns x with the sign of y.
///
/// Always returns a float.
#[py_signature("copysign(x: float, y: float, /)", error_name = "math.copysign")]
fn math_copysign(vm: &mut VM<'_>, params: &MathCopysignArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.copysign(params.y)))
}

/// Raises `ValueError` if either tolerance is negative.
#[py_signature("isclose(a: float, b: float, *, rel_tol: float = 1e-09, abs_tol: float = 0.0) -> bool")]
fn math_isclose(vm: &mut VM<'_>, params: &IscloseArgs) -> RunResult<Value> {
    if params.rel_tol < 0.0 || params.abs_tol < 0.0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "tolerances must be non-negative").into());
    }

    // Exact equality check matches CPython's isclose() behavior — two identical
    // values (including infinities) are always considered close.
    #[expect(
        clippy::float_cmp,
        reason = "exact equality check matches CPython's isclose() semantics"
    )]
    if params.a == params.b {
        return Ok(Value::Bool(true));
    }
    if params.a.is_infinite() || params.b.is_infinite() {
        return Ok(Value::Bool(false));
    }
    if params.a.is_nan() || params.b.is_nan() {
        return Ok(Value::Bool(false));
    }

    let diff = (params.a - params.b).abs();
    let result = diff <= (params.rel_tol * params.a.abs().max(params.b.abs())).max(params.abs_tol);
    Ok(Value::Bool(result))
}

/// `math.nextafter(x, y)` — returns the next float after x towards y.
#[py_signature("nextafter(x: float, y: float, /)", error_name = "math.nextafter")]
fn math_nextafter(vm: &mut VM<'_>, params: &MathNextafterArgs) -> RunResult<Value> {
    Ok(Value::Float(libm::nextafter(params.x, params.y)))
}

/// `math.ulp(x)` — returns the value of the least significant bit of x.
///
/// For finite non-zero x, returns the smallest float `u` such that `x + u != x`.
/// Special cases: `ulp(nan)` returns nan, `ulp(inf)` returns inf.
#[py_signature("ulp(x: float, /)", error_name = "math.ulp")]
fn math_ulp(vm: &mut VM<'_>, params: &UlpArgs) -> RunResult<Value> {
    if params.x.is_nan() {
        return Ok(Value::Float(f64::NAN));
    }
    if params.x.is_infinite() {
        return Ok(Value::Float(f64::INFINITY));
    }
    let x = params.x.abs();
    if x == 0.0 {
        // CPython returns the smallest positive subnormal: 5e-324
        return Ok(Value::Float(f64::from_bits(1)));
    }
    // ULP = nextafter(x, inf) - x
    let next = libm::nextafter(x, f64::INFINITY);
    Ok(Value::Float(next - x))
}

// ==========================
// Trigonometric functions
// ==========================

/// `math.sin(x)` — returns the sine of x (in radians).
///
/// CPython 3.14 raises ValueError for infinity: "expected a finite input, got inf".
#[py_signature("sin(x: float, /)", error_name = "math.sin")]
fn math_sin(vm: &mut VM<'_>, params: &SinArgs) -> RunResult<Value> {
    require_finite(params.x)?;
    Ok(Value::Float(params.x.sin()))
}

/// `math.cos(x)` — returns the cosine of x (in radians).
#[py_signature("cos(x: float, /)", error_name = "math.cos")]
fn math_cos(vm: &mut VM<'_>, params: &CosArgs) -> RunResult<Value> {
    require_finite(params.x)?;
    Ok(Value::Float(params.x.cos()))
}

/// `math.tan(x)` — returns the tangent of x (in radians).
#[py_signature("tan(x: float, /)", error_name = "math.tan")]
fn math_tan(vm: &mut VM<'_>, params: &TanArgs) -> RunResult<Value> {
    require_finite(params.x)?;
    Ok(Value::Float(params.x.tan()))
}

/// `math.asin(x)` — returns the arc sine of x (in radians).
///
/// CPython 3.14: "expected a number in range from -1 up to 1, got <x>".
#[py_signature("asin(x: float, /)", error_name = "math.asin")]
fn math_asin(vm: &mut VM<'_>, params: &AsinArgs) -> RunResult<Value> {
    require_unit_range(params.x)?;
    Ok(Value::Float(params.x.asin()))
}

/// `math.acos(x)` — returns the arc cosine of x (in radians).
///
/// CPython 3.14: "expected a number in range from -1 up to 1, got <x>".
#[py_signature("acos(x: float, /)", error_name = "math.acos")]
fn math_acos(vm: &mut VM<'_>, params: &AcosArgs) -> RunResult<Value> {
    require_unit_range(params.x)?;
    Ok(Value::Float(params.x.acos()))
}

/// `math.atan(x)` — returns the arc tangent of x (in radians).
#[py_signature("atan(x: float, /)", error_name = "math.atan")]
fn math_atan(vm: &mut VM<'_>, params: &AtanArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.atan()))
}

/// `math.atan2(y, x)` — returns atan(y/x) in radians, using the signs of both
/// to determine the correct quadrant.
#[py_signature("atan2(y: float, x: float, /)", error_name = "math.atan2")]
fn math_atan2(vm: &mut VM<'_>, params: &MathAtan2Args) -> RunResult<Value> {
    Ok(Value::Float(params.y.atan2(params.x)))
}

// ==========================
// Hyperbolic functions
// ==========================

/// `math.sinh(x)` — returns the hyperbolic sine of x.
#[py_signature("sinh(x: float, /)", error_name = "math.sinh")]
fn math_sinh(vm: &mut VM<'_>, params: &SinhArgs) -> RunResult<Value> {
    let result = params.x.sinh();
    check_range_error(result, params.x)?;
    Ok(Value::Float(result))
}

/// `math.cosh(x)` — returns the hyperbolic cosine of x.
#[py_signature("cosh(x: float, /)", error_name = "math.cosh")]
fn math_cosh(vm: &mut VM<'_>, params: &CoshArgs) -> RunResult<Value> {
    let result = params.x.cosh();
    check_range_error(result, params.x)?;
    Ok(Value::Float(result))
}

/// `math.tanh(x)` — returns the hyperbolic tangent of x.
#[py_signature("tanh(x: float, /)", error_name = "math.tanh")]
fn math_tanh(vm: &mut VM<'_>, params: &TanhArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.tanh()))
}

/// `math.asinh(x)` — returns the inverse hyperbolic sine of x.
#[py_signature("asinh(x: float, /)", error_name = "math.asinh")]
fn math_asinh(vm: &mut VM<'_>, params: &AsinhArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.asinh()))
}

/// `math.acosh(x)` — returns the inverse hyperbolic cosine of x.
///
/// CPython 3.14: "expected argument value not less than 1, got <x>".
#[py_signature("acosh(x: float, /)", error_name = "math.acosh")]
fn math_acosh(vm: &mut VM<'_>, params: &AcoshArgs) -> RunResult<Value> {
    if params.x < 1.0 {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected argument value not less than 1, got {x:?}", x = params.x),
        )
        .into());
    }
    Ok(Value::Float(params.x.acosh()))
}

/// `math.atanh(x)` — returns the inverse hyperbolic tangent of x.
///
/// CPython 3.14: "expected a number between -1 and 1, got <x>".
#[py_signature("atanh(x: float, /)", error_name = "math.atanh")]
fn math_atanh(vm: &mut VM<'_>, params: &AtanhArgs) -> RunResult<Value> {
    if params.x <= -1.0 || params.x >= 1.0 {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected a number between -1 and 1, got {x:?}", x = params.x),
        )
        .into());
    }
    // The standard-library formula loses precision near -1.
    Ok(Value::Float(libm::atanh(params.x)))
}

// ==========================
// Angular conversion
// ==========================

/// `math.degrees(x)` — converts angle x from radians to degrees.
#[py_signature("degrees(x: float, /)", error_name = "math.degrees")]
fn math_degrees(vm: &mut VM<'_>, params: &DegreesArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.to_degrees()))
}

/// `math.radians(x)` — converts angle x from degrees to radians.
#[py_signature("radians(x: float, /)", error_name = "math.radians")]
fn math_radians(vm: &mut VM<'_>, params: &RadiansArgs) -> RunResult<Value> {
    Ok(Value::Float(params.x.to_radians()))
}

// ==========================
// Integer math
// ==========================

/// `math.factorial(n)` — returns n factorial.
///
/// Only accepts non-negative integers (and bools). Raises `ValueError` for
/// negative values, `TypeError` for non-integer types.
#[py_signature("factorial(n: int, /)", error_name = "math.factorial")]
fn math_factorial(vm: &mut VM<'_>, params: &FactorialArgs) -> RunResult<Value> {
    let tracker = &vm.heap.tracker;
    let result = params
        .n
        .view(vm)
        .with_value(vm, |n| factorial(&n, tracker), |n| factorial(n, tracker))?;
    Ok(LongInt::new(result).into_value(vm.heap))
}

/// `n!` with CPython's argument errors, shared by `factorial` and one-argument `perm`.
fn factorial(n: &(impl Signed + ToPrimitive), tracker: &ResourceTracker) -> RunResult<BigInt> {
    if n.is_negative() {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, "factorial() not defined for negative values").into(),
        );
    }
    let Some(n) = n.to_i64() else {
        return Err(SimpleException::new_msg(
            ExcType::OverflowError,
            "factorial() argument should not exceed 9223372036854775807",
        )
        .into());
    };
    let n = n.unsigned_abs();
    // `n!` has fewer than `n * bits(n)` bits.
    check_product_size(n.saturating_mul(u64::from(n.bit_width())), tracker)?;
    product_range(2, n, &mut 0, tracker)
}

/// Multiplies `lo..=hi` by binary splitting, so a big factorial combines factors of
/// similar size rather than one at a time against a growing product.
fn product_range(lo: u64, hi: u64, polls: &mut usize, tracker: &ResourceTracker) -> RunResult<BigInt> {
    if lo > hi {
        Ok(BigInt::one())
    } else if hi - lo < 32 {
        // Nothing here returns to the VM's dispatch checkpoint, so the loop polls the clock itself.
        tracker.check_time_every(*polls)?;
        *polls += 1;
        Ok((lo..=hi).fold(BigInt::one(), |product, i| product * i))
    } else {
        let mid = lo + (hi - lo) / 2;
        let (low, high) = (
            product_range(lo, mid, polls, tracker)?,
            product_range(mid + 1, hi, polls, tracker)?,
        );
        // Combining two halves is where the expensive multiplications are, so each one polls.
        tracker.check_time()?;
        Ok(low * high)
    }
}

/// `math.gcd(*integers)` — returns the greatest common divisor of the arguments.
///
/// Supports 0 or more arguments, matching CPython 3.9+. `gcd()` returns 0,
/// `gcd(n)` returns `abs(n)`, and for multiple args reduces pairwise.
/// The result is always non-negative.
#[py_signature("gcd(*integers)", error_name = "math.gcd")]
fn math_gcd(vm: &mut VM<'_>, params: &mut GcdArgs) -> RunResult<Value> {
    let mut result = BigInt::ZERO;
    for arg in &mut params.integers {
        defer_drop!(arg, vm);
        result = result.gcd(&value_to_bigint(arg, vm)?);
    }
    Ok(LongInt::new(result).into_value(vm.heap))
}

/// `math.lcm(*integers)` — returns the least common multiple of the arguments.
///
/// Supports 0 or more arguments, matching CPython 3.9+. `lcm()` returns 1,
/// `lcm(n)` returns `abs(n)`, and for multiple args reduces pairwise.
/// The result is always non-negative. Returns 0 if any argument is 0.
#[py_signature("lcm(*integers)", error_name = "math.lcm")]
fn math_lcm(vm: &mut VM<'_>, params: &mut LcmArgs) -> RunResult<Value> {
    let mut result = BigInt::one();
    for arg in &mut params.integers {
        defer_drop!(arg, vm);
        let n = value_to_bigint(arg, vm)?;
        // A zero anywhere makes the result zero, but every argument is still type-checked.
        if !result.is_zero() && !n.is_zero() {
            // `lcm` divides out the gcd, holding a quotient up to `result`'s size, then
            // multiplies: preflight both like `*`.
            check_mult_size(result.bits().saturating_mul(2), n.bits(), &vm.heap.tracker)?;
            result = result.lcm(&n);
        } else {
            result = BigInt::ZERO;
        }
    }
    Ok(LongInt::new(result).into_value(vm.heap))
}

/// `math.comb(n, k)` — returns the number of ways to choose k items from n.
///
/// Both arguments must be non-negative integers.
#[py_signature("comb(n, k, /)", error_name = "math.comb")]
fn math_comb(vm: &mut VM<'_>, params: &MathCombArgs) -> RunResult<Value> {
    let n = value_to_bigint(&params.n, vm)?;
    let k = value_to_bigint(&params.k, vm)?;

    if n.is_negative() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "n must be a non-negative integer").into());
    }
    if k.is_negative() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "k must be a non-negative integer").into());
    }
    if k > n {
        return Ok(Value::Int(0));
    }

    // C(n, k) == C(n, n - k): take the shorter product.
    let n_minus_k = &n - &k;
    let k = k.min(n_minus_k);
    let Some(k) = k.to_i64() else {
        return Err(SimpleException::new_msg(
            ExcType::OverflowError,
            "min(n - k, k) must not exceed 9223372036854775807",
        )
        .into());
    };
    let result = falling_product(&n, k.unsigned_abs(), true, &vm.heap.tracker)?;
    Ok(LongInt::new(result).into_value(vm.heap))
}

/// `n * (n - 1) * ... * (n - k + 1)`, the `k`-permutation count; with `binomial` each
/// step also divides by `i + 1`, so every partial product is the exact `C(n, i + 1)`.
fn falling_product(n: &BigInt, k: u64, binomial: bool, tracker: &ResourceTracker) -> RunResult<BigInt> {
    // The product has fewer than `k * bits(n)` bits; a binomial is also below `2**n`.
    let mut result_bits = k.saturating_mul(n.bits());
    if binomial && let Some(n) = n.to_u64() {
        result_bits = result_bits.min(n);
    }
    check_product_size(result_bits, tracker)?;
    let mut result = BigInt::one();
    for (polls, i) in (0..k).enumerate() {
        // Nothing here returns to the VM's dispatch checkpoint, so the loop polls the clock itself.
        tracker.check_time_every(polls)?;
        result *= n - i;
        if binomial {
            result /= i + 1;
        }
    }
    Ok(result)
}

/// `math.perm(n, k=None)` — returns the number of k-length permutations from n items.
///
/// Both arguments must be non-negative integers. When `k` is omitted, defaults to `n`
/// (i.e., `perm(n)` returns `n!`), matching CPython behavior.
#[py_signature("perm(n, k=..., /)")]
fn math_perm(vm: &mut VM<'_>, params: &PermArgs) -> RunResult<Value> {
    let n = value_to_bigint(&params.n, vm)?;
    // `perm(n)` is `n!`, argument errors included.
    let Some(k_val) = &params.k else {
        return Ok(LongInt::new(factorial(&n, &vm.heap.tracker)?).into_value(vm.heap));
    };
    let k = value_to_bigint(k_val, vm)?;

    if n.is_negative() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "n must be a non-negative integer").into());
    }
    if k.is_negative() {
        return Err(SimpleException::new_msg(ExcType::ValueError, "k must be a non-negative integer").into());
    }
    if k > n {
        return Ok(Value::Int(0));
    }
    let Some(k) = k.to_i64() else {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "k must not exceed 9223372036854775807").into());
    };
    let result = falling_product(&n, k.unsigned_abs(), false, &vm.heap.tracker)?;
    Ok(LongInt::new(result).into_value(vm.heap))
}

// ==========================
// Modular / decomposition
// ==========================

/// `math.fmod(x, y)` — returns x modulo y as a float.
///
/// Unlike `x % y`, the result has the same sign as x. Raises `ValueError`
/// when y is zero (CPython: "math domain error").
#[py_signature("fmod(x: float, y: float, /)", error_name = "math.fmod")]
fn math_fmod(vm: &mut VM<'_>, params: &MathFmodArgs) -> RunResult<Value> {
    if params.y == 0.0 || params.x.is_infinite() {
        // CPython raises for both fmod(x, 0) and fmod(inf, y)
        // but NaN inputs propagate
        if !params.x.is_nan() && !params.y.is_nan() {
            return Err(math_domain_error());
        }
    }
    Ok(Value::Float(params.x % params.y))
}

/// `math.remainder(x, y)` — IEEE 754 remainder of x with respect to y.
///
/// The result is `x - n*y` where n is the closest integer to `x/y`.
#[py_signature("remainder(x: float, y: float, /)", error_name = "math.remainder")]
fn math_remainder(vm: &mut VM<'_>, params: &MathRemainderArgs) -> RunResult<Value> {
    // NaN propagates
    if params.x.is_nan() || params.y.is_nan() {
        return Ok(Value::Float(f64::NAN));
    }
    if params.y == 0.0 {
        return Err(math_domain_error());
    }
    if params.x.is_infinite() {
        return Err(math_domain_error());
    }
    if params.y.is_infinite() {
        return Ok(Value::Float(params.x));
    }

    Ok(Value::Float(libm::remainder(params.x, params.y)))
}

/// `math.modf(x)` — returns the fractional and integer parts of x as a tuple.
///
/// Both values carry the sign of x. Returns `(fractional, integer)`.
#[py_signature("modf(x: float, /)", error_name = "math.modf")]
fn math_modf(vm: &mut VM<'_>, params: &ModfArgs) -> RunResult<Value> {
    let (fractional, integer) = libm::modf(params.x);
    let tuple = allocate_tuple(smallvec![Value::Float(fractional), Value::Float(integer)], vm.heap);
    Ok(tuple)
}

/// `math.frexp(x)` — returns (mantissa, exponent) such that `x == mantissa * 2**exponent`.
///
/// The mantissa is always in the range [0.5, 1.0) or zero.
/// Returns a tuple `(float, int)`.
#[py_signature("frexp(x: float, /)", error_name = "math.frexp")]
fn math_frexp(vm: &mut VM<'_>, params: &FrexpArgs) -> RunResult<Value> {
    let (m, exp) = libm::frexp(params.x);
    let tuple = allocate_tuple(smallvec![Value::Float(m), Value::Int(i64::from(exp))], vm.heap);
    Ok(tuple)
}

/// `math.ldexp(x, i)` — returns `x * 2**i`, the inverse of `frexp`.
///
/// Clamps the exponent to the `i32` range before calling `libm::ldexp`.
/// For finite nonzero inputs, exponents outside that range already overflow or underflow.
#[py_signature("ldexp(x: float, i: int, /)", error_name = "math.ldexp")]
fn math_ldexp(vm: &mut VM<'_>, params: &MathLdexpArgs) -> RunResult<Value> {
    // Special cases: inf/nan/zero pass through regardless of exponent
    if !params.x.is_finite() || params.x == 0.0 {
        return Ok(Value::Float(params.x));
    }

    let exp = params.i.view(vm).with_value(
        vm,
        |i| i32::try_from(i).unwrap_or(if i < 0 { i32::MIN } else { i32::MAX }),
        |i| {
            i.to_i32()
                .unwrap_or_else(|| if i.is_negative() { i32::MIN } else { i32::MAX })
        },
    );
    let result = libm::ldexp(params.x, exp);

    // If the result overflowed to infinity, CPython raises OverflowError
    if result.is_infinite() {
        return Err(math_range_error());
    }

    Ok(Value::Float(result))
}

// ==========================
// Special functions
// ==========================

/// `math.gamma(x)` — returns the Gamma function at x.
///
/// CPython 3.14 raises ValueError for non-positive integers:
/// "expected a noninteger or positive integer, got <x>".
#[py_signature("gamma(x: float, /)", error_name = "math.gamma")]
fn math_gamma(vm: &mut VM<'_>, params: &GammaArgs) -> RunResult<Value> {
    // CPython also rejects -inf for gamma (but not lgamma, where lgamma(-inf) = inf)
    if params.x == f64::NEG_INFINITY {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected a noninteger or positive integer, got {x:?}", x = params.x),
        )
        .into());
    }
    check_gamma_pole(params.x)?;

    let result = libm::tgamma(params.x);
    check_range_error(result, params.x)?;
    Ok(Value::Float(result))
}

/// `math.lgamma(x)` — returns the natural log of the absolute value of Gamma(x).
#[py_signature("lgamma(x: float, /)", error_name = "math.lgamma")]
fn math_lgamma(vm: &mut VM<'_>, params: &LgammaArgs) -> RunResult<Value> {
    check_gamma_pole(params.x)?;

    let result = libm::lgamma(params.x);
    check_range_error(result, params.x)?;
    Ok(Value::Float(result))
}

/// `math.erf(x)` — returns the error function at x.
#[py_signature("erf(x: float, /)", error_name = "math.erf")]
fn math_erf(vm: &mut VM<'_>, params: &ErfArgs) -> RunResult<Value> {
    Ok(Value::Float(libm::erf(params.x)))
}

/// `math.erfc(x)` — returns the complementary error function at x (1 - erf(x)).
///
/// More accurate than `1 - erf(x)` for large x.
#[py_signature("erfc(x: float, /)", error_name = "math.erfc")]
fn math_erfc(vm: &mut VM<'_>, params: &ErfcArgs) -> RunResult<Value> {
    Ok(Value::Float(libm::erfc(params.x)))
}

// ==========================
// Helper functions
// ==========================

/// Converts a `Value` to `f64`, raising `TypeError` if the value is not numeric.
///
/// Accepts `Float`, `Int`, `Bool`, long ints and `Decimal`, which convert like `float()`,
/// so long ints raise `OverflowError` beyond the float range. Other types raise a
/// `TypeError` with a message matching CPython's format: "must be real number, not <type>".
#[expect(
    clippy::cast_precision_loss,
    reason = "i64-to-f64 can lose precision for large integers (beyond 2^53), but this matches CPython's conversion semantics"
)]
fn value_to_float(value: &Value, vm: &VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => match value.as_long_int(vm) {
            Some(n) => bigint_to_f64_checked(n),
            // `Decimal.__float__`, as `PyFloat_AsDouble` calls it.
            None => match value.get_as::<decimal::Decimal>(vm.heap) {
                Some(d) => d.to_f64(),
                None => Err(ExcType::type_error_not_real_number(&value.py_type_name(vm))),
            },
        },
    }
}

/// Converts a `Value` to an arbitrary-precision integer, raising `TypeError` otherwise.
///
/// Accepts `Int`, `Bool` and long ints; the message matches CPython's `__index__` failure.
fn value_to_bigint(value: &Value, vm: &VM<'_>) -> RunResult<BigInt> {
    match value {
        Value::Int(n) => Ok(BigInt::from(*n)),
        Value::Bool(b) => Ok(BigInt::from(*b)),
        _ => value
            .as_long_int(vm)
            .cloned()
            .ok_or_else(|| not_an_integer_error(value, vm)),
    }
}

/// The `TypeError` CPython raises when `__index__` is missing on a `math` argument.
fn not_an_integer_error(value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format!(
        "'{}' object cannot be interpreted as an integer",
        value.py_type_name(vm)
    ))
}

/// Requires that a float is finite, raising ValueError if it's inf or nan.
///
/// CPython 3.14 uses "expected a finite input, got inf" for trig functions.
fn require_finite(f: f64) -> RunResult<()> {
    if f.is_infinite() {
        Err(SimpleException::new_msg(ExcType::ValueError, format!("expected a finite input, got {f:?}")).into())
    } else {
        Ok(())
    }
}
