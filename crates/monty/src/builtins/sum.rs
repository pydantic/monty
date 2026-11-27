//! Implementation of the sum() builtin function.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::DropGuard,
    py_value::PyValue,
    types::{Complex, PyTrait, Type},
    value::Value,
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
pub fn builtin_sum(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SumArgs { iterable, start } = SumArgs::from_args(args, vm)?;
    defer_drop_mut!(start, vm);

    let iter = iterable.into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    // Reject str/bytes start values - Python explicitly forbids these
    match start.py_type(vm).native() {
        Some(Type::Str) => return Err(ExcType::type_error_sum_start("strings", "''")),
        Some(Type::Bytes) => return Err(ExcType::type_error_sum_start("bytes", "b''")),
        _ => {}
    }
    // Take the start value out of its guard (dropping `None` is a no-op).
    let accumulator = mem::replace(start, Value::None);

    // DropGuard for accumulator: on success we extract it via into_inner(),
    // on any error path it's dropped automatically
    let mut acc_guard = DropGuard::new(accumulator, vm);
    let (accumulator, vm) = acc_guard.as_parts_mut();

    let mut phase = Phase::start(accumulator, vm);
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if phase.add(item, vm)? {
            continue;
        }

        phase.store(accumulator, vm);
        let new_value = accumulator.py_add(item, vm)?;
        let old = mem::replace(accumulator, new_value);
        old.drop_with(vm);
        phase = phase.next(item, accumulator, vm);
    }
    phase.store(accumulator, vm);

    Ok(acc_guard.into_inner())
}

/// CPython's fast paths through `sum()`, in the order it tries them.
///
/// The float and complex paths keep a compensated running total instead of
/// adding item by item, so `sum([0.1] * 10) == 1.0`. Once an item doesn't fit
/// a path, the total is stored and the sum moves on to a later path for good.
enum Phase {
    /// The total is an int and every item so far was an int or bool.
    Int,
    Float(CompensatedSum),
    Complex(CompensatedSum, CompensatedSum),
    /// Plain `+` from here on.
    Generic,
}

impl Phase {
    fn start(total: &Value, vm: &VM<'_>) -> Self {
        match total {
            Value::Int(_) => Self::Int,
            _ => Self::float_onwards(total, vm),
        }
    }

    fn float_onwards(total: &Value, vm: &VM<'_>) -> Self {
        match total {
            Value::Float(x) => Self::Float(CompensatedSum::new(*x)),
            _ => Self::complex_onwards(total, vm),
        }
    }

    fn complex_onwards(total: &Value, vm: &VM<'_>) -> Self {
        match total.py_value(vm) {
            PyValue::Complex(z) => {
                let z = z.get(vm.heap);
                Self::Complex(CompensatedSum::new(z.real), CompensatedSum::new(z.imag))
            }
            _ => Self::Generic,
        }
    }

    /// Adds `item` to a compensated total, or returns `false` if it doesn't fit this path.
    fn add(&mut self, item: &Value, vm: &VM<'_>) -> RunResult<bool> {
        if matches!(self, Self::Int | Self::Generic) {
            return Ok(false);
        }
        match (self, item.py_value(vm)) {
            (Self::Float(real) | Self::Complex(real, _), PyValue::Float(x)) => real.add(x),
            (Self::Float(real) | Self::Complex(real, _), PyValue::Int(i)) => real.add(i.to_f64_checked(vm)?),
            (Self::Float(real) | Self::Complex(real, _), PyValue::Bool(b)) => real.add(f64::from(b)),
            (Self::Complex(real, imag), PyValue::Complex(z)) => {
                let z = z.get(vm.heap);
                real.add(z.real);
                imag.add(z.imag);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Writes a compensated total back into `total`.
    fn store(&self, total: &mut Value, vm: &mut VM<'_>) {
        let value = match self {
            Self::Float(real) => Value::Float(real.value()),
            Self::Complex(real, imag) => Complex::new(real.value(), imag.value()).into_value(vm.heap),
            Self::Int | Self::Generic => return,
        };
        mem::replace(total, value).drop_with(vm);
    }

    /// The path to take after `item` was added to `total` with plain `+`.
    fn next(self, item: &Value, total: &Value, vm: &VM<'_>) -> Self {
        match self {
            Self::Int if matches!(item, Value::Int(_) | Value::Bool(_)) && matches!(total, Value::Int(_)) => Self::Int,
            Self::Int => Self::float_onwards(total, vm),
            Self::Float(_) => Self::complex_onwards(total, vm),
            Self::Complex(..) | Self::Generic => Self::Generic,
        }
    }
}

/// A float total with Neumaier compensation, CPython's `CompensatedSum`.
#[derive(Clone, Copy)]
struct CompensatedSum {
    hi: f64,
    lo: f64,
}

impl CompensatedSum {
    fn new(x: f64) -> Self {
        Self { hi: x, lo: 0.0 }
    }

    fn add(&mut self, x: f64) {
        let t = self.hi + x;
        self.lo += if self.hi.abs() >= x.abs() {
            (self.hi - t) + x
        } else {
            (x - t) + self.hi
        };
        self.hi = t;
    }

    /// Skips a zero or non-finite compensation, so `-0.0` keeps its sign and
    /// an overflowed total doesn't turn into `nan`.
    fn value(self) -> f64 {
        if self.lo != 0.0 && self.lo.is_finite() {
            self.hi + self.lo
        } else {
            self.hi
        }
    }
}
