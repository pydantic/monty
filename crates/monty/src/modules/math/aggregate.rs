//! Norms, accurate sums, and products for `math`.
//!
//! The numerical algorithms are broadly based on CPython's `Modules/mathmodule.c`:
//! Shewchuk's partials for `fsum`, a scaled compensated sum for `hypot`/`dist`,
//! and Neumaier's dot product for `sumprod`.

use std::mem;

use smallvec::SmallVec;

use super::value_to_float;
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::DropGuard,
    types::{PyTrait, Type, iter::collect_iterable},
    value::Value,
};

/// Coordinates remain owned until all conversions have succeeded.
#[derive(FromArgs)]
#[from_args(name = "hypot")]
struct HypotArgs {
    #[from_args(varargs)]
    coordinates: Vec<Value>,
}

/// Computes a norm without overflowing intermediate squares.
pub(super) fn hypot(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let args = args.reject_kwargs("math.hypot", vm.heap)?;
    let HypotArgs { coordinates } = HypotArgs::from_args(args, vm)?;
    defer_drop!(coordinates, vm);
    let mut values = coordinates
        .iter()
        .map(|value| value_to_float(value, vm).map(f64::abs))
        .collect::<RunResult<SmallVec<[f64; 16]>>>()?;
    Ok(Value::Float(vector_norm(&mut values)))
}

/// Positional-only points; CPython qualifies only the keyword error.
#[derive(FromArgs)]
#[from_args(name = "dist", style = unpack, kwarg_error_name = "math.dist")]
struct DistArgs {
    #[from_args(pos_only)]
    p: Value,
    #[from_args(pos_only)]
    q: Value,
}

/// Collects both points before checking dimensions and converting coordinates.
pub(super) fn dist(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DistArgs { p, q } = DistArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    defer_drop!(q, vm);
    let p = collect_iterable(p, vm)?;
    defer_drop!(p, vm);
    let q = collect_iterable(q, vm)?;
    defer_drop!(q, vm);
    if p.len() != q.len() {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "both points must have the same number of dimensions",
        )
        .into());
    }
    let mut differences = p
        .iter()
        .zip(q.iter())
        .map(|(p, q)| Ok((value_to_float(p, vm)? - value_to_float(q, vm)?).abs()))
        .collect::<RunResult<SmallVec<[f64; 16]>>>()?;
    Ok(Value::Float(vector_norm(&mut differences)))
}

/// Scales by powers of two, compensates squares and sums, then corrects the square root.
/// Inputs are magnitudes; the subnormal branch rescales them in place once.
fn vector_norm(values: &mut [f64]) -> f64 {
    let max = values.iter().copied().fold(0.0, f64::max);
    if max.is_infinite() {
        max
    } else if values.iter().any(|x| x.is_nan()) {
        f64::NAN
    } else if max == 0.0 || values.len() <= 1 {
        max
    } else {
        let (_, exponent) = libm::frexp(max);
        if exponent < -1023 {
            for value in values.iter_mut() {
                *value /= f64::MIN_POSITIVE;
            }
            f64::MIN_POSITIVE * vector_norm(values)
        } else {
            let scale = libm::ldexp(1.0, -exponent);
            // Starting at one makes the first operand dominate every square.
            let mut sum = 1.0;
            let mut square_errors = 0.0;
            let mut sum_errors = 0.0;
            for value in values {
                let x = *value * scale;
                let (square, error) = two_product(x, x);
                let (next, residual) = fast_two_sum(sum, square);
                sum = next;
                square_errors += error;
                sum_errors += residual;
            }
            let mut root = (sum - 1.0 + (square_errors + sum_errors)).sqrt();
            let (square, error) = two_product(-root, root);
            let (sum, residual) = fast_two_sum(sum, square);
            square_errors += error;
            sum_errors += residual;
            let correction = sum - 1.0 + (square_errors + sum_errors);
            root += correction / (2.0 * root);
            root / scale
        }
    }
}

/// Returns the rounded sum and its residual when `|a| >= |b|`.
fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    (sum, (a - sum) + b)
}

/// Returns a sum and its residual without requiring ordered magnitudes.
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    let recovered_b = sum - a;
    (sum, (a - (sum - recovered_b)) + (b - recovered_b))
}

/// Retains the product's rounding error with fused multiply-add.
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let product = a * b;
    (product, a.mul_add(b, -product))
}

/// Sums non-overlapping partials, preserving cancellation and half-even rounding.
pub(super) fn fsum(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let args = args.reject_kwargs("math.fsum", vm.heap)?;
    let iterable = args.get_one_arg("math.fsum", vm.heap)?;
    let iterator = iterable.into_py_iter(vm)?;
    defer_drop!(iterator, vm);
    let mut iterator = iterator.read(vm);
    let mut partials = SmallVec::<[f64; 32]>::new();
    let mut special_sum = 0.0;
    let mut inf_sum = 0.0;
    while let Some(item) = iterator.py_next(vm)? {
        defer_drop!(item, vm);
        let original = value_to_float(item, vm)?;
        let mut x = original;
        let mut retained = 0;
        for index in 0..partials.len() {
            let mut y = partials[index];
            if x.abs() < y.abs() {
                mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                partials[retained] = lo;
                retained += 1;
            }
            x = hi;
        }
        partials.truncate(retained);
        if x != 0.0 {
            if x.is_finite() {
                partials.push(x);
            } else {
                if original.is_finite() {
                    return Err(
                        SimpleException::new_msg(ExcType::OverflowError, "intermediate overflow in fsum").into(),
                    );
                }
                if original.is_infinite() {
                    inf_sum += original;
                }
                special_sum += original;
                partials.clear();
            }
        }
    }
    if special_sum == 0.0 {
        Ok(Value::Float(round_partials(&mut partials)))
    } else if inf_sum.is_nan() {
        Err(SimpleException::new_msg(ExcType::ValueError, "-inf + inf in fsum").into())
    } else {
        Ok(Value::Float(special_sum))
    }
}

/// Combines partials largest-first, using the remaining tail to resolve rounding ties.
#[expect(
    clippy::float_cmp,
    reason = "exact comparison detects a representable rounding correction"
)]
fn round_partials(partials: &mut SmallVec<[f64; 32]>) -> f64 {
    let mut hi = partials.pop().unwrap_or(0.0);
    let mut lo = 0.0;
    while let Some(y) = partials.pop() {
        let x = hi;
        hi = x + y;
        lo = y - (hi - x);
        if lo != 0.0 {
            break;
        }
    }
    if let Some(tail) = partials.last()
        && ((lo < 0.0 && *tail < 0.0) || (lo > 0.0 && *tail > 0.0))
    {
        let correction = lo * 2.0;
        let rounded = hi + correction;
        if correction == rounded - hi {
            hi = rounded;
        }
    }
    hi
}

/// `start` is keyword-only and is returned unchanged for an empty iterable.
#[derive(FromArgs)]
#[from_args(name = "prod", style = c_named, at_most_total)]
struct ProdArgs {
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(kw_only, default = Value::Int(1))]
    start: Value,
}

/// Multiplies through Python's numeric protocol, retaining arbitrary-size integer results.
pub(super) fn prod(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ProdArgs { iterable, start } = ProdArgs::from_args(args, vm)?;
    let mut result = DropGuard::new(start, vm);
    {
        let (total, vm) = result.as_parts_mut();
        let iterator = iterable.into_py_iter(vm)?;
        defer_drop!(iterator, vm);
        let mut iterator = iterator.read(vm);
        while let Some(item) = iterator.py_next(vm)? {
            defer_drop!(item, vm);
            let next = multiply(total, item, vm)?;
            mem::replace(total, next).drop_with(vm);
        }
    }
    Ok(result.into_inner())
}

/// Dot-product compensation keeps three floating-point components.
#[derive(Clone, Copy, Default)]
struct Triple {
    /// Leading rounded sum.
    hi: f64,
    /// Residual from the leading sum and product.
    lo: f64,
    /// Accumulated residuals from combining the low components.
    tiny: f64,
}

impl Triple {
    /// Adds a product with the three-component algorithm used by CPython's `sumprod`.
    fn add_product(self, a: f64, b: f64) -> Self {
        let (product, product_lo) = two_product(a, b);
        let (hi, sum_lo) = two_sum(self.hi, product);
        let (low_sum, low_error) = two_sum(self.lo, product_lo);
        let (lo, tiny) = two_sum(low_sum, sum_lo);
        Self {
            hi,
            lo,
            tiny: self.tiny + low_error + tiny,
        }
    }

    /// Rounds the low components into the leading sum.
    fn to_float(self) -> f64 {
        let (hi, lo) = two_sum(self.lo, self.hi);
        self.tiny + lo + hi
    }
}

/// Dot-product inputs are advanced in pairs, with strict length checking.
#[derive(FromArgs)]
#[from_args(name = "sumprod", style = unpack, kwarg_error_name = "math.sumprod")]
struct SumprodArgs {
    #[from_args(pos_only)]
    p: Value,
    #[from_args(pos_only)]
    q: Value,
}

/// Uses exact integer arithmetic and compensated float products before generic arithmetic.
pub(super) fn sumprod(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SumprodArgs { p, q } = SumprodArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    defer_drop!(q, vm);
    let p = p.py_iter(vm)?;
    defer_drop!(p, vm);
    let q = q.py_iter(vm)?;
    defer_drop!(q, vm);
    let mut p = p.read(vm);
    let mut q = q.read(vm);
    let mut result = DropGuard::new(Value::Int(0), vm);
    let (total, vm) = result.as_parts_mut();
    let mut integer_total = Some(0_i64);
    let mut float_total = Some(Triple::default());
    let mut float_used = false;
    loop {
        let a = p.py_next(vm)?;
        defer_drop!(a, vm);
        let b = q.py_next(vm)?;
        defer_drop!(b, vm);
        if a.is_some() != b.is_some() {
            return Err(SimpleException::new_msg(ExcType::ValueError, "Inputs are not the same length").into());
        }
        let pair = a.as_ref().zip(b.as_ref());
        if let Some(accumulator) = integer_total {
            if let Some((Value::Int(a), Value::Int(b))) = pair
                && let Some(next) = a.checked_mul(*b).and_then(|product| accumulator.checked_add(product))
            {
                integer_total = Some(next);
                continue;
            }
            integer_total = None;
            let next = add(total, &Value::Int(accumulator), vm)?;
            mem::replace(total, next).drop_with(vm);
        }
        if let Some(accumulator) = float_total {
            if let Some((a, b)) = pair
                && let Ok(Some((a, b))) = float_pair(a, b, vm)
            {
                let next = accumulator.add_product(a, b);
                if next.hi.is_finite() {
                    float_total = Some(next);
                    float_used = true;
                    continue;
                }
            }
            float_total = None;
            if float_used {
                let next = add(total, &Value::Float(accumulator.to_float()), vm)?;
                mem::replace(total, next).drop_with(vm);
            }
        }
        if let Some((a, b)) = pair {
            let product = multiply(a, b, vm)?;
            defer_drop!(product, vm);
            let next = add(total, product, vm)?;
            mem::replace(total, next).drop_with(vm);
        } else {
            break;
        }
    }
    Ok(result.into_inner())
}

/// The compensated path accepts float/float and float/int pairs, including booleans.
fn float_pair(a: &Value, b: &Value, vm: &VM<'_>) -> RunResult<Option<(f64, f64)>> {
    if (matches!(a, Value::Float(_)) && matches!(b.py_type(vm), Type::Float | Type::Int | Type::Bool))
        || (matches!(b, Value::Float(_)) && matches!(a.py_type(vm), Type::Float | Type::Int | Type::Bool))
    {
        Ok(Some((value_to_float(a, vm)?, value_to_float(b, vm)?)))
    } else {
        Ok(None)
    }
}

/// Checks mixed float/integer conversion before multiplication; generic arithmetic permits infinity.
fn multiply(a: &Value, b: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    if let Some((a, b)) = float_pair(a, b, vm)? {
        Ok(Value::Float(a * b))
    } else {
        a.py_mul(b, vm)
    }
}

/// Checks mixed float/integer conversion when flushing a dot-product accumulator.
fn add(a: &Value, b: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    if let Some((a, b)) = float_pair(a, b, vm)? {
        Ok(Value::Float(a + b))
    } else {
        a.py_add(b, vm)
    }
}

/// Three positional real numbers, converted in declaration order.
#[derive(FromArgs)]
#[from_args(name = "fma", style = unpack, kwarg_error_name = "math.fma")]
struct FmaArgs {
    #[from_args(pos_only)]
    x: Value,
    #[from_args(pos_only)]
    y: Value,
    #[from_args(pos_only)]
    z: Value,
}

/// Performs one rounding and raises CPython's errors for invalid operations and overflow.
pub(super) fn fma(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FmaArgs { x, y, z } = FmaArgs::from_args(args, vm)?;
    defer_drop!(x, vm);
    defer_drop!(y, vm);
    defer_drop!(z, vm);
    let x = value_to_float(x, vm)?;
    let y = value_to_float(y, vm)?;
    let z = value_to_float(z, vm)?;
    let result = x.mul_add(y, z);
    if result.is_nan() && !x.is_nan() && !y.is_nan() && !z.is_nan() {
        Err(SimpleException::new_msg(ExcType::ValueError, "invalid operation in fma").into())
    } else if result.is_infinite() && x.is_finite() && y.is_finite() && z.is_finite() {
        Err(SimpleException::new_msg(ExcType::OverflowError, "overflow in fma").into())
    } else {
        Ok(Value::Float(result))
    }
}
