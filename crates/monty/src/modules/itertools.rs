//! Implementation of Python's `itertools` module.
//!
//! A subset so far — see `limitations/itertools.md` for what is implemented and
//! what is not. Unimplemented names are absent from the namespace rather than
//! stubbed, so they raise `AttributeError` up front. See
//! [`crate::types::itertools`] for why the family shares one `HeapData` variant.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{
        ItertoolsIter, Module, Type,
        itertools::{Chain, Compress, Count, Cycle, Islice, Pairwise, Repeat},
    },
    value::Value,
};

/// `itertools` module functions — each variant is a Python-visible callable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ItertoolsFunctions {
    Count,
    Repeat,
    Pairwise,
    Compress,
    Islice,
    Chain,
    Cycle,
}

/// Static mapping of attribute names to functions for module creation.
const ITERTOOLS_FUNCTIONS: &[(StaticStrings, ItertoolsFunctions)] = &[
    (StaticStrings::Count, ItertoolsFunctions::Count),
    (StaticStrings::Repeat, ItertoolsFunctions::Repeat),
    (StaticStrings::Pairwise, ItertoolsFunctions::Pairwise),
    (StaticStrings::Compress, ItertoolsFunctions::Compress),
    (StaticStrings::Islice, ItertoolsFunctions::Islice),
    (StaticStrings::Chain, ItertoolsFunctions::Chain),
    (StaticStrings::Cycle, ItertoolsFunctions::Cycle),
];

/// Creates the `itertools` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Itertools);

    for (name, func) in ITERTOOLS_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Itertools(*func)), vm);
    }

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to an `itertools` module function.
pub(super) fn call(vm: &mut VM<'_>, function: ItertoolsFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        ItertoolsFunctions::Count => call_count(vm, args),
        ItertoolsFunctions::Repeat => call_repeat(vm, args),
        ItertoolsFunctions::Pairwise => call_pairwise(vm, args),
        ItertoolsFunctions::Compress => call_compress(vm, args),
        ItertoolsFunctions::Islice => call_islice(vm, args),
        ItertoolsFunctions::Chain => call_chain(vm, args),
        ItertoolsFunctions::Cycle => call_cycle(vm, args),
    }
}

/// Argument shape for `count(start=0, step=1)`.
///
/// CPython parses this with `"|OO:count"`, so errors carry the function name
/// (`style = c_named`) and arity counts positionals + keywords together
/// (`at_most_total`): `count(1, 2, step=3)` reports three arguments.
#[derive(FromArgs)]
#[from_args(name = "count", style = c_named, at_most_total)]
struct CountArgs {
    #[from_args(default = Value::Int(0))]
    start: Value,
    #[from_args(default = Value::Int(1))]
    step: Value,
}

/// `itertools.count(start=0, step=1)` — an infinite arithmetic progression.
fn call_count(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CountArgs { start, step } = CountArgs::from_args(args, vm)?;
    // CPython's `count_new` runs one `PyNumber_Check` over both arguments after
    // parsing, so a bad `start` and a bad `step` give the same single message.
    // Rejecting here means `py_next` can add without a defined-ness check.
    if is_number(&start, vm) && is_number(&step, vm) {
        let iter = ItertoolsIter::Count(Count::new(normalize_bool(start), normalize_bool(step)));
        Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
    } else {
        start.drop_with(vm);
        step.drop_with(vm);
        Err(ExcType::type_error("a number is required"))
    }
}

/// Argument shape for `repeat(object, times=?)`.
///
/// CPython parses this with `"O|n:repeat"` — see [`CountArgs`] for why that
/// means `c_named` + `at_most_total`. `times` stays a raw `Value` so the
/// `__index__` coercion (and its `OverflowError`) happens in the body.
#[derive(FromArgs)]
#[from_args(name = "repeat", style = c_named, at_most_total)]
struct RepeatArgs {
    object: Value,
    #[from_args(default)]
    times: Option<Value>,
}

/// `itertools.repeat(object, times=?)` — `object` forever, or `times` times.
fn call_repeat(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let RepeatArgs { object, times } = RepeatArgs::from_args(args, vm)?;
    let remaining = match times {
        None => None,
        Some(times) => {
            let count = repeat_times(&times, vm);
            times.drop_with(vm);
            match count {
                Ok(count) => Some(count),
                Err(error) => {
                    object.drop_with(vm);
                    return Err(error);
                }
            }
        }
    };
    let iter = ItertoolsIter::Repeat(Repeat::new(object, remaining));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Whether `value` satisfies CPython's `PyNumber_Check` for `count()`.
///
/// Monty's numeric types are exactly `int` (immediate, interned big, or heap
/// `LongInt` — all reported as [`Type::Int`]), `float`, and `bool`.
fn is_number(value: &Value, vm: &VM<'_>) -> bool {
    matches!(value.py_type_heap(vm.heap), Type::Int | Type::Float | Type::Bool)
}

/// Widens a `bool` start/step to the `int` it stands for.
///
/// CPython's `count_new` does the same — `repr(count(True))` is `count(1)`, not
/// `count(True)` — so this is parity rather than a shortcut. It also keeps
/// `py_next` off Monty's unsupported `bool + int` path.
fn normalize_bool(value: Value) -> Value {
    match value {
        Value::Bool(b) => Value::Int(i64::from(b)),
        other => other,
    }
}

/// Coerces `repeat`'s `times` the way CPython's `n` format unit does.
///
/// Negative counts clamp to zero (`repeat(x, -1)` is empty) and a `times` too
/// large for a machine integer raises `OverflowError`, matching the conversion
/// to `Py_ssize_t`. `bool` is accepted because it is an `int` subclass.
fn repeat_times(value: &Value, vm: &VM<'_>) -> RunResult<usize> {
    let count = match value {
        Value::Bool(b) => i64::from(*b),
        other => other.as_int(vm)?,
    };
    // Saturates rather than wrapping on a 32-bit host, where a `times` between
    // `usize::MAX` and `i64::MAX` is still effectively infinite.
    Ok(usize::try_from(count.max(0)).unwrap_or(usize::MAX))
}

/// Argument shape for `pairwise(iterable)`.
///
/// CPython parses this with `PyArg_UnpackTuple`, so arity errors read
/// `pairwise expected 1 argument, got 2` and any keyword is rejected wholesale
/// with `pairwise() takes no keyword arguments`.
#[derive(FromArgs)]
#[from_args(name = "pairwise", style = unpack)]
struct PairwiseArgs {
    #[from_args(pos_only)]
    iterable: Value,
}

/// `itertools.pairwise(iterable)` — successive overlapping pairs.
fn call_pairwise(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PairwiseArgs { iterable } = PairwiseArgs::from_args(args, vm)?;
    // Resolve to an iterator up front, as CPython does: a non-iterable raises
    // here rather than on the first `next()`.
    let source = iterable.into_py_iter(vm)?;
    let iter = ItertoolsIter::Pairwise(Pairwise::new(source));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `compress(data, selectors)`.
///
/// CPython parses this with `PyArg_ParseTupleAndKeywords`, so both parameters
/// are also accepted by keyword and arity counts positionals + keywords
/// together: `compress([1], [1], data=[1])` reports three arguments.
#[derive(FromArgs)]
#[from_args(name = "compress", style = c_named, at_most_total)]
struct CompressArgs {
    data: Value,
    selectors: Value,
}

/// `itertools.compress(data, selectors)` — data items with a truthy selector.
fn call_compress(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CompressArgs { data, selectors } = CompressArgs::from_args(args, vm)?;
    // `into_py_iter` consumes its receiver, so each conversion guards the other
    // argument: whichever is not being converted is released if this one raises.
    let mut guard = DropGuard::new(selectors, vm);
    let data = data.into_py_iter(guard.ctx())?;
    let (selectors, vm) = guard.into_parts();
    let mut guard = DropGuard::new(data, vm);
    let selectors = selectors.into_py_iter(guard.ctx())?;
    let (data, vm) = guard.into_parts();
    let iter = ItertoolsIter::Compress(Compress::new(data, selectors));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `islice(iterable, [start,] stop[, step])`.
///
/// CPython uses `PyArg_UnpackTuple` with a 2..4 arity, so the trailing three
/// slots are read positionally and their *meaning* depends on how many were
/// given — resolved in the body by [`islice_bounds`], not by the binder.
#[derive(FromArgs)]
#[from_args(name = "islice", style = unpack)]
struct IsliceArgs {
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(pos_only)]
    first: Value,
    #[from_args(pos_only, default)]
    second: Option<Value>,
    #[from_args(pos_only, default)]
    third: Option<Value>,
}

/// `itertools.islice(iterable, [start,] stop[, step])` — a slice of an iterator.
fn call_islice(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IsliceArgs {
        iterable,
        first,
        second,
        third,
    } = IsliceArgs::from_args(args, vm)?;

    // `iterable` is guarded across the bounds check so an invalid slot raises
    // only after the three overloaded arguments have been released.
    let mut guard = DropGuard::new(iterable, vm);
    let vm = guard.ctx();
    let bounds = islice_bounds(&first, second.as_ref(), third.as_ref(), vm);
    first.drop_with(vm);
    second.drop_with(vm);
    third.drop_with(vm);
    let (start, stop, step) = bounds?;
    let (iterable, vm) = guard.into_parts();

    let source = iterable.into_py_iter(vm)?;
    let iter = ItertoolsIter::Islice(Islice::new(source, start, stop, step));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Resolves `islice`'s overloaded positional slots into `(start, stop, step)`.
///
/// With two arguments the second is `stop`; with three or more it is `start`.
/// CPython words the two failures differently, so they are not interchangeable.
fn islice_bounds(
    first: &Value,
    second: Option<&Value>,
    third: Option<&Value>,
    vm: &VM<'_>,
) -> RunResult<(usize, Option<usize>, usize)> {
    match second {
        None => match islice_index(first, vm) {
            IsliceBound::Unbounded => Ok((0, None, 1)),
            IsliceBound::Index(stop) => Ok((0, Some(stop), 1)),
            IsliceBound::Invalid => Err(ExcType::islice_bad_stop()),
        },
        Some(second) => {
            // A `start` of `None` means "from the beginning", as in a slice.
            let start = match islice_index(first, vm) {
                IsliceBound::Unbounded => 0,
                IsliceBound::Index(start) => start,
                IsliceBound::Invalid => return Err(ExcType::islice_bad_indices()),
            };
            let stop = match islice_index(second, vm) {
                IsliceBound::Unbounded => None,
                IsliceBound::Index(stop) => Some(stop),
                IsliceBound::Invalid => return Err(ExcType::islice_bad_indices()),
            };
            // A step of `None` is 1; zero and negatives are rejected outright.
            let step = match third.map(|third| islice_index(third, vm)) {
                None | Some(IsliceBound::Unbounded) => 1,
                Some(IsliceBound::Index(step)) if step > 0 => step,
                Some(_) => return Err(ExcType::islice_bad_step()),
            };
            Ok((start, stop, step))
        }
    }
}

/// One parsed `islice` bound.
///
/// Three-way rather than `Option<usize>`: `stop` accepts an explicit `None`
/// where `step` does not, so the two rejections must stay distinguishable.
enum IsliceBound {
    /// Python `None` — unbounded for `stop`, the default for `start`/`step`.
    Unbounded,
    /// An index in `0 ..= sys.maxsize`.
    Index(usize),
    /// Neither, so the caller raises.
    Invalid,
}

/// Reads one `islice` bound; whether `Unbounded` is allowed is the caller's
/// business, and differs per parameter.
fn islice_index(value: &Value, vm: &VM<'_>) -> IsliceBound {
    let index = match value {
        Value::None => return IsliceBound::Unbounded,
        Value::Bool(b) => i64::from(*b),
        other => match other.as_int(vm) {
            Ok(index) => index,
            Err(_) => return IsliceBound::Invalid,
        },
    };
    usize::try_from(index).map_or(IsliceBound::Invalid, IsliceBound::Index)
}

/// `itertools.chain(*iterables)` — each argument's items, back to back.
///
/// Uses `into_pos_only` because the derive cannot express unbounded `*args`
/// with no keywords: `style = unpack` models a fixed `min..max` and rejects
/// `varargs`.
fn call_chain(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let iterables: Vec<Value> = args.into_pos_only("chain", vm.heap)?.collect();
    // Arguments are NOT resolved here: CPython calls `iter()` on each only as it
    // reaches it, so `chain([1], 5)` constructs and raises mid-consumption.
    let iter = ItertoolsIter::Chain(Chain::new(iterables));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `cycle(iterable)`.
///
/// `PyArg_UnpackTuple` like `pairwise`, so arity reads `cycle expected 1
/// argument, got 2` and keywords are rejected wholesale.
#[derive(FromArgs)]
#[from_args(name = "cycle", style = unpack)]
struct CycleArgs {
    #[from_args(pos_only)]
    iterable: Value,
}

/// `itertools.cycle(iterable)` — the source's items, repeating forever.
fn call_cycle(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CycleArgs { iterable } = CycleArgs::from_args(args, vm)?;
    // Unlike `chain`, CPython resolves eagerly here, so `cycle(5)` raises now.
    let source = iterable.into_py_iter(vm)?;
    let iter = ItertoolsIter::Cycle(Cycle::new(source));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}
