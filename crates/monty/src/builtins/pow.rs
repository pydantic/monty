//! Implementation of the pow() builtin function.

use std::num::NonZero;

use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData},
    resource::{ResourceTracker, check_pow_size},
    types::{LongInt, PyTrait},
    value::Value,
};

/// Implementation of the pow() builtin function.
///
/// Returns base to the power exp. With three arguments, returns (base ** exp) % mod.
/// Handles negative exponents by returning a float.
pub fn builtin_pow(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let PowArgs { base, exp, modulus } = PowArgs::from_args(args, vm)?;
    defer_drop!(base, vm);
    defer_drop!(exp, vm);
    defer_drop!(modulus, vm);
    let base = normalize_bool(base);
    let exp = normalize_bool(exp);

    match modulus {
        Value::None => two_arg_pow(base, exp, vm),
        m => {
            let m = normalize_bool(m);
            // Three-argument pow: modular exponentiation
            match (base, exp, m) {
                (Value::Int(b), Value::Int(e), Value::Int(m_val)) => {
                    let Some(m_nz) = NonZero::new(*m_val) else {
                        return Err(
                            SimpleException::new_msg(ExcType::ValueError, "pow() 3rd argument cannot be 0").into(),
                        );
                    };
                    let Ok(e) = u64::try_from(*e) else {
                        debug_assert!(*e < 0, "i64 -> u64 succeeds for all non-negative values");
                        return Err(SimpleException::new_msg(
                            ExcType::ValueError,
                            "pow() 2nd argument cannot be negative when 3rd argument specified",
                        )
                        .into());
                    };
                    Ok(Value::Int(mod_pow(*b, e, m_nz)))
                }
                _ => Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    "pow() 3rd argument not allowed unless all arguments are integers",
                )
                .into()),
            }
        }
    }
}

/// `pow(base, exp[, mod])` — CPython accepts all three as positional-or-keyword
/// (and `mod` defaults to `None`), but Monty has not plumbed kwarg dispatch
/// through to the dispatch body yet. `kwargs_not_supported_yet` rejects
/// any kwarg with `NotImplementedError: pow() does not yet support
/// keyword arguments` (replacing the previous `TypeError: pow() takes no
/// keyword arguments` from `into_pos_only`) while the macro takes over
/// positional arity validation — the bespoke
/// `pow expected 2 or 3 arguments, got N` message becomes CPython's
/// `pow() takes at most 3 arguments (N given)` /
/// `pow() missing required argument 'X' (pos N)`. The `modulus` field
/// will be renamed to `r#mod` and lose the flag when kwargs are
/// implemented.
#[derive(FromArgs)]
#[from_args(name = "pow", c_error_named, at_most_total, kwargs_not_supported_yet)]
struct PowArgs {
    base: Value,
    exp: Value,
    #[from_args(default = Value::None)]
    modulus: Value,
}

/// Normalizes a `Bool` to its `Int` equivalent by reference.
///
/// Returns `&Value::Int(0)` or `&Value::Int(1)` for bools (using static storage),
/// and the original reference unchanged for all other types.
fn normalize_bool(value: &Value) -> &Value {
    static FALSE_INT: Value = Value::Int(0);
    static TRUE_INT: Value = Value::Int(1);
    match value {
        Value::Bool(false) => &FALSE_INT,
        Value::Bool(true) => &TRUE_INT,
        other => other,
    }
}

/// Computes (base^exp) % modulo using binary exponentiation.
///
/// Matches CPython for `|modulo| == 1`: the result is always `0`, including
/// the `exp == 0` corner case where the loop would otherwise leave
/// `result` at `1`.
fn mod_pow(base: i64, exp: u64, modulo: NonZero<i64>) -> i64 {
    let modulo = modulo.get();

    // The `|modulo| == 1` short-circuit is also load-bearing for panic safety:
    // without it, `base.rem_euclid(modulo)` panics when `base == i64::MIN` and
    // `modulo == -1` (the intermediate `i64::MIN / -1` overflows). Filtering
    // `modulo ∈ {-1, 1}` up front (combined with the `NonZero` guarantee)
    // ensures `rem_euclid` cannot panic.
    if modulo == 1 || modulo == -1 {
        return 0;
    }

    // `modulo` is now neither 0 nor ±1, so `rem_euclid` cannot panic and
    // `modulo_u` is in `2..=2^63`.
    let modulo_u = u128::from(modulo.unsigned_abs());
    let mut result: u128 = 1;
    let mut b = base.rem_euclid(modulo) as u128;
    let mut e = exp;

    while e > 0 {
        if e % 2 == 1 {
            result = (result * b) % modulo_u;
        }
        e /= 2;
        b = (b * b) % modulo_u;
    }

    // `result < modulo_u <= 2^63`, so the conversion to i64 always succeeds.
    let result_i64 = i64::try_from(result).expect("mod_pow result exceeds i64::MAX");
    if modulo < 0 && result_i64 > 0 {
        result_i64 + modulo
    } else {
        result_i64
    }
}

fn checked_pow_i64(mut base: i64, mut exp: u32) -> Option<i64> {
    let mut result: i64 = 1;

    while exp > 0 {
        if exp & 1 == 1 {
            result = result.checked_mul(base)?;
        }
        exp >>= 1;
        if exp > 0 {
            base = base.checked_mul(base)?;
        }
    }

    Some(result)
}

/// Implements two-argument pow with LongInt support.
///
/// On overflow, promotes to LongInt instead of returning an error.
fn two_arg_pow(base: &Value, exp: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    match (base, exp) {
        (Value::Int(b), Value::Int(e)) => int_pow_int(*b, *e, vm.heap),
        (Value::Int(b), Value::Ref(id)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
            int_pow_longint(*b, li.inner(), vm.heap)
        }
        (Value::Ref(id), Value::Int(e)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
            longint_pow_int(li.inner(), *e, vm.heap)
        }
        (Value::Ref(id1), Value::Ref(id2))
            if let HeapData::LongInt(b_li) = vm.heap.get(*id1)
                && let HeapData::LongInt(e_li) = vm.heap.get(*id2) =>
        {
            longint_pow_longint(b_li.inner(), e_li.inner(), vm.heap)
        }
        (Value::Float(b), Value::Float(e)) => {
            if *b == 0.0 && *e < 0.0 {
                Err(ExcType::zero_negative_power())
            } else {
                Ok(Value::Float(b.powf(*e)))
            }
        }
        (Value::Int(b), Value::Float(e)) => {
            if *b == 0 && *e < 0.0 {
                Err(ExcType::zero_negative_power())
            } else {
                Ok(Value::Float((*b as f64).powf(*e)))
            }
        }
        (Value::Float(b), Value::Int(e)) => {
            if *b == 0.0 && *e < 0 {
                Err(ExcType::zero_negative_power())
            } else if let Ok(exp_i32) = i32::try_from(*e) {
                Ok(Value::Float(b.powi(exp_i32)))
            } else {
                Ok(Value::Float(b.powf(*e as f64)))
            }
        }
        _ => Err(ExcType::binary_type_error(
            "** or pow()",
            base.py_type(vm),
            exp.py_type(vm),
        )),
    }
}

/// int ** int with LongInt promotion on overflow.
fn int_pow_int(b: i64, e: i64, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    if e < 0 {
        // Negative exponent returns float
        if b == 0 {
            return Err(ExcType::zero_negative_power());
        }
        Ok(Value::Float((b as f64).powf(e as f64)))
    } else if let Ok(exp_u32) = u32::try_from(e) {
        if let Some(v) = checked_pow_i64(b, exp_u32) {
            Ok(Value::Int(v))
        } else {
            // Overflow - promote to LongInt
            // Check size before computing to prevent DoS
            check_pow_size(i64_bits(b), u64::from(exp_u32), heap.tracker())?;
            let bi = BigInt::from(b).pow(exp_u32);
            Ok(LongInt::new(bi).into_value(heap)?)
        }
    } else {
        // Exponent too large for u32 - use BigInt for result
        // Safety: e >= 0 at this point
        #[expect(clippy::cast_sign_loss)]
        let exp_u64 = e as u64;
        // Check size before computing to prevent DoS
        check_pow_size(i64_bits(b), exp_u64, heap.tracker())?;
        let base_bi = BigInt::from(b);
        let bi = bigint_pow_large(&base_bi, exp_u64)?;
        Ok(LongInt::new(bi).into_value(heap)?)
    }
}

/// int ** LongInt with LongInt result.
fn int_pow_longint(b: i64, e: &BigInt, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    if b == 0 && e.is_negative() {
        return Err(ExcType::zero_negative_power());
    }
    if e.is_negative() {
        // Negative LongInt exponent: return float
        if let Some(e_f64) = e.to_f64() {
            Ok(Value::Float((b as f64).powf(e_f64)))
        } else {
            Ok(Value::Float(0.0))
        }
    } else if e.is_zero() {
        // x ** 0 = 1 for all x (including 0 ** 0 = 1)
        Ok(Value::Int(1))
    } else if b == 0 {
        Ok(Value::Int(0))
    } else if b == 1 {
        Ok(Value::Int(1))
    } else if b == -1 {
        // (-1) ** n = 1 if n is even, -1 if n is odd
        let is_even = (e % 2i32).is_zero();
        Ok(Value::Int(if is_even { 1 } else { -1 }))
    } else if let Some(exp_u32) = e.to_u32() {
        // Check size before computing to prevent DoS
        check_pow_size(i64_bits(b), u64::from(exp_u32), heap.tracker())?;
        let bi = BigInt::from(b).pow(exp_u32);
        Ok(LongInt::new(bi).into_value(heap)?)
    } else {
        // Exponent too large
        Err(ExcType::overflow_exponent_too_large())
    }
}

/// LongInt ** int with LongInt result.
fn longint_pow_int(b: &BigInt, e: i64, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    if b.is_zero() && e < 0 {
        return Err(ExcType::zero_negative_power());
    }
    if e < 0 {
        // Negative exponent: return float
        if let (Some(b_f64), Some(e_f64)) = (b.to_f64(), Some(e as f64)) {
            Ok(Value::Float(b_f64.powf(e_f64)))
        } else {
            Ok(Value::Float(0.0))
        }
    } else if let Ok(exp_u32) = u32::try_from(e) {
        // Check size before computing to prevent DoS
        check_pow_size(b.bits(), u64::from(exp_u32), heap.tracker())?;
        let bi = b.pow(exp_u32);
        Ok(LongInt::new(bi).into_value(heap)?)
    } else {
        // Exponent too large for u32
        // Safety: e >= 0 at this point
        #[expect(clippy::cast_sign_loss)]
        let exp_u64 = e as u64;
        // Check size before computing to prevent DoS
        check_pow_size(b.bits(), exp_u64, heap.tracker())?;
        let bi = bigint_pow_large(b, exp_u64)?;
        Ok(LongInt::new(bi).into_value(heap)?)
    }
}

/// LongInt ** LongInt with LongInt result.
fn longint_pow_longint(b: &BigInt, e: &BigInt, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    if b.is_zero() && e.is_negative() {
        return Err(ExcType::zero_negative_power());
    }
    if e.is_negative() {
        // Negative exponent: return float
        if let (Some(b_f64), Some(e_f64)) = (b.to_f64(), e.to_f64()) {
            Ok(Value::Float(b_f64.powf(e_f64)))
        } else {
            Ok(Value::Float(0.0))
        }
    } else if let Some(exp_u32) = e.to_u32() {
        // Check size before computing to prevent DoS
        check_pow_size(b.bits(), u64::from(exp_u32), heap.tracker())?;
        let bi = b.pow(exp_u32);
        Ok(LongInt::new(bi).into_value(heap)?)
    } else {
        // Exponent too large
        Err(ExcType::overflow_exponent_too_large())
    }
}

/// BigInt power for large exponents (> u32::MAX).
///
/// This handles exponents that are too large for the standard pow function.
/// For most bases, the result would be astronomically large, so we only handle
/// special cases (0, 1, -1) and return an error for others.
fn bigint_pow_large(base: &BigInt, exp: u64) -> RunResult<BigInt> {
    if base.is_zero() {
        Ok(BigInt::from(0))
    } else if *base == BigInt::from(1) {
        Ok(BigInt::from(1))
    } else if *base == BigInt::from(-1) {
        // (-1) ** n = 1 if n is even, -1 if n is odd
        if exp.is_multiple_of(2) {
            Ok(BigInt::from(1))
        } else {
            Ok(BigInt::from(-1))
        }
    } else {
        // For any other base, exponent > u32::MAX would produce an astronomically large result
        Err(ExcType::overflow_exponent_too_large())
    }
}

/// Computes the number of significant bits in an i64.
fn i64_bits(value: i64) -> u64 {
    if value == 0 {
        0
    } else {
        u64::from(64 - value.unsigned_abs().leading_zeros())
    }
}
