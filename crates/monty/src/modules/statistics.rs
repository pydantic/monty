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
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, HeapData, HeapReadOutput},
    intern::StaticStrings,
    modules::{ModuleFunctions, collections::counter_init, math},
    resource_checks::try_vec_with_capacity,
    sorting::{py_lt, sort_values},
    types::{
        List, Module, PyTrait, Type,
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
pub fn create_module(vm: &mut VM<'_>) -> Module {
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
    module
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

/// `mean(data)`: the exact arithmetic mean, rounded once at the end as CPython rounds it.
fn mean(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let MeanArgs { data } = MeanArgs::from_args(args, vm)?;
    let items: Vec<Value> = collect_owned_iterable(data, vm)?;
    defer_drop!(items, vm);
    let (total, has_float, n) = exact_sum(items, vm)?;
    match total {
        _ if n == 0 => Err(ExcType::statistics_error("mean requires at least one data point")),
        Total::Special(x) => Ok(Value::Float(x / n as f64)),
        Total::Exact(frac) => convert(frac.div_int(n), has_float, vm),
    }
}

/// Declares the `FromArgs` struct of a function that takes only `data`.
///
/// Each function gets its own struct so that an argument error names the function that was called.
macro_rules! data_args {
    ($name:ident, $func:literal) => {
        #[doc = concat!("The arguments of `statistics.", $func, "(data)`.")]
        #[derive(FromArgs)]
        #[from_args(name = $func, style = def)]
        struct $name {
            data: Value,
        }
    };
}

data_args!(MeanArgs, "mean");

/// `fmean(data, weights=None)`: the mean as a float, through `math.fsum` / `math.sumprod` as CPython computes it.
fn fmean(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FmeanArgs { data, weights } = FmeanArgs::from_args(args, vm)?;
    if matches!(weights, Value::None) {
        // CPython divides `fsum(data)` by `len(data)` when there is one, and otherwise counts the items.
        let (data, n) = if let Some(n) = data.py_len(vm) {
            (data, n)
        } else {
            let items: Vec<Value> = collect_owned_iterable(data, vm)?;
            let n = items.len();
            (list_value(items, vm), n)
        };
        match math::fsum(vm, ArgValues::One(data))? {
            _ if n == 0 => Err(ExcType::statistics_error("fmean requires at least one data point")),
            Value::Float(total) => Ok(Value::Float(total / n as f64)),
            _ => unreachable!("math.fsum returns a float"),
        }
    } else {
        weighted_fmean(data, weights, vm)
    }
}

/// The arguments of `statistics.fmean(data, weights=None)`.
#[derive(FromArgs)]
#[from_args(name = "fmean", style = def)]
struct FmeanArgs {
    data: Value,
    #[from_args(default = Value::None)]
    weights: Value,
}

/// `fmean(data, weights)`: `sum(x * w) / sum(w)`.
///
/// CPython runs `list(weights)` before `sumprod` consumes `data`, so a one-shot iterator shared by
/// both is read in that order. A list or tuple of weights, and `data`, are handed on as they are,
/// never copied.
fn weighted_fmean(data: Value, weights: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut data_guard = DropGuard::new(data, vm);
    let (_, vm) = data_guard.as_parts_mut();
    let weights = if matches!(weights.py_type(vm), Type::List | Type::Tuple) {
        weights
    } else {
        let weights: Vec<Value> = collect_owned_iterable(weights, vm)?;
        list_value(weights, vm)
    };
    let (data, vm) = data_guard.into_parts();
    defer_drop!(weights, vm);
    let weights_for_sum = weights.clone_with_heap(vm.heap);
    let num = math::sumprod(vm, ArgValues::Two(data, weights_for_sum)).map_err(|err| match err {
        // CPython turns any `ValueError` from `sumprod` into this message, not only the length check.
        RunError::Exc(raise) if raise.exc.exc_type().is_subclass_of(ExcType::ValueError) => {
            ExcType::statistics_error("data and weights must be the same length")
        }
        err => err,
    })?;
    defer_drop!(num, vm);
    let weights_for_den = weights.clone_with_heap(vm.heap);
    match math::fsum(vm, ArgValues::One(weights_for_den))? {
        // A float pattern compares with `==`, so this also catches `-0.0`, as CPython's `not den` does.
        Value::Float(0.0) => Err(ExcType::statistics_error("sum of weights must be non-zero")),
        den => num.py_truediv(&den, vm),
    }
}

/// The variance family, which differ only in their divisor and whether they take the square root.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spread {
    Variance,
    Pvariance,
    Stdev,
    Pstdev,
}

impl Spread {
    /// The function's name, for its "requires at least" error.
    fn name(self) -> &'static str {
        match self {
            Self::Variance => "variance",
            Self::Pvariance => "pvariance",
            Self::Stdev => "stdev",
            Self::Pstdev => "pstdev",
        }
    }

    /// Whether this is a sample statistic, which divides by `n - 1` and needs two data points.
    fn sample(self) -> bool {
        matches!(self, Self::Variance | Self::Stdev)
    }

    /// Whether the result is the square root of the variance.
    fn is_stdev(self) -> bool {
        matches!(self, Self::Stdev | Self::Pstdev)
    }
}

/// `variance`, `pvariance`, `stdev` and `pstdev`: parses the function's own arguments, then computes [`spread_of`].
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
    defer_drop!(center, vm);
    let items: Vec<Value> = collect_owned_iterable(data, vm)?;
    defer_drop!(items, vm);
    spread_of(items, center, kind, vm)
}

/// The arguments of `statistics.variance(data, xbar=None)`.
#[derive(FromArgs)]
#[from_args(name = "variance", style = def)]
struct VarianceArgs {
    data: Value,
    #[from_args(default = Value::None)]
    xbar: Value,
}

/// The arguments of `statistics.stdev(data, xbar=None)`.
#[derive(FromArgs)]
#[from_args(name = "stdev", style = def)]
struct StdevArgs {
    data: Value,
    #[from_args(default = Value::None)]
    xbar: Value,
}

/// The arguments of `statistics.pvariance(data, mu=None)`.
#[derive(FromArgs)]
#[from_args(name = "pvariance", style = def)]
struct PvarianceArgs {
    data: Value,
    #[from_args(default = Value::None)]
    mu: Value,
}

/// The arguments of `statistics.pstdev(data, mu=None)`.
#[derive(FromArgs)]
#[from_args(name = "pstdev", style = def)]
struct PstdevArgs {
    data: Value,
    #[from_args(default = Value::None)]
    mu: Value,
}

/// The body CPython's four variance functions share: the sum of squares, the count check, then the mean
/// square (`variance`) or its correctly rounded root (`stdev`).
fn spread_of(items: &[Value], center: &Value, kind: Spread, vm: &mut VM<'_>) -> RunResult<Value> {
    let (total, has_float, n) = if matches!(center, Value::None) {
        sum_squares(items, vm)?
    } else {
        sum_squares_about(items, center, vm)?
    };
    let divisor = if kind.sample() { n.checked_sub(1) } else { Some(n) };
    match (divisor, total) {
        (None | Some(0), _) => {
            let at_least = if kind.sample() {
                "two data points"
            } else {
                "one data point"
            };
            Err(ExcType::statistics_error(format!(
                "{} requires at least {at_least}",
                kind.name()
            )))
        }
        (Some(_), Total::Special(_)) if kind.is_stdev() => Err(ExcType::value_error("inf or nan encountered in data")),
        (Some(divisor), Total::Special(x)) => Ok(Value::Float(x / divisor as f64)),
        (Some(divisor), Total::Exact(ss)) if kind.is_stdev() => {
            let mss = ss.div_int(divisor).reduced();
            // A user-defined `__sub__`/`__mul__` can make the total negative; CPython's `isqrt` rejects it.
            if mss.num.is_negative() {
                Err(ExcType::isqrt_negative_error())
            } else {
                float_sqrt_of_frac(&mss.num, &mss.den, vm)
            }
        }
        (Some(divisor), Total::Exact(ss)) => convert(ss.div_int(divisor), has_float, vm),
    }
}

/// Mirrors `statistics._ss(data)`: the exact sum of squared deviations from the mean, from one pass
/// of running `sum(x)` and `sum(x * x)`. Returns `(total, has_float, count)`.
fn sum_squares(items: &[Value], vm: &mut VM<'_>) -> RunResult<(Total, bool, usize)> {
    let mut sx = ExactSum::new();
    let mut sxx = Dyadic::new();
    for (i, item) in items.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(i)?;
        let (elem, is_float) = element(item, vm)?;
        if let Elem::Finite(num, exp) = &elem {
            sxx.add(num * num, exp * 2);
        }
        sx.add_elem(elem, is_float);
    }
    let (total, has_float, count) = sx.finish();
    let total = match total {
        // `(n * sum(x*x) - sum(x)**2) / n`; an empty sum stays an exact zero.
        Total::Exact(sx) if count > 0 => {
            let count_frac = Frac::dyadic(BigInt::from(count), 0);
            Total::Exact(count_frac.mul(&sxx.into_frac()).sub(&sx.mul(&sx)).div_int(count))
        }
        total => total,
    };
    Ok((total, has_float, count))
}

/// Mirrors `statistics._ss(data, c)`: the exact sum of `(x - c) * (x - c)`, using the data's own operators.
fn sum_squares_about(items: &[Value], center: &Value, vm: &mut VM<'_>) -> RunResult<(Total, bool, usize)> {
    let mut sum = ExactSum::new();
    for (i, item) in items.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(i)?;
        let deviation = item.py_sub(center, vm)?;
        defer_drop!(deviation, vm);
        let square = deviation.py_mul(deviation, vm)?;
        defer_drop!(square, vm);
        sum.add(square, vm)?;
    }
    Ok(sum.finish())
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

/// Which middle value the median family returns when there are two.
#[derive(Clone, Copy)]
enum Median {
    /// `median`: the mean of the two middle values.
    Average,
    /// `median_low`: the smaller middle value.
    Low,
    /// `median_high`: the larger middle value.
    High,
}

/// `median`, `median_low` and `median_high`: the middle of the sorted data.
fn median(vm: &mut VM<'_>, args: ArgValues, kind: Median) -> RunResult<Value> {
    let data = match kind {
        Median::Average => MedianArgs::from_args(args, vm)?.data,
        Median::Low => MedianLowArgs::from_args(args, vm)?.data,
        Median::High => MedianHighArgs::from_args(args, vm)?.data,
    };
    let items = sorted_items(data, vm)?;
    defer_drop!(items, vm);
    let n = items.len();
    let i = n / 2;
    match kind {
        _ if n == 0 => Err(ExcType::statistics_error("no median for empty data")),
        Median::Low | Median::Average if n % 2 == 1 => Ok(items[i].clone_with_heap(vm.heap)),
        Median::High => Ok(items[i].clone_with_heap(vm.heap)),
        Median::Low => Ok(items[i - 1].clone_with_heap(vm.heap)),
        Median::Average => {
            let sum = items[i - 1].py_add(&items[i], vm)?;
            defer_drop!(sum, vm);
            sum.py_truediv(&Value::Int(2), vm)
        }
    }
}

data_args!(MedianArgs, "median");
data_args!(MedianLowArgs, "median_low");
data_args!(MedianHighArgs, "median_high");

/// `sorted(data)`: the data collected and sorted with Python's ordering, as the median and quantile functions
/// start by doing.
fn sorted_items(data: Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let items: Vec<Value> = collect_owned_iterable(data, vm)?;
    let mut items = DropGuard::new(items, vm);
    let (values, vm) = items.as_parts_mut();
    sort_values(values, None, false, vm)?;
    Ok(items.into_inner())
}

/// `quantiles(data, *, n=4, method='exclusive')`: the `n - 1` cut points that divide the data into `n` intervals.
///
/// Follows CPython's order: `n < 1` is an ordinary comparison, a single datum is `data * (n - 1)`, and `method`
/// is checked before `n` bounds the loop, so each bad argument raises what CPython raises.
fn quantiles(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let QuantilesArgs { data, n, method } = QuantilesArgs::from_args(args, vm)?;
    defer_drop!(n, vm);
    defer_drop!(method, vm);
    let mut data_guard = DropGuard::new(data, vm);
    let (_, vm) = data_guard.as_parts_mut();
    if py_lt(n, &Value::Int(1), vm)? {
        Err(ExcType::statistics_error("n must be at least 1"))
    } else {
        let (data, vm) = data_guard.into_parts();
        let items = sorted_items(data, vm)?;
        defer_drop!(items, vm);
        quantile_values(items, n, method, vm)
    }
}

/// The arguments of `statistics.quantiles(data, *, n=4, method='exclusive')`.
#[derive(FromArgs)]
#[from_args(name = "quantiles", style = def)]
struct QuantilesArgs {
    data: Value,
    #[from_args(kw_only, default = Value::Int(4))]
    n: Value,
    #[from_args(kw_only, default = Value::Undefined)]
    method: Value,
}

/// The cut points of sorted `items`, once `n` has passed the `n < 1` check.
fn quantile_values(items: &[Value], n: &Value, method: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    match items {
        [] => Err(ExcType::statistics_error("must have at least one data point")),
        // CPython returns `data * (n - 1)`, so `n` is used exactly as a sequence repeat count.
        [only] => {
            let repeat = n.py_sub(&Value::Int(1), vm)?;
            defer_drop!(repeat, vm);
            let single = list_value(vec![only.clone_with_heap(vm.heap)], vm);
            defer_drop!(single, vm);
            single.py_mul(repeat, vm)
        }
        _ => {
            let inclusive = inclusive_method(method, vm)?;
            // `range(1, n)` takes `n` through `__index__`; a count past `i64` could never be allocated.
            let n = n.saturating_index(vm)?;
            cut_points(items, n, inclusive, vm)
        }
    }
}

/// Whether `method` names the inclusive method; anything but the two method names is CPython's `ValueError`.
fn inclusive_method(method: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    match method {
        Value::Undefined => Ok(false),
        other => match other.as_str(vm) {
            Some("exclusive") => Ok(false),
            Some("inclusive") => Ok(true),
            _ => {
                let repr = other.py_repr(vm)?;
                defer_drop!(repr, vm);
                let text = repr.as_str(vm).expect("py_repr returns a str");
                Err(ExcType::value_error(format!("Unknown method: {text}")))
            }
        },
    }
}

/// CPython's two `quantiles` loops over at least two sorted values: each cut point interpolates between the
/// pair of data points `i * m / n` falls between, with exact integer weights.
fn cut_points(items: &[Value], n: i64, inclusive: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    // `n >= 1` here; a count past `usize` saturates so the reservation refuses it on 32-bit targets too.
    let count = usize::try_from(n - 1).unwrap_or(usize::MAX);
    let mut points = DropGuard::new(reserve_values(count, vm)?, vm);
    let (values, vm) = points.as_parts_mut();
    let ld = i128::try_from(items.len()).expect("a length fits in i128");
    let m = if inclusive { ld - 1 } else { ld + 1 };
    let n_wide = i128::from(n);
    for i in 1..n {
        vm.heap
            .tracker
            .check_memory_time_every(usize::try_from(i).unwrap_or(usize::MAX))?;
        let scaled = i128::from(i) * m;
        // Exclusive cut points are clamped to the interior, so their `delta` can fall outside `0..n`.
        let j = if inclusive {
            scaled / n_wide
        } else {
            (scaled / n_wide).clamp(1, ld - 1)
        };
        let delta = scaled - j * n_wide;
        let lo = usize::try_from(if inclusive { j } else { j - 1 }).expect("cut point index is in range");
        values.push(interpolate(&items[lo], &items[lo + 1], n, delta, vm)?);
    }
    let (values, vm) = points.into_parts();
    Ok(list_value(values, vm))
}

/// `(lo * (n - delta) + hi * delta) / n`, computed with the data's own operators as CPython's `quantiles` does.
fn interpolate(lo: &Value, hi: &Value, n: i64, delta: i128, vm: &mut VM<'_>) -> RunResult<Value> {
    let lo_weight = int_value(i128::from(n) - delta, vm);
    defer_drop!(lo_weight, vm);
    let hi_weight = int_value(delta, vm);
    defer_drop!(hi_weight, vm);
    let left = lo.py_mul(lo_weight, vm)?;
    defer_drop!(left, vm);
    let right = hi.py_mul(hi_weight, vm)?;
    defer_drop!(right, vm);
    let sum = left.py_add(right, vm)?;
    defer_drop!(sum, vm);
    sum.py_truediv(&Value::Int(n), vm)
}

/// An `int` holding `x`: an `i64` in practice, but never truncated when an exact weight is wider.
fn int_value(x: i128, vm: &mut VM<'_>) -> Value {
    i64::try_from(x).map_or_else(|_| LongInt::new(BigInt::from(x)).into_value(vm.heap), Value::Int)
}

/// `mode` and `multimode`: the most common value(s) of `Counter(iter(data))`, in first-seen order.
fn mode(vm: &mut VM<'_>, args: ArgValues, all: bool) -> RunResult<Value> {
    let data = if all {
        MultimodeArgs::from_args(args, vm)?.data
    } else {
        ModeArgs::from_args(args, vm)?.data
    };
    // CPython counts `iter(data)`, so a mapping contributes its keys rather than being read as counts.
    let iterator = data.into_py_iter(vm)?.require_synchronous(vm)?;
    let counter = counter_init(vm, ArgValues::One(iterator))?;
    defer_drop!(counter, vm);
    let Value::Ref(id) = counter else {
        unreachable!("Counter is a heap value");
    };
    let pairs = {
        let HeapReadOutput::Dict(dict) = vm.heap.read(*id) else {
            unreachable!("Counter is a dict");
        };
        dict.clone_all_pairs(vm)
    }?;
    defer_drop!(pairs, vm);
    let count_of = |count: &Value| if let Value::Int(count) = count { *count } else { 0 };
    let max = pairs.iter().map(|(_, count)| count_of(count)).max();
    match max {
        None if all => Ok(list_value(Vec::new(), vm)),
        None => Err(ExcType::statistics_error("no mode for empty data")),
        Some(max) if all => {
            let is_winner = |(_, count): &&(Value, Value)| count_of(count) == max;
            // Distinct data makes every value a winner, so the result can be as large as the counter.
            let mut winners = reserve_values(pairs.iter().filter(is_winner).count(), vm)?;
            winners.extend(
                pairs
                    .iter()
                    .filter(is_winner)
                    .map(|(value, _)| value.clone_with_heap(vm.heap)),
            );
            Ok(list_value(winners, vm))
        }
        Some(max) => {
            let (first, _) = pairs
                .iter()
                .find(|(_, count)| count_of(count) == max)
                .expect("max came from pairs");
            Ok(first.clone_with_heap(vm.heap))
        }
    }
}

data_args!(ModeArgs, "mode");
data_args!(MultimodeArgs, "multimode");

/// Allocates `items` as a new `list`.
fn list_value(items: Vec<Value>, vm: &mut VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))))
}

/// An empty buffer with room for exactly `count` values, after checking that it fits the memory budget.
fn reserve_values(count: usize, vm: &VM<'_>) -> RunResult<Vec<Value>> {
    vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE))?;
    try_vec_with_capacity(count)
}

/// Mirrors `statistics._sum` over `items`; returns `(total, has_float, count)`.
fn exact_sum(items: &[Value], vm: &VM<'_>) -> RunResult<(Total, bool, usize)> {
    let mut sum = ExactSum::new();
    for (i, item) in items.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(i)?;
        sum.add(item, vm)?;
    }
    Ok(sum.finish())
}

/// The result of `_sum`: an exact total, or the float sum of the non-finite data.
enum Total {
    Exact(Frac),
    Special(f64),
}

/// A running `statistics._sum`: finite data summed exactly, infinities and NaNs as a float.
struct ExactSum {
    /// The exact sum of the finite data.
    finite: Dyadic,
    /// The float sum of the non-finite data, once there is any; it then decides the total.
    special: Option<f64>,
    /// Whether any datum was a `float`, which makes the result a float even when it is integral.
    has_float: bool,
    /// How many data have been added.
    count: usize,
}

impl ExactSum {
    /// An empty sum.
    fn new() -> Self {
        Self {
            finite: Dyadic::new(),
            special: None,
            has_float: false,
            count: 0,
        }
    }

    /// Adds one datum, raising CPython's `TypeError` for a type with no exact ratio.
    fn add(&mut self, value: &Value, vm: &VM<'_>) -> RunResult<()> {
        let (elem, is_float) = element(value, vm)?;
        self.add_elem(elem, is_float);
        Ok(())
    }

    /// Adds one datum already read by [`element`].
    fn add_elem(&mut self, elem: Elem, is_float: bool) {
        self.has_float |= is_float;
        self.count += 1;
        match elem {
            Elem::Finite(num, exp) => self.finite.add(num, exp),
            Elem::Special(x) => self.special = Some(self.special.unwrap_or(0.0) + x),
        }
    }

    /// The total, whether any datum was a float, and how many data there were.
    fn finish(self) -> (Total, bool, usize) {
        let Self {
            finite,
            special,
            has_float,
            count,
        } = self;
        let total = special.map_or_else(|| Total::Exact(finite.into_frac()), Total::Special);
        (total, has_float, count)
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
            Some(n) => Ok((Elem::Finite(n.clone(), 0), false)),
            None => Err(ExcType::type_error(format!(
                "can't convert type '{}' to numerator/denominator",
                value.py_type_name(vm)
            ))),
        },
    }
}

/// A float as an exact `num / 2**exp` in lowest terms, or as itself when it is infinite or NaN.
fn float_element(f: f64) -> Elem {
    if !f.is_finite() {
        return Elem::Special(f);
    }
    let bits = f.to_bits();
    let biased = i64::try_from((bits >> 52) & 0x7ff).expect("11 bits");
    let fraction = bits & ((1_u64 << 52) - 1);
    let (mantissa, exponent) = if biased == 0 {
        (fraction, -1074_i64)
    } else {
        (fraction | (1_u64 << 52), biased - 1075)
    };
    if mantissa == 0 {
        Elem::Finite(BigInt::zero(), 0)
    } else {
        // Strip trailing zero bits so the power of two, and every sum built on it, stays as small as possible.
        let trailing = mantissa.trailing_zeros();
        let exponent = exponent + i64::from(trailing);
        let magnitude = BigInt::from(mantissa >> trailing);
        let num = if bits >> 63 == 1 { -magnitude } else { magnitude };
        if exponent >= 0 {
            Elem::Finite(num << usize::try_from(exponent).expect("non-negative"), 0)
        } else {
            Elem::Finite(num, u32::try_from(-exponent).expect("at most 1074"))
        }
    }
}

/// Mirrors `statistics._convert`: an `int` when exactly integral and every datum was an int,
/// otherwise the correctly rounded float.
fn convert(frac: Frac, has_float: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let frac = frac.reduced();
    if !has_float && frac.den.is_one() {
        Ok(LongInt::new(frac.num).into_value(vm.heap))
    } else {
        Ok(Value::Float(bigint_true_divide(
            &frac.num,
            &frac.den,
            &vm.heap.tracker,
        )?))
    }
}

/// An exact running sum `num / 2**exp` of dyadic rationals, which every finite float is.
///
/// Keeping one power-of-two denominator means adding a datum is a shift and an add, with no gcd until the end.
struct Dyadic {
    num: BigInt,
    exp: u32,
}

impl Dyadic {
    /// Zero.
    fn new() -> Self {
        Self {
            num: BigInt::zero(),
            exp: 0,
        }
    }

    /// Adds `num / 2**exp`, rescaling whichever side has the smaller power of two.
    fn add(&mut self, num: BigInt, exp: u32) {
        if exp > self.exp {
            self.num <<= (exp - self.exp) as usize;
            self.exp = exp;
            self.num += num;
        } else {
            self.num += num << (self.exp - exp) as usize;
        }
    }

    /// The sum as a [`Frac`].
    fn into_frac(self) -> Frac {
        Frac::dyadic(self.num, self.exp)
    }
}

/// A fraction with a positive denominator, not necessarily in lowest terms, standing in for `fractions.Fraction`.
struct Frac {
    num: BigInt,
    den: BigInt,
}

impl Frac {
    /// `num / 2**exp`.
    fn dyadic(num: BigInt, exp: u32) -> Self {
        Self {
            num,
            den: BigInt::one() << exp,
        }
    }

    /// `self * other`, unreduced.
    fn mul(&self, other: &Self) -> Self {
        Self {
            num: &self.num * &other.num,
            den: &self.den * &other.den,
        }
    }

    /// `self - other`, unreduced.
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
            self
        } else {
            Self {
                num: self.num / &gcd,
                den: self.den / gcd,
            }
        }
    }
}
