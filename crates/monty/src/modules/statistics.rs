//! Implementation of the `statistics` module's core functions.
//!
//! CPython sums `int` and `float` data exactly (via `fractions.Fraction`) and rounds
//! once at the end, so results are correctly rounded rather than accumulating float
//! error. Monty has no `fractions` module, so the same exact arithmetic is done here
//! on `BigInt`s: every finite float is an integer over a power of two, which lets a
//! sum be kept as one `BigInt` numerator over `2**exp` and reduced only at the end.
//!
//! Only `int`, `bool` and `float` data are supported by the exact functions; other
//! numeric types (`Decimal`, `Fraction`) raise the same `TypeError` CPython raises for
//! a type with no numerator/denominator. `NormalDist`, `kde`, `kde_random`,
//! `geometric_mean`, `harmonic_mean`, `median_grouped`, `covariance`, `correlation`
//! and `linear_regression` are not provided.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, Zero};

use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::{ModuleFunctions, collections::counter_init, math},
    resource_checks::{check_div_size, check_mult_size, try_vec_with_capacity},
    sorting::sort_values,
    types::{
        List, Module, PyTrait,
        iter::collect_owned_iterable,
        long_int::{LongInt, bigint_true_divide},
    },
    value::{VALUE_SIZE, Value},
};

/// The functions of the `statistics` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum StatisticsFunctions {
    Mean,
    Fmean,
    Median,
    MedianLow,
    MedianHigh,
    Mode,
    Multimode,
    Pstdev,
    Pvariance,
    Stdev,
    Variance,
    Quantiles,
}

/// Creates the module and its `StatisticsError` exception type.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Statistics, vm.interns);
    for (name, function) in [
        (StaticStrings::Mean, StatisticsFunctions::Mean),
        (StaticStrings::Fmean, StatisticsFunctions::Fmean),
        (StaticStrings::Median, StatisticsFunctions::Median),
        (StaticStrings::MedianLow, StatisticsFunctions::MedianLow),
        (StaticStrings::MedianHigh, StatisticsFunctions::MedianHigh),
        (StaticStrings::Mode, StatisticsFunctions::Mode),
        (StaticStrings::Multimode, StatisticsFunctions::Multimode),
        (StaticStrings::Pstdev, StatisticsFunctions::Pstdev),
        (StaticStrings::Pvariance, StatisticsFunctions::Pvariance),
        (StaticStrings::Stdev, StatisticsFunctions::Stdev),
        (StaticStrings::Variance, StatisticsFunctions::Variance),
        (StaticStrings::Quantiles, StatisticsFunctions::Quantiles),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Statistics(function)), vm);
    }
    module.set_attr(
        StaticStrings::StatisticsError,
        Value::Builtin(Builtins::ExcType(ExcType::StatisticsError)),
        vm,
    );
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `statistics` function.
pub(super) fn call(vm: &mut VM<'_>, function: StatisticsFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        StatisticsFunctions::Mean => mean(vm, args),
        StatisticsFunctions::Fmean => fmean(vm, args),
        StatisticsFunctions::Median => median(vm, args, Median::Average),
        StatisticsFunctions::MedianLow => median(vm, args, Median::Low),
        StatisticsFunctions::MedianHigh => median(vm, args, Median::High),
        StatisticsFunctions::Mode => mode(vm, args, false),
        StatisticsFunctions::Multimode => mode(vm, args, true),
        StatisticsFunctions::Pstdev => spread(vm, args, Spread::Pstdev),
        StatisticsFunctions::Pvariance => spread(vm, args, Spread::Pvariance),
        StatisticsFunctions::Stdev => spread(vm, args, Spread::Stdev),
        StatisticsFunctions::Variance => spread(vm, args, Spread::Variance),
        StatisticsFunctions::Quantiles => quantiles(vm, args),
    }
}

fn statistics_error(msg: impl Into<String>) -> RunError {
    SimpleException::new_msg(ExcType::StatisticsError, msg.into()).into()
}

// ---------------------------------------------------------------------------
// Exact arithmetic
// ---------------------------------------------------------------------------

/// A fraction with a positive denominator, not necessarily in lowest terms.
struct Frac {
    num: BigInt,
    den: BigInt,
}

impl Frac {
    fn dyadic(num: BigInt, exp: u32) -> Self {
        Self {
            num,
            den: BigInt::one() << exp,
        }
    }

    fn mul(&self, other: &Self) -> Self {
        Self {
            num: &self.num * &other.num,
            den: &self.den * &other.den,
        }
    }

    fn sub(&self, other: &Self) -> Self {
        Self {
            num: &self.num * &other.den - &other.num * &self.den,
            den: &self.den * &other.den,
        }
    }

    /// Divides by a positive integer.
    fn div_int(self, k: usize) -> Self {
        Self {
            num: self.num,
            den: self.den * BigInt::from(k),
        }
    }

    /// Reduces to lowest terms, as `fractions.Fraction` always is.
    fn reduced(self) -> Self {
        let gcd = self.num.gcd(&self.den);
        if gcd.is_one() || gcd.is_zero() {
            return self;
        }
        Self {
            num: self.num / &gcd,
            den: self.den / gcd,
        }
    }
}

/// One datum: an exact `num / 2**exp`, or an infinity/NaN that has no exact ratio.
enum Elem {
    Finite(BigInt, u32),
    Special(f64),
}

/// Mirrors `statistics._exact_ratio`; the bool says whether the datum was a `float`.
fn element(value: &Value, vm: &VM<'_>) -> RunResult<(Elem, bool)> {
    match value {
        Value::Int(n) => Ok((Elem::Finite(BigInt::from(*n), 0), false)),
        Value::Bool(b) => Ok((Elem::Finite(BigInt::from(i32::from(*b)), 0), false)),
        Value::Float(f) => Ok((float_element(*f), true)),
        _ => match value.as_long_int(vm) {
            Some(n) => {
                // The datum is copied out of the heap, so preflight that copy like any other big-int result.
                check_div_size(n.bits(), &vm.heap.tracker)?;
                Ok((Elem::Finite(n.clone(), 0), false))
            }
            None => Err(ExcType::type_error(format!(
                "can't convert type '{}' to numerator/denominator",
                value.py_type_name(vm)
            ))),
        },
    }
}

fn float_element(f: f64) -> Elem {
    if !f.is_finite() {
        return Elem::Special(f);
    }
    let bits = f.to_bits();
    let biased = i64::try_from((bits >> 52) & 0x7ff).expect("11 bits");
    let fraction = bits & ((1_u64 << 52) - 1);
    let (mut mantissa, mut exponent) = if biased == 0 {
        (fraction, -1074_i64)
    } else {
        (fraction | (1_u64 << 52), biased - 1075)
    };
    if mantissa == 0 {
        return Elem::Finite(BigInt::zero(), 0);
    }
    let trailing = mantissa.trailing_zeros();
    mantissa >>= trailing;
    exponent += i64::from(trailing);
    let mut num = BigInt::from(mantissa);
    if bits >> 63 == 1 {
        num = -num;
    }
    if exponent >= 0 {
        Elem::Finite(num << usize::try_from(exponent).expect("non-negative"), 0)
    } else {
        Elem::Finite(num, u32::try_from(-exponent).expect("at most 1074"))
    }
}

/// An exact running sum `num / 2**exp`.
struct Dyadic {
    num: BigInt,
    exp: u32,
}

impl Dyadic {
    fn new() -> Self {
        Self {
            num: BigInt::zero(),
            exp: 0,
        }
    }

    fn add(&mut self, num: BigInt, exp: u32) {
        if exp > self.exp {
            self.num <<= (exp - self.exp) as usize;
            self.exp = exp;
            self.num += num;
        } else {
            self.num += num << (self.exp - exp) as usize;
        }
    }

    fn into_frac(self) -> Frac {
        Frac::dyadic(self.num, self.exp)
    }
}

/// The result of `_sum`: an exact total, or the float sum of the non-finite data.
enum Total {
    Exact(Frac),
    Special(f64),
}

/// Mirrors `statistics._sum`; returns `(total, has_float, count)`.
fn exact_sum<'a>(values: impl Iterator<Item = &'a Value>, vm: &mut VM<'_>) -> RunResult<(Total, bool, usize)> {
    let mut acc = Dyadic::new();
    let mut special: Option<f64> = None;
    let mut has_float = false;
    let mut count = 0;
    for (i, value) in values.enumerate() {
        vm.heap.tracker.check_memory_time_every(i)?;
        let (elem, is_float) = element(value, vm)?;
        has_float |= is_float;
        count += 1;
        match elem {
            Elem::Finite(num, exp) => acc.add(num, exp),
            Elem::Special(x) => special = Some(special.unwrap_or(0.0) + x),
        }
    }
    let total = special.map_or_else(|| Total::Exact(acc.into_frac()), Total::Special);
    Ok((total, has_float, count))
}

/// Mirrors `statistics._convert`: an `int` when exactly integral and every datum was an int,
/// otherwise the correctly rounded float.
fn convert(frac: Frac, has_float: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let frac = frac.reduced();
    if !has_float && frac.den.is_one() {
        return Ok(LongInt::new(frac.num).into_value(vm.heap));
    }
    Ok(Value::Float(bigint_true_divide(
        &frac.num,
        &frac.den,
        &vm.heap.tracker,
    )?))
}

// ---------------------------------------------------------------------------
// Argument shapes
// ---------------------------------------------------------------------------

/// Declares the argument shape of a function that takes only `data`, named so that
/// argument errors identify the function the caller used.
macro_rules! data_args {
    ($name:ident, $func:literal) => {
        #[derive(FromArgs)]
        #[from_args(name = $func, style = def)]
        struct $name {
            data: Value,
        }
    };
}

data_args!(MeanArgs, "mean");
data_args!(MedianArgs, "median");
data_args!(MedianLowArgs, "median_low");
data_args!(MedianHighArgs, "median_high");
data_args!(ModeArgs, "mode");
data_args!(MultimodeArgs, "multimode");

#[derive(FromArgs)]
#[from_args(name = "variance", style = def)]
struct VarianceArgs {
    data: Value,
    #[from_args(default = Value::None)]
    xbar: Value,
}

#[derive(FromArgs)]
#[from_args(name = "stdev", style = def)]
struct StdevArgs {
    data: Value,
    #[from_args(default = Value::None)]
    xbar: Value,
}

#[derive(FromArgs)]
#[from_args(name = "pvariance", style = def)]
struct PvarianceArgs {
    data: Value,
    #[from_args(default = Value::None)]
    mu: Value,
}

#[derive(FromArgs)]
#[from_args(name = "pstdev", style = def)]
struct PstdevArgs {
    data: Value,
    #[from_args(default = Value::None)]
    mu: Value,
}

#[derive(FromArgs)]
#[from_args(name = "fmean", style = def)]
struct FmeanArgs {
    data: Value,
    #[from_args(default = Value::None)]
    weights: Value,
}

#[derive(FromArgs)]
#[from_args(name = "quantiles", style = def)]
struct QuantilesArgs {
    data: Value,
    #[from_args(kw_only, default = Value::Int(4))]
    n: Value,
    #[from_args(kw_only, default = Value::Undefined)]
    method: Value,
}

/// An empty buffer with room for exactly `count` values, after checking that it fits the memory budget.
fn reserve_values(count: usize, vm: &VM<'_>) -> RunResult<Vec<Value>> {
    vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE))?;
    try_vec_with_capacity(count)
}

fn collect(data: Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    collect_owned_iterable(data, vm)
}

// ---------------------------------------------------------------------------
// mean / fmean
// ---------------------------------------------------------------------------

fn mean(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let MeanArgs { data } = MeanArgs::from_args(args, vm)?;
    let items = collect(data, vm)?;
    let result = mean_of(&items, vm);
    items.drop_with(vm);
    result
}

fn mean_of(items: &[Value], vm: &mut VM<'_>) -> RunResult<Value> {
    let (total, has_float, n) = exact_sum(items.iter(), vm)?;
    if n < 1 {
        return Err(statistics_error("mean requires at least one data point"));
    }
    match total {
        Total::Special(x) => Ok(Value::Float(x / n as f64)),
        Total::Exact(frac) => convert(frac.div_int(n), has_float, vm),
    }
}

fn fmean(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FmeanArgs { data, weights } = FmeanArgs::from_args(args, vm)?;
    let items = match collect(data, vm) {
        Ok(items) => items,
        Err(err) => {
            weights.drop_with(vm);
            return Err(err);
        }
    };
    let n = items.len();
    let data = Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))));
    if matches!(weights, Value::None) {
        let total = math::fsum(vm, ArgValues::One(data))?;
        if n == 0 {
            return Err(statistics_error("fmean requires at least one data point"));
        }
        let Value::Float(total) = total else {
            unreachable!("math.fsum returns a float");
        };
        return Ok(Value::Float(total / n as f64));
    }
    let weights = match collect(weights, vm) {
        Ok(weights) => weights,
        Err(err) => {
            data.drop_with(vm);
            return Err(err);
        }
    };
    let weights = Value::Ref(vm.heap.allocate(HeapData::List(List::new(weights))));
    let weights_for_sum = weights.clone_with_heap(vm.heap);
    let num = match math::sumprod(vm, ArgValues::Two(data, weights_for_sum)) {
        Ok(num) => num,
        Err(RunError::Exc(raise)) if raise.exc.exc_type().is_subclass_of(ExcType::ValueError) => {
            weights.drop_with(vm);
            return Err(statistics_error("data and weights must be the same length"));
        }
        Err(err) => {
            weights.drop_with(vm);
            return Err(err);
        }
    };
    let den = math::fsum(vm, ArgValues::One(weights));
    let den = match den {
        Ok(den) => den,
        Err(err) => {
            num.drop_with(vm);
            return Err(err);
        }
    };
    if matches!(den, Value::Float(d) if d == 0.0) {
        num.drop_with(vm);
        return Err(statistics_error("sum of weights must be non-zero"));
    }
    let result = num.py_truediv(&den, vm);
    num.drop_with(vm);
    den.drop_with(vm);
    result
}

// ---------------------------------------------------------------------------
// variance / stdev
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Spread {
    Variance,
    Pvariance,
    Stdev,
    Pstdev,
}

impl Spread {
    fn name(self) -> &'static str {
        match self {
            Self::Variance => "variance",
            Self::Pvariance => "pvariance",
            Self::Stdev => "stdev",
            Self::Pstdev => "pstdev",
        }
    }

    fn sample(self) -> bool {
        matches!(self, Self::Variance | Self::Stdev)
    }

    fn is_stdev(self) -> bool {
        matches!(self, Self::Stdev | Self::Pstdev)
    }
}

fn spread(vm: &mut VM<'_>, args: ArgValues, kind: Spread) -> RunResult<Value> {
    let (data, center) = match kind {
        Spread::Variance => {
            let VarianceArgs { data, xbar } = VarianceArgs::from_args(args, vm)?;
            (data, xbar)
        }
        Spread::Stdev => {
            let StdevArgs { data, xbar } = StdevArgs::from_args(args, vm)?;
            (data, xbar)
        }
        Spread::Pvariance => {
            let PvarianceArgs { data, mu } = PvarianceArgs::from_args(args, vm)?;
            (data, mu)
        }
        Spread::Pstdev => {
            let PstdevArgs { data, mu } = PstdevArgs::from_args(args, vm)?;
            (data, mu)
        }
    };
    let items = match collect(data, vm) {
        Ok(items) => items,
        Err(err) => {
            center.drop_with(vm);
            return Err(err);
        }
    };
    let result = spread_of(&items, &center, kind, vm);
    items.drop_with(vm);
    center.drop_with(vm);
    result
}

/// Mirrors `statistics._ss`: the sum of squared deviations (exact, or a float for
/// non-finite data), whether any datum was a float, and the count.
fn sum_squares(items: &[Value], center: &Value, vm: &mut VM<'_>) -> RunResult<(Total, bool, usize)> {
    if !matches!(center, Value::None) {
        let mut squares = reserve_values(items.len(), vm)?;
        let mut failure = None;
        for (i, item) in items.iter().enumerate() {
            let step = vm
                .heap
                .tracker
                .check_memory_time_every(i)
                .map_err(RunError::from)
                .and_then(|()| item.py_sub(center, vm))
                .and_then(|d| {
                    let square = d.py_mul(&d, vm);
                    d.drop_with(vm);
                    square
                });
            match step {
                Ok(square) => squares.push(square),
                Err(err) => {
                    failure = Some(err);
                    break;
                }
            }
        }
        let result = match failure {
            Some(err) => Err(err),
            None => exact_sum(squares.iter(), vm),
        };
        squares.drop_with(vm);
        return result;
    }

    let mut sx = Dyadic::new();
    let mut sxx = Dyadic::new();
    let mut special: Option<f64> = None;
    let mut has_float = false;
    for (i, item) in items.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(i)?;
        let (elem, is_float) = element(item, vm)?;
        has_float |= is_float;
        match elem {
            Elem::Finite(num, exp) => {
                check_mult_size(num.bits(), num.bits(), &vm.heap.tracker)?;
                sxx.add(&num * &num, exp * 2);
                sx.add(num, exp);
            }
            Elem::Special(x) => special = Some(special.unwrap_or(0.0) + x),
        }
    }
    let count = items.len();
    if let Some(x) = special {
        return Ok((Total::Special(x), has_float, count));
    }
    if count == 0 {
        return Ok((Total::Exact(Frac::dyadic(BigInt::zero(), 0)), has_float, 0));
    }
    let sx = sx.into_frac();
    let sxx = sxx.into_frac();
    // The final combination multiplies the running sums, so bound its working set before doing it.
    check_mult_size(sx.num.bits(), sx.num.bits(), &vm.heap.tracker)?;
    check_mult_size(sxx.num.bits(), BigInt::from(count).bits(), &vm.heap.tracker)?;
    let count_frac = Frac::dyadic(BigInt::from(count), 0);
    let ssd = count_frac.mul(&sxx).sub(&sx.mul(&sx)).div_int(count);
    Ok((Total::Exact(ssd), has_float, count))
}

fn spread_of(items: &[Value], center: &Value, kind: Spread, vm: &mut VM<'_>) -> RunResult<Value> {
    let (total, has_float, n) = sum_squares(items, center, vm)?;
    let min = if kind.sample() { 2 } else { 1 };
    if n < min {
        let at_least = if min == 2 { "two data points" } else { "one data point" };
        return Err(statistics_error(format!(
            "{} requires at least {at_least}",
            kind.name()
        )));
    }
    let divisor = if kind.sample() { n - 1 } else { n };
    match total {
        Total::Special(x) => {
            if kind.is_stdev() {
                Err(ExcType::value_error("inf or nan encountered in data"))
            } else {
                Ok(Value::Float(x / divisor as f64))
            }
        }
        Total::Exact(ss) => {
            let mss = ss.div_int(divisor);
            if kind.is_stdev() {
                let mss = mss.reduced();
                // A user-defined `__sub__`/`__mul__` can make the total negative; CPython's `isqrt` rejects it.
                if mss.num.is_negative() {
                    return Err(ExcType::value_error("isqrt() argument must be nonnegative"));
                }
                float_sqrt_of_frac(&mss.num, &mss.den, vm)
            } else {
                convert(mss, has_float, vm)
            }
        }
    }
}

/// Mirrors `statistics._float_sqrt_of_frac`: the correctly rounded square root of `n / m`.
fn float_sqrt_of_frac(n: &BigInt, m: &BigInt, vm: &mut VM<'_>) -> RunResult<Value> {
    // 2 * float_info.mant_dig + 3
    const SQRT_BIT_WIDTH: i64 = 109;
    let bits = |x: &BigInt| i64::try_from(x.bits()).unwrap_or(i64::MAX);
    let q = (bits(n) - bits(m) - SQRT_BIT_WIDTH).div_euclid(2);
    let (numerator, denominator) = if q >= 0 {
        let shift = usize::try_from(q).expect("non-negative");
        (integer_sqrt_of_frac_rto(n, &(m << (2 * shift))) << shift, BigInt::one())
    } else {
        let shift = usize::try_from(-q).expect("positive");
        (integer_sqrt_of_frac_rto(&(n << (2 * shift)), m), BigInt::one() << shift)
    };
    Ok(Value::Float(bigint_true_divide(
        &numerator,
        &denominator,
        &vm.heap.tracker,
    )?))
}

/// The integer square root of `n / m`, rounded to odd so a later rounding cannot double-round.
fn integer_sqrt_of_frac_rto(n: &BigInt, m: &BigInt) -> BigInt {
    let a = (n / m).sqrt();
    if &(&a * &a * m) == n { a } else { a | BigInt::one() }
}

// ---------------------------------------------------------------------------
// median family / quantiles
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Median {
    Average,
    Low,
    High,
}

fn sorted_items(data: Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let mut items = collect(data, vm)?;
    if let Err(err) = sort_values(&mut items, None, false, vm) {
        items.drop_with(vm);
        return Err(err);
    }
    Ok(items)
}

fn median(vm: &mut VM<'_>, args: ArgValues, kind: Median) -> RunResult<Value> {
    let data = match kind {
        Median::Average => MedianArgs::from_args(args, vm)?.data,
        Median::Low => MedianLowArgs::from_args(args, vm)?.data,
        Median::High => MedianHighArgs::from_args(args, vm)?.data,
    };
    let items = sorted_items(data, vm)?;
    let n = items.len();
    let result = if n == 0 {
        Err(statistics_error("no median for empty data"))
    } else {
        let i = n / 2;
        match kind {
            Median::High => Ok(items[i].clone_with_heap(vm.heap)),
            Median::Low if n % 2 == 0 => Ok(items[i - 1].clone_with_heap(vm.heap)),
            Median::Low => Ok(items[i].clone_with_heap(vm.heap)),
            Median::Average if n % 2 == 1 => Ok(items[i].clone_with_heap(vm.heap)),
            Median::Average => items[i - 1].py_add(&items[i], vm).and_then(|sum| {
                let half = sum.py_truediv(&Value::Int(2), vm);
                sum.drop_with(vm);
                half
            }),
        }
    };
    items.drop_with(vm);
    result
}

fn quantiles(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let QuantilesArgs { data, n, method } = QuantilesArgs::from_args(args, vm)?;
    let result = quantiles_of(data, &n, &method, vm);
    n.drop_with(vm);
    method.drop_with(vm);
    result
}

fn quantiles_of(data: Value, n: &Value, method: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let n = match n {
        Value::Int(n) => *n,
        Value::Bool(b) => i64::from(*b),
        other => {
            data.drop_with(vm);
            return Err(ExcType::type_error(format!(
                "'{}' object cannot be interpreted as an integer",
                other.py_type_name(vm)
            )));
        }
    };
    if n < 1 {
        data.drop_with(vm);
        return Err(statistics_error("n must be at least 1"));
    }
    let items = sorted_items(data, vm)?;
    let result = quantile_values(&items, n, method, vm);
    items.drop_with(vm);
    result
}

fn quantile_values(items: &[Value], n: i64, method: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let ld = i64::try_from(items.len()).expect("length fits in i64");
    if ld < 2 {
        if ld == 1 {
            let mut copies = reserve_values(usize::try_from(n - 1).unwrap_or(usize::MAX), vm)?;
            for i in 1..n {
                if let Err(err) = vm.heap.tracker.check_memory_time_every(usize::try_from(i).unwrap_or(0)) {
                    copies.drop_with(vm);
                    return Err(err.into());
                }
                copies.push(items[0].clone_with_heap(vm.heap));
            }
            return Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(copies)))));
        }
        return Err(statistics_error("must have at least one data point"));
    }
    let inclusive = match method {
        Value::Undefined => false,
        other => match other.as_str(vm) {
            Some("exclusive") => false,
            Some("inclusive") => true,
            _ => {
                let repr = other.py_repr(vm)?;
                let text = repr.as_str(vm).expect("py_repr returns str").to_owned();
                repr.drop_with(vm);
                return Err(ExcType::value_error(format!("Unknown method: {text}")));
            }
        },
    };
    let mut out = reserve_values(usize::try_from(n - 1).unwrap_or(usize::MAX), vm)?;
    let mut failure = None;
    for i in 1..n {
        let (delta, lo_hi) = if inclusive {
            let m = ld - 1;
            let (j, delta) = (i * m).div_mod_floor(&n);
            (delta, (j, j + 1))
        } else {
            let m = ld + 1;
            let j = ((i * m).div_euclid(n)).clamp(1, ld - 1);
            (i * m - j * n, (j - 1, j))
        };
        let step = vm
            .heap
            .tracker
            .check_memory_time_every(usize::try_from(i).unwrap_or(0))
            .map_err(RunError::from)
            .and_then(|()| {
                let (lo, hi) = lo_hi;
                let left = items[usize::try_from(lo).expect("in range")].py_mul(&Value::Int(n - delta), vm)?;
                let right = match items[usize::try_from(hi).expect("in range")].py_mul(&Value::Int(delta), vm) {
                    Ok(right) => right,
                    Err(err) => {
                        left.drop_with(vm);
                        return Err(err);
                    }
                };
                let sum = left.py_add(&right, vm);
                left.drop_with(vm);
                right.drop_with(vm);
                let sum = sum?;
                let quotient = sum.py_truediv(&Value::Int(n), vm);
                sum.drop_with(vm);
                quotient
            });
        match step {
            Ok(value) => out.push(value),
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }
    if let Some(err) = failure {
        out.drop_with(vm);
        return Err(err);
    }
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(out)))))
}

// ---------------------------------------------------------------------------
// mode / multimode
// ---------------------------------------------------------------------------

fn mode(vm: &mut VM<'_>, args: ArgValues, all: bool) -> RunResult<Value> {
    let data = if all {
        MultimodeArgs::from_args(args, vm)?.data
    } else {
        ModeArgs::from_args(args, vm)?.data
    };
    let counter = counter_init(vm, ArgValues::One(data))?;
    let Value::Ref(id) = &counter else {
        unreachable!("Counter is a heap value");
    };
    let pairs = {
        let HeapReadOutput::Dict(dict) = vm.heap.read(*id) else {
            unreachable!("Counter is a dict");
        };
        dict.clone_all_pairs(vm)
    };
    counter.drop_with(vm);
    let mut pairs = pairs?;
    let count_of = |value: &Value| if let Value::Int(c) = value { *c } else { 0 };
    let max = pairs.iter().map(|(_, c)| count_of(c)).max();
    let result = match (max, all) {
        (None, true) => Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(Vec::new()))))),
        (None, false) => Err(statistics_error("no mode for empty data")),
        (Some(max), true) => {
            let mut winners = Vec::new();
            for (key, count) in &pairs {
                if count_of(count) == max {
                    winners.push(key.clone_with_heap(vm.heap));
                }
            }
            Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(winners)))))
        }
        (Some(max), false) => {
            let first = pairs
                .iter()
                .find(|(_, c)| count_of(c) == max)
                .expect("max came from pairs");
            Ok(first.0.clone_with_heap(vm.heap))
        }
    };
    for (key, count) in pairs.drain(..) {
        key.drop_with(vm);
        count.drop_with(vm);
    }
    result
}
