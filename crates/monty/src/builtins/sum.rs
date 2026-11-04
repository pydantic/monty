//! Implementation of the sum() builtin function.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, HeapData},
    types::{
        Complex, PyTrait, Type,
        complex::{Operand, operand},
    },
    value::{Value, ValueRead},
};

/// Argument shape for `sum(iterable, /, start=0)` — Argument Clinic in
/// CPython, so `start` is keyword-capable while `iterable` is positional-only.
/// `at_most_total` reproduces `_PyArg_UnpackKeywords`' total pre-count
/// (`sum() takes at most 2 arguments (3 given)`).
#[derive(FromArgs)]
#[from_args(name = "sum", at_most_total)]
struct SumArgs {
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(default = Value::Int(0))]
    start: Value,
}

/// Implementation of the sum() builtin function.
///
/// Sums the items of an iterable from left to right with an optional start value.
/// The default start value is 0. Str and bytes start values are explicitly
/// rejected, pointing at `''.join(seq)` / `b''.join(seq)` instead.
///
/// As in CPython's `builtin_sum_impl`, an int, float or complex total is kept unboxed
/// while the items fit it, and the float and complex loops use compensated summation:
/// `sum([0.1] * 10)` is `1.0`, not `0.9999999999999999`.
pub fn builtin_sum(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SumArgs { iterable, start } = SumArgs::from_args(args, vm)?;
    defer_drop_mut!(start, vm);

    let iter = iterable.into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    // Reject str/bytes start values - Python explicitly forbids these
    match start.py_type(vm) {
        Type::Str => return Err(ExcType::type_error_sum_start("strings", "''")),
        Type::Bytes => return Err(ExcType::type_error_sum_start("bytes", "b''")),
        _ => {}
    }
    // Take the start value out of its guard (dropping `None` is a no-op).
    let accumulator = mem::replace(start, Value::None);

    // DropGuard for accumulator: on success we extract it via into_inner(),
    // on any error path it's dropped automatically
    let mut acc_guard = DropGuard::new(accumulator, vm);
    let (accumulator, vm) = acc_guard.as_parts_mut();

    // A loop returns the total once the iterable is exhausted. Otherwise it stops at the
    // first item it can't take, adds it with `+` and leaves the result in `accumulator`,
    // where a later loop continues from it if the type matches.
    if let Value::Int(total) = *accumulator
        && let Some(total) = sum_ints(total, accumulator, &mut iter, vm)?
    {
        return Ok(total);
    }
    if let Value::Float(total) = *accumulator
        && let Some(total) = sum_floats(total, accumulator, &mut iter, vm)?
    {
        return Ok(total);
    }
    if let Value::Ref(id) = *accumulator
        && let HeapData::Complex(total) = *vm.heap.get(id)
        && let Some(total) = sum_complexes(total, accumulator, &mut iter, vm)?
    {
        return Ok(total);
    }

    // For any other total, including an int past `i64`, the rest is added with plain `+`
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);

        let new_value = accumulator.py_add(item, vm)?;
        let old = mem::replace(accumulator, new_value);
        old.drop_with(vm);
    }

    Ok(acc_guard.into_inner())
}

/// Adds ints and bools to an `i64` total, CPython's integer fast path.
///
/// Returns the total once the iterable is exhausted. The first item that isn't an int, or
/// would overflow `i64`, is added with `+` into `accumulator` instead, and `None` returned.
fn sum_ints<'h>(
    mut total: i64,
    accumulator: &mut Value,
    iter: &mut ValueRead<'h, '_>,
    vm: &mut VM<'h>,
) -> RunResult<Option<Value>> {
    while let Some(item) = iter.py_next(vm)? {
        let addend = match item {
            Value::Int(n) => Some(n),
            Value::Bool(b) => Some(i64::from(b)),
            _ => None,
        };
        if let Some(sum) = addend.and_then(|n| total.checked_add(n)) {
            total = sum;
        } else {
            defer_drop!(item, vm);
            let new_value = Value::Int(total).py_add(item, vm)?;
            mem::replace(accumulator, new_value).drop_with(vm);
            return Ok(None);
        }
    }
    Ok(Some(Value::Int(total)))
}

/// Adds floats, ints and bools to a float total with compensated summation.
///
/// Returns the total once the iterable is exhausted. Any other item, a complex included, is
/// added with `+` into `accumulator` instead, and `None` returned.
fn sum_floats<'h>(
    start: f64,
    accumulator: &mut Value,
    iter: &mut ValueRead<'h, '_>,
    vm: &mut VM<'h>,
) -> RunResult<Option<Value>> {
    let mut total = CompensatedSum::new(start);
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if let Some(Operand::Real(x)) = operand(item, vm)? {
            total.add(x);
        } else {
            let new_value = Value::Float(total.value()).py_add(item, vm)?;
            mem::replace(accumulator, new_value).drop_with(vm);
            return Ok(None);
        }
    }
    Ok(Some(Value::Float(total.value())))
}

/// Adds complexes, floats, ints and bools to a complex total, compensating each part.
///
/// Returns the total once the iterable is exhausted. Any other item is added with `+`
/// into `accumulator` instead, and `None` returned.
fn sum_complexes<'h>(
    start: Complex,
    accumulator: &mut Value,
    iter: &mut ValueRead<'h, '_>,
    vm: &mut VM<'h>,
) -> RunResult<Option<Value>> {
    let mut real = CompensatedSum::new(start.real);
    let mut imag = CompensatedSum::new(start.imag);
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        match operand(item, vm)? {
            Some(Operand::Complex(z)) => {
                real.add(z.real);
                imag.add(z.imag);
            }
            // A real item leaves the imaginary part alone, so `-0.0` stays negative.
            Some(Operand::Real(x)) => real.add(x),
            None => {
                let total = Complex::new(real.value(), imag.value()).into_value(vm.heap);
                defer_drop!(total, vm);
                let new_value = total.py_add(item, vm)?;
                mem::replace(accumulator, new_value).drop_with(vm);
                return Ok(None);
            }
        }
    }
    Ok(Some(Complex::new(real.value(), imag.value()).into_value(vm.heap)))
}

/// A float total with Neumaier's running compensation for the low-order bits that each
/// rounded addition loses, CPython's `CompensatedSum` (used by `sum()` since 3.12).
#[derive(Debug, Clone, Copy)]
struct CompensatedSum {
    /// The running total, rounded after every addition.
    hi: f64,
    /// The sum of the rounding errors `hi` has accumulated.
    lo: f64,
}

impl CompensatedSum {
    /// Starts a total at `start`, with no error yet.
    fn new(start: f64) -> Self {
        Self { hi: start, lo: 0.0 }
    }

    /// Adds `x`, collecting the error of rounding `hi + x` in `lo` (CPython's `cs_add`).
    fn add(&mut self, x: f64) {
        let t = self.hi + x;
        self.lo += if self.hi.abs() >= x.abs() {
            (self.hi - t) + x
        } else {
            (x - t) + self.hi
        };
        self.hi = t;
    }

    /// The compensated total, CPython's `cs_to_double`.
    ///
    /// A zero `lo` is skipped so that `-0.0` keeps its sign, and a non-finite one so that an
    /// overflowed total stays infinite instead of becoming `nan`.
    fn value(self) -> f64 {
        if self.lo != 0.0 && self.lo.is_finite() {
            self.hi + self.lo
        } else {
            self.hi
        }
    }
}
