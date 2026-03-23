//! Implementation of Python's `math` module.
//!
//! Provides mathematical functions and constants matching CPython 3.14 behavior
//! and error messages. All functions are pure computations that don't require
//! host involvement, so they return `Value` directly rather than `AttrCallResult`.
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
//!
//! ## Constants
//!
//! `pi`, `e`, `tau`, `inf`, `nan`

use num_bigint::BigInt;
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{LongInt, Module, PyTrait, allocate_tuple},
    value::Value,
};

// ==========================
// Shared constants and error helpers
// ==========================

/// Returns a `ValueError` with the standard CPython "math domain error" message.
fn math_domain_error() -> crate::exception_private::RunError {
    SimpleException::new_msg(ExcType::ValueError, "math domain error").into()
}

/// Returns an `OverflowError` with the standard CPython "math range error" message.
fn math_range_error() -> crate::exception_private::RunError {
    SimpleException::new_msg(ExcType::OverflowError, "math range error").into()
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum MathFunctions {
    // Rounding
    Floor,
    Ceil,
    Trunc,
    // Roots & powers
    Sqrt,
    Isqrt,
    Cbrt,
    Pow,
    Exp,
    Exp2,
    Expm1,
    // Logarithms
    Log,
    Log1p,
    Log2,
    Log10,
    // Float properties
    Fabs,
    Isnan,
    Isinf,
    Isfinite,
    Copysign,
    Isclose,
    Nextafter,
    Ulp,
    // Trigonometric
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    // Hyperbolic
    Sinh,
    Cosh,
    Tanh,
    Asinh,
    Acosh,
    Atanh,
    // Angular conversion
    Degrees,
    Radians,
    // Integer math
    Factorial,
    Gcd,
    Lcm,
    Comb,
    Perm,
    // Modular / decomposition
    Fmod,
    Remainder,
    Modf,
    Frexp,
    Ldexp,
    // Special functions
    Gamma,
    Lgamma,
    Erf,
    Erfc,
}

/// Creates the `math` module and allocates it on the heap.
///
/// Registers all math functions and constants (`pi`, `e`, `tau`, `inf`, `nan`)
/// matching CPython's `math` module. Functions are registered as
/// `ModuleFunctions::Math` variants.
///
/// # Returns
/// A `HeapId` pointing to the newly allocated module.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Math);

    // Register all math functions
    for (name, func) in MATH_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Math(*func)), vm);
    }

    // Constants
    module.set_attr(StaticStrings::Pi, Value::Float(std::f64::consts::PI), vm);
    module.set_attr(StaticStrings::MathE, Value::Float(std::f64::consts::E), vm);
    module.set_attr(StaticStrings::Tau, Value::Float(std::f64::consts::TAU), vm);
    module.set_attr(StaticStrings::MathInf, Value::Float(f64::INFINITY), vm);
    module.set_attr(StaticStrings::MathNan, Value::Float(f64::NAN), vm);

    vm.heap.allocate(HeapData::Module(module))
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
];

/// Dispatches a call to a math module function.
///
/// All math functions are pure computations and return `Value` directly.
pub(super) fn call(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    function: MathFunctions,
    args: ArgValues,
) -> RunResult<Value> {
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
    }
}

// ==========================
// Rounding functions
// ==========================

/// `math.floor(x)` — returns the largest integer less than or equal to x.
///
/// Accepts int, float, or bool. Returns int.
/// Raises `OverflowError` for infinity, `ValueError` for NaN.
fn math_floor(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.floor", vm.heap)?;
    defer_drop!(value, vm);

    match value {
        Value::Float(f) => float_to_int_checked(f.floor(), *f, vm.heap),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        _ => Err(ExcType::type_error(format!(
            "must be real number, not {}",
            value.py_type(vm)
        ))),
    }
}

/// `math.ceil(x)` — returns the smallest integer greater than or equal to x.
///
/// Accepts int, float, or bool. Returns int.
/// Raises `OverflowError` for infinity, `ValueError` for NaN.
fn math_ceil(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.ceil", vm.heap)?;
    defer_drop!(value, vm);

    match value {
        Value::Float(f) => float_to_int_checked(f.ceil(), *f, vm.heap),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        _ => Err(ExcType::type_error(format!(
            "must be real number, not {}",
            value.py_type(vm)
        ))),
    }
}

/// `math.trunc(x)` — truncates x to the nearest integer toward zero.
///
/// Accepts int, float, or bool. Returns int.
fn math_trunc(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.trunc", vm.heap)?;
    defer_drop!(value, vm);

    match value {
        Value::Float(f) => float_to_int_checked(f.trunc(), *f, vm.heap),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        _ => Err(ExcType::type_error(format!(
            "type {} doesn't define __trunc__ method",
            value.py_type(vm)
        ))),
    }
}

// ==========================
// Roots & powers
// ==========================

/// `math.sqrt(x)` — returns the square root of x.
///
/// Always returns a float. Raises `ValueError` for negative inputs with a
/// descriptive message matching CPython 3.14: "expected a nonnegative input, got <x>".
fn math_sqrt(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.sqrt", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    if f < 0.0 {
        Err(SimpleException::new_msg(ExcType::ValueError, format!("expected a nonnegative input, got {f:?}")).into())
    } else {
        Ok(Value::Float(f.sqrt()))
    }
}

/// `math.isqrt(n)` — returns the integer square root of a non-negative integer.
///
/// Returns the largest integer `r` such that `r * r <= n`.
/// Only accepts non-negative integers (and bools).
fn math_isqrt(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.isqrt", vm.heap)?;
    defer_drop!(value, vm);

    let n = value_to_int(value, vm)?;
    if n < 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "isqrt() argument must be nonnegative").into());
    }
    if n == 0 {
        return Ok(Value::Int(0));
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
    Ok(Value::Int(x))
}

/// `math.cbrt(x)` — returns the cube root of x.
///
/// Always returns a float. Unlike `sqrt`, works for negative inputs.
fn math_cbrt(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.cbrt", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(f.cbrt()))
}

/// `math.pow(x, y)` — returns x raised to the power y.
///
/// Always returns a float. Unlike the builtin `pow()`, does not support
/// three-argument modular exponentiation. Raises `ValueError` for
/// negative base with non-integer exponent.
fn math_pow(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, y_val) = args.get_two_args("math.pow", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(y_val, vm);

    let x = value_to_float(x_val, vm)?;
    let y = value_to_float(y_val, vm)?;
    let result = x.powf(y);
    // CPython raises ValueError for domain errors: 0**negative, negative**non-integer
    if result.is_nan() && !x.is_nan() && !y.is_nan() {
        return Err(math_domain_error());
    }
    if result.is_infinite() && x.is_finite() && y.is_finite() {
        // 0**negative is a domain error (ValueError), not overflow
        if x == 0.0 && y < 0.0 {
            return Err(math_domain_error());
        }
        return Err(math_range_error());
    }
    Ok(Value::Float(result))
}

/// `math.exp(x)` — returns e raised to the power x.
fn math_exp(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.exp", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    let result = f.exp();
    check_range_error(result, f)?;
    Ok(Value::Float(result))
}

/// `math.exp2(x)` — returns 2 raised to the power x.
fn math_exp2(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.exp2", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    let result = f.exp2();
    check_range_error(result, f)?;
    Ok(Value::Float(result))
}

/// `math.expm1(x)` — returns e**x - 1.
///
/// More accurate than `exp(x) - 1` for small values of x.
fn math_expm1(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.expm1", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    let result = f.exp_m1();
    check_range_error(result, f)?;
    Ok(Value::Float(result))
}

// ==========================
// Logarithms
// ==========================

/// `math.log(x[, base])` — returns the logarithm of x.
///
/// With one argument, returns the natural logarithm (base e).
/// With two arguments, returns `log(x) / log(base)`.
/// Raises `ValueError` for non-positive inputs (CPython 3.14: "expected a positive input").
fn math_log(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, base_val) = args.get_one_two_args("math.log", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(base_val, vm);

    let x = value_to_float(x_val, vm)?;
    if x <= 0.0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "expected a positive input").into());
    }

    match base_val {
        Some(base_v) => {
            let base = value_to_float(base_v, vm)?;
            // base == 1.0 causes division by zero in log(x)/log(base), matching
            // CPython which raises ZeroDivisionError for this case.
            #[expect(
                clippy::float_cmp,
                reason = "exact comparison with 1.0 is intentional — log(1.0) is exactly 0.0"
            )]
            if base == 1.0 {
                return Err(SimpleException::new_msg(ExcType::ZeroDivisionError, "division by zero").into());
            }
            if base <= 0.0 {
                return Err(SimpleException::new_msg(ExcType::ValueError, "expected a positive input").into());
            }
            Ok(Value::Float(x.ln() / base.ln()))
        }
        None => Ok(Value::Float(x.ln())),
    }
}

/// `math.log1p(x)` — returns the natural logarithm of 1 + x.
///
/// More accurate than `log(1 + x)` for small values of x.
/// CPython 3.14 raises ValueError with "expected argument value > -1, got <x>".
fn math_log1p(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.log1p", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    if f <= -1.0 {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("expected argument value > -1, got {f:?}")).into(),
        );
    }
    Ok(Value::Float(f.ln_1p()))
}

/// `math.log2(x)` — returns the base-2 logarithm of x.
///
/// Returns `inf` for positive infinity, `nan` for NaN.
/// Raises `ValueError` for non-positive finite inputs.
fn math_log2(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.log2", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    if f <= 0.0 {
        Err(SimpleException::new_msg(ExcType::ValueError, "expected a positive input").into())
    } else {
        Ok(Value::Float(f.log2()))
    }
}

/// `math.log10(x)` — returns the base-10 logarithm of x.
///
/// Returns `inf` for positive infinity, `nan` for NaN.
/// Raises `ValueError` for non-positive finite inputs.
fn math_log10(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.log10", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    if f <= 0.0 {
        Err(SimpleException::new_msg(ExcType::ValueError, "expected a positive input").into())
    } else {
        Ok(Value::Float(f.log10()))
    }
}

// ==========================
// Float properties
// ==========================

/// `math.fabs(x)` — returns the absolute value as a float.
///
/// Unlike the builtin `abs()`, always returns a float.
fn math_fabs(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.fabs", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(f.abs()))
}

/// `math.isnan(x)` — returns True if x is NaN.
fn math_isnan(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.isnan", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Bool(f.is_nan()))
}

/// `math.isinf(x)` — returns True if x is positive or negative infinity.
fn math_isinf(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.isinf", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Bool(f.is_infinite()))
}

/// `math.isfinite(x)` — returns True if x is neither infinity nor NaN.
fn math_isfinite(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.isfinite", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Bool(f.is_finite()))
}

/// `math.copysign(x, y)` — returns x with the sign of y.
///
/// Always returns a float.
fn math_copysign(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, y_val) = args.get_two_args("math.copysign", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(y_val, vm);

    let x = value_to_float(x_val, vm)?;
    let y = value_to_float(y_val, vm)?;
    Ok(Value::Float(x.copysign(y)))
}

/// `math.isclose(a, b, *, rel_tol=1e-9, abs_tol=0.0)` — returns True if a and b are close.
///
/// Supports keyword-only `rel_tol` and `abs_tol` parameters matching CPython.
/// Raises `ValueError` if either tolerance is negative.
fn math_isclose(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (positional, kwargs) = args.into_parts();
    defer_drop_mut!(positional, vm);

    // Extract exactly two positional args
    let Some(a_val) = positional.next() else {
        return Err(ExcType::type_error_at_least("math.isclose", 2, 0));
    };
    defer_drop!(a_val, vm);
    let Some(b_val) = positional.next() else {
        return Err(ExcType::type_error_at_least("math.isclose", 2, 1));
    };
    defer_drop!(b_val, vm);
    if positional.len() > 0 {
        return Err(ExcType::type_error_at_most("math.isclose", 2, 2 + positional.len()));
    }

    let a = value_to_float(a_val, vm)?;
    let b = value_to_float(b_val, vm)?;

    // Parse optional keyword arguments rel_tol and abs_tol
    let (rel_tol, abs_tol) = extract_isclose_kwargs(kwargs, vm)?;

    if rel_tol < 0.0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "tolerances must be non-negative").into());
    }
    if abs_tol < 0.0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "tolerances must be non-negative").into());
    }

    // Exact equality check matches CPython's isclose() behavior — two identical
    // values (including infinities) are always considered close.
    #[expect(
        clippy::float_cmp,
        reason = "exact equality check matches CPython's isclose() semantics"
    )]
    if a == b {
        return Ok(Value::Bool(true));
    }
    if a.is_infinite() || b.is_infinite() {
        return Ok(Value::Bool(false));
    }
    if a.is_nan() || b.is_nan() {
        return Ok(Value::Bool(false));
    }

    let diff = (a - b).abs();
    let result = diff <= (rel_tol * a.abs().max(b.abs())).max(abs_tol);
    Ok(Value::Bool(result))
}

/// Extracts `rel_tol` and `abs_tol` keyword arguments for `math.isclose`.
///
/// Returns `(rel_tol, abs_tol)` with defaults of `(1e-9, 0.0)`.
fn extract_isclose_kwargs(
    kwargs: crate::args::KwargsValues,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<(f64, f64)> {
    let mut rel_tol: f64 = 1e-9;
    let mut abs_tol: f64 = 0.0;

    if kwargs.is_empty() {
        return Ok((rel_tol, abs_tol));
    }

    for (key, value) in kwargs {
        defer_drop!(key, vm);
        defer_drop!(value, vm);

        let Some(keyword_name) = key.as_either_str(vm.heap) else {
            return Err(ExcType::type_error("keywords must be strings"));
        };

        let key_str = keyword_name.as_str(vm.interns);
        match key_str {
            "rel_tol" => {
                rel_tol = value_to_float(value, vm)?;
            }
            "abs_tol" => {
                abs_tol = value_to_float(value, vm)?;
            }
            other => {
                return Err(ExcType::type_error(format!(
                    "isclose() got an unexpected keyword argument '{other}'"
                )));
            }
        }
    }

    Ok((rel_tol, abs_tol))
}

/// `math.nextafter(x, y)` — returns the next float after x towards y.
fn math_nextafter(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, y_val) = args.get_two_args("math.nextafter", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(y_val, vm);

    let x = value_to_float(x_val, vm)?;
    let y = value_to_float(y_val, vm)?;

    Ok(Value::Float(libm::nextafter(x, y)))
}

/// `math.ulp(x)` — returns the value of the least significant bit of x.
///
/// For finite non-zero x, returns the smallest float `u` such that `x + u != x`.
/// Special cases: `ulp(nan)` returns nan, `ulp(inf)` returns inf.
fn math_ulp(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.ulp", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    if f.is_nan() {
        return Ok(Value::Float(f64::NAN));
    }
    if f.is_infinite() {
        return Ok(Value::Float(f64::INFINITY));
    }
    let f = f.abs();
    if f == 0.0 {
        // CPython returns the smallest positive subnormal: 5e-324
        return Ok(Value::Float(f64::from_bits(1)));
    }
    // ULP = nextafter(f, inf) - f
    let next = libm::nextafter(f, f64::INFINITY);
    Ok(Value::Float(next - f))
}

// ==========================
// Trigonometric functions
// ==========================

/// `math.sin(x)` — returns the sine of x (in radians).
///
/// CPython 3.14 raises ValueError for infinity: "expected a finite input, got inf".
fn math_sin(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.sin", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    require_finite(f)?;
    Ok(Value::Float(f.sin()))
}

/// `math.cos(x)` — returns the cosine of x (in radians).
fn math_cos(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.cos", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    require_finite(f)?;
    Ok(Value::Float(f.cos()))
}

/// `math.tan(x)` — returns the tangent of x (in radians).
fn math_tan(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.tan", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    require_finite(f)?;
    Ok(Value::Float(f.tan()))
}

/// `math.asin(x)` — returns the arc sine of x (in radians).
///
/// CPython 3.14: "expected a number in range from -1 up to 1, got <x>".
fn math_asin(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.asin", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    require_unit_range(f)?;
    Ok(Value::Float(f.asin()))
}

/// `math.acos(x)` — returns the arc cosine of x (in radians).
///
/// CPython 3.14: "expected a number in range from -1 up to 1, got <x>".
fn math_acos(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.acos", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    require_unit_range(f)?;
    Ok(Value::Float(f.acos()))
}

/// `math.atan(x)` — returns the arc tangent of x (in radians).
fn math_atan(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.atan", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(f.atan()))
}

/// `math.atan2(y, x)` — returns atan(y/x) in radians, using the signs of both
/// to determine the correct quadrant.
fn math_atan2(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (y_val, x_val) = args.get_two_args("math.atan2", vm.heap)?;
    defer_drop!(y_val, vm);
    defer_drop!(x_val, vm);

    let y = value_to_float(y_val, vm)?;
    let x = value_to_float(x_val, vm)?;
    Ok(Value::Float(y.atan2(x)))
}

// ==========================
// Hyperbolic functions
// ==========================

/// `math.sinh(x)` — returns the hyperbolic sine of x.
fn math_sinh(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.sinh", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    let result = f.sinh();
    check_range_error(result, f)?;
    Ok(Value::Float(result))
}

/// `math.cosh(x)` — returns the hyperbolic cosine of x.
fn math_cosh(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.cosh", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    let result = f.cosh();
    check_range_error(result, f)?;
    Ok(Value::Float(result))
}

/// `math.tanh(x)` — returns the hyperbolic tangent of x.
fn math_tanh(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.tanh", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(f.tanh()))
}

/// `math.asinh(x)` — returns the inverse hyperbolic sine of x.
fn math_asinh(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.asinh", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(f.asinh()))
}

/// `math.acosh(x)` — returns the inverse hyperbolic cosine of x.
///
/// CPython 3.14: "expected argument value not less than 1, got <x>".
fn math_acosh(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.acosh", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    if f < 1.0 {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected argument value not less than 1, got {f:?}"),
        )
        .into());
    }
    Ok(Value::Float(f.acosh()))
}

/// `math.atanh(x)` — returns the inverse hyperbolic tangent of x.
///
/// CPython 3.14: "expected a number between -1 and 1, got <x>".
fn math_atanh(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.atanh", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    if f <= -1.0 || f >= 1.0 {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected a number between -1 and 1, got {f:?}"),
        )
        .into());
    }
    Ok(Value::Float(f.atanh()))
}

// ==========================
// Angular conversion
// ==========================

/// `math.degrees(x)` — converts angle x from radians to degrees.
fn math_degrees(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.degrees", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(f.to_degrees()))
}

/// `math.radians(x)` — converts angle x from degrees to radians.
fn math_radians(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.radians", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(f.to_radians()))
}

// ==========================
// Integer math
// ==========================

/// `math.factorial(n)` — returns n factorial.
///
/// Only accepts non-negative integers (and bools). Raises `ValueError` for
/// negative values, `TypeError` for non-integer types.
fn math_factorial(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.factorial", vm.heap)?;
    defer_drop!(value, vm);

    let n = match value {
        Value::Int(n) => *n,
        Value::Bool(b) => i64::from(*b),
        _ => {
            return Err(ExcType::type_error(format!(
                "'{}' object cannot be interpreted as an integer",
                value.py_type(vm)
            )));
        }
    };

    if n < 0 {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, "factorial() not defined for negative values").into(),
        );
    }

    // Compute factorial iteratively
    let mut result: i64 = 1;
    for i in 2..=n {
        match result.checked_mul(i) {
            Some(v) => result = v,
            None => {
                // Overflow — for simplicity, return an error for very large factorials
                // since we don't have LongInt factorial support yet
                return Err(
                    SimpleException::new_msg(ExcType::OverflowError, "int too large to convert to factorial").into(),
                );
            }
        }
    }
    Ok(Value::Int(result))
}

/// `math.gcd(*integers)` — returns the greatest common divisor of the arguments.
///
/// Supports 0 or more arguments, matching CPython 3.9+. `gcd()` returns 0,
/// `gcd(n)` returns `abs(n)`, and for multiple args reduces pairwise.
/// The result is always non-negative.
fn math_gcd(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let positional = args.into_pos_only("math.gcd", vm.heap)?;
    defer_drop_mut!(positional, vm);

    let mut result: u64 = 0;
    for arg in positional.by_ref() {
        defer_drop!(arg, vm);
        let n = value_to_int(arg, vm)?;
        result = gcd(result, n.unsigned_abs());
    }
    u64_to_value(result, vm.heap)
}

/// `math.lcm(*integers)` — returns the least common multiple of the arguments.
///
/// Supports 0 or more arguments, matching CPython 3.9+. `lcm()` returns 1,
/// `lcm(n)` returns `abs(n)`, and for multiple args reduces pairwise.
/// The result is always non-negative. Returns 0 if any argument is 0.
fn math_lcm(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let positional = args.into_pos_only("math.lcm", vm.heap)?;
    defer_drop_mut!(positional, vm);

    let mut result: u64 = 1;
    for arg in positional.by_ref() {
        defer_drop!(arg, vm);
        let n = value_to_int(arg, vm)?;
        let abs_n = n.unsigned_abs();
        if abs_n == 0 {
            return Ok(Value::Int(0));
        }
        let g = gcd(result, abs_n);
        // lcm(a, b) = |a| / gcd(a,b) * |b| — dividing first avoids intermediate overflow
        result = (result / g)
            .checked_mul(abs_n)
            .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, "integer overflow in lcm"))?;
    }
    u64_to_value(result, vm.heap)
}

/// `math.comb(n, k)` — returns the number of ways to choose k items from n.
///
/// Both arguments must be non-negative integers.
fn math_comb(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (n_val, k_val) = args.get_two_args("math.comb", vm.heap)?;
    defer_drop!(n_val, vm);
    defer_drop!(k_val, vm);

    let n = value_to_int(n_val, vm)?;
    let k = value_to_int(k_val, vm)?;

    if n < 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "n must be a non-negative integer").into());
    }
    if k < 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "k must be a non-negative integer").into());
    }
    if k > n {
        return Ok(Value::Int(0));
    }

    // Use the smaller of k and n-k for efficiency: C(n, k) = C(n, n-k)
    let k = k.min(n - k);
    let mut result: i64 = 1;
    for i in 0..k {
        // Use GCD reduction to keep intermediates small:
        // result = result * (n - i) / (i + 1)
        // By dividing both numerator and denominator by their GCD first,
        // we reduce the chance of overflow in the multiplication step.
        let mut numerator = n - i;
        let mut denominator = i + 1;
        #[expect(clippy::cast_sign_loss, reason = "both values are known non-negative at this point")]
        let g = gcd(numerator as u64, denominator as u64).cast_signed();
        numerator /= g;
        denominator /= g;
        // Also reduce against the running result
        #[expect(clippy::cast_sign_loss, reason = "result and denominator are known non-negative")]
        let g2 = gcd(result as u64, denominator as u64).cast_signed();
        result /= g2;
        denominator /= g2;
        debug_assert!(denominator == 1, "denominator should be 1 after GCD reduction in comb");
        match result.checked_mul(numerator) {
            Some(v) => result = v,
            None => {
                return Err(SimpleException::new_msg(ExcType::OverflowError, "integer overflow in comb").into());
            }
        }
    }
    Ok(Value::Int(result))
}

/// `math.perm(n, k=None)` — returns the number of k-length permutations from n items.
///
/// Both arguments must be non-negative integers. When `k` is omitted, defaults to `n`
/// (i.e., `perm(n)` returns `n!`), matching CPython behavior.
fn math_perm(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (n_val, k_val) = args.get_one_two_args("math.perm", vm.heap)?;
    defer_drop!(n_val, vm);

    let n = value_to_int(n_val, vm)?;
    let k_explicit = k_val.is_some();
    let k = match k_val {
        Some(kv) => {
            defer_drop!(kv, vm);
            value_to_int(kv, vm)?
        }
        None => n,
    };

    if n < 0 {
        // When called as perm(n) without k, CPython uses the factorial error message
        let msg = if k_explicit {
            "n must be a non-negative integer"
        } else {
            "factorial() not defined for negative values"
        };
        return Err(SimpleException::new_msg(ExcType::ValueError, msg).into());
    }
    if k < 0 {
        return Err(SimpleException::new_msg(ExcType::ValueError, "k must be a non-negative integer").into());
    }
    if k > n {
        return Ok(Value::Int(0));
    }

    let mut result: i64 = 1;
    for i in 0..k {
        match result.checked_mul(n - i) {
            Some(v) => result = v,
            None => {
                return Err(SimpleException::new_msg(ExcType::OverflowError, "integer overflow in perm").into());
            }
        }
    }
    Ok(Value::Int(result))
}

// ==========================
// Modular / decomposition
// ==========================

/// `math.fmod(x, y)` — returns x modulo y as a float.
///
/// Unlike `x % y`, the result has the same sign as x. Raises `ValueError`
/// when y is zero (CPython: "math domain error").
fn math_fmod(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, y_val) = args.get_two_args("math.fmod", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(y_val, vm);

    let x = value_to_float(x_val, vm)?;
    let y = value_to_float(y_val, vm)?;

    if y == 0.0 || x.is_infinite() {
        // CPython raises for both fmod(x, 0) and fmod(inf, y)
        // but NaN inputs propagate
        if !x.is_nan() && !y.is_nan() {
            return Err(math_domain_error());
        }
    }
    Ok(Value::Float(x % y))
}

/// `math.remainder(x, y)` — IEEE 754 remainder of x with respect to y.
///
/// The result is `x - n*y` where n is the closest integer to `x/y`.
fn math_remainder(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, y_val) = args.get_two_args("math.remainder", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(y_val, vm);

    let x = value_to_float(x_val, vm)?;
    let y = value_to_float(y_val, vm)?;

    // NaN propagates
    if x.is_nan() || y.is_nan() {
        return Ok(Value::Float(f64::NAN));
    }
    if y == 0.0 {
        return Err(math_domain_error());
    }
    if x.is_infinite() {
        return Err(math_domain_error());
    }
    if y.is_infinite() {
        return Ok(Value::Float(x));
    }

    Ok(Value::Float(libm::remainder(x, y)))
}

/// `math.modf(x)` — returns the fractional and integer parts of x as a tuple.
///
/// Both values carry the sign of x. Returns `(fractional, integer)`.
fn math_modf(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.modf", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    let (fractional, integer) = libm::modf(f);
    let tuple = allocate_tuple(smallvec![Value::Float(fractional), Value::Float(integer)], vm.heap)?;
    Ok(tuple)
}

/// `math.frexp(x)` — returns (mantissa, exponent) such that `x == mantissa * 2**exponent`.
///
/// The mantissa is always in the range [0.5, 1.0) or zero.
/// Returns a tuple `(float, int)`.
fn math_frexp(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.frexp", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    let (m, exp) = libm::frexp(f);
    let tuple = allocate_tuple(smallvec![Value::Float(m), Value::Int(i64::from(exp))], vm.heap)?;
    Ok(tuple)
}

/// `math.ldexp(x, i)` — returns `x * 2**i`, the inverse of `frexp`.
///
/// Clamps the exponent to `i32` range before calling `libm::ldexp`, which is safe
/// because IEEE 754 double exponents only span -1074 to +1023 — any `i64` outside
/// `i32` range would trivially overflow or underflow anyway.
fn math_ldexp(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (x_val, i_val) = args.get_two_args("math.ldexp", vm.heap)?;
    defer_drop!(x_val, vm);
    defer_drop!(i_val, vm);

    let x = value_to_float(x_val, vm)?;
    let i = value_to_int(i_val, vm)?;

    // Special cases: inf/nan/zero pass through regardless of exponent
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return Ok(Value::Float(x));
    }

    // Clamp i64 to i32 range — exponents beyond ±2 billion trivially overflow/underflow
    #[expect(clippy::cast_possible_truncation, reason = "clamped to i32 range first")]
    let exp = i.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let result = libm::ldexp(x, exp);

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
fn math_gamma(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.gamma", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    // CPython also rejects -inf for gamma (but not lgamma, where lgamma(-inf) = inf)
    if f == f64::NEG_INFINITY {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("expected a noninteger or positive integer, got {f:?}"),
        )
        .into());
    }
    check_gamma_pole(f)?;

    let result = libm::tgamma(f);
    check_range_error(result, f)?;
    Ok(Value::Float(result))
}

/// `math.lgamma(x)` — returns the natural log of the absolute value of Gamma(x).
fn math_lgamma(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.lgamma", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    check_gamma_pole(f)?;

    let result = libm::lgamma(f);
    check_range_error(result, f)?;
    Ok(Value::Float(result))
}

/// `math.erf(x)` — returns the error function at x.
fn math_erf(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.erf", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(libm::erf(f)))
}

/// `math.erfc(x)` — returns the complementary error function at x (1 - erf(x)).
///
/// More accurate than `1 - erf(x)` for large x.
fn math_erfc(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("math.erfc", vm.heap)?;
    defer_drop!(value, vm);

    let f = value_to_float(value, vm)?;
    Ok(Value::Float(libm::erfc(f)))
}

// ==========================
// Helper functions
// ==========================

/// Converts a rounded float to an integer `Value`, checking for infinity/NaN.
///
/// `rounded` is the already-rounded float value (e.g., from `floor()`, `ceil()`, `trunc()`).
/// `original` is the original input float, used only to determine the error type:
/// infinity produces `OverflowError`, NaN produces `ValueError`.
///
/// For finite values outside the i64 range, promotes to `LongInt` to match CPython's
/// behavior of returning arbitrary-precision integers from `math.floor`/`ceil`/`trunc`.
fn float_to_int_checked(rounded: f64, original: f64, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    if original.is_infinite() {
        Err(SimpleException::new_msg(ExcType::OverflowError, "cannot convert float infinity to integer").into())
    } else if original.is_nan() {
        Err(SimpleException::new_msg(ExcType::ValueError, "cannot convert float NaN to integer").into())
    } else if rounded >= i64::MIN as f64 && rounded < i64::MAX as f64 {
        // Note: `i64::MAX as f64` rounds up to 2^63 (9223372036854775808.0), so we use
        // strict less-than to exclude that value. `i64::MIN as f64` is exact (-2^63).
        #[expect(
            clippy::cast_possible_truncation,
            reason = "intentional: value is within i64 range after bounds check"
        )]
        let result = rounded as i64;
        Ok(Value::Int(result))
    } else {
        // Value exceeds i64 range — promote to LongInt.
        // Format with no decimal places and parse as BigInt. This is correct because
        // `rounded` is already an integer-valued float from floor/ceil/trunc.
        let s = format!("{rounded:.0}");
        let bi = s
            .parse::<BigInt>()
            .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "float too large to convert to integer"))?;
        Ok(LongInt::new(bi).into_value(heap)?)
    }
}

/// Converts a `Value` to `f64`, raising `TypeError` if the value is not numeric.
///
/// Accepts `Float`, `Int`, and `Bool` values. For other types, raises a `TypeError`
/// with a message matching CPython's format: "must be real number, not <type>".
#[expect(
    clippy::cast_precision_loss,
    reason = "i64-to-f64 can lose precision for large integers (beyond 2^53), but this matches CPython's conversion semantics"
)]
fn value_to_float(value: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(ExcType::type_error(format!(
            "must be real number, not {}",
            value.py_type(vm)
        ))),
    }
}

/// Converts a `Value` to `i64`, raising `TypeError` if the value is not an integer.
///
/// Accepts `Int` and `Bool` values. For other types, raises a `TypeError`
/// with a message matching CPython's format.
fn value_to_int(value: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<i64> {
    match value {
        Value::Int(n) => Ok(*n),
        Value::Bool(b) => Ok(i64::from(*b)),
        _ => Err(ExcType::type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            value.py_type(vm)
        ))),
    }
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

/// Euclidean GCD algorithm for unsigned 64-bit integers.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Converts a `u64` result to a `Value`, promoting to `LongInt` if it exceeds `i64::MAX`.
///
/// This is needed for operations like `gcd(i64::MIN, 0)` where the unsigned result
/// (`2^63`) doesn't fit in a signed `i64`.
fn u64_to_value(n: u64, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    if let Ok(signed) = i64::try_from(n) {
        Ok(Value::Int(signed))
    } else {
        Ok(LongInt::new(BigInt::from(n)).into_value(heap)?)
    }
}
