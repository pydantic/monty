//! Implementation of Python's `itertools` module.
//!
//! A subset so far — see `limitations/itertools.md` for what is implemented and
//! what is not. Unimplemented names are absent from the namespace rather than
//! stubbed, so they raise `AttributeError` up front. See
//! [`crate::types::itertools`] for why the family shares one `HeapData` variant.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    builtins::Builtins,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HEAP_ENTRY_SIZE, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource_checks::check_estimated_size,
    types::{
        ItertoolsIter, Module, TupleVec, Type, allocate_tuple,
        iter::collect_owned_iterable,
        itertools::{
            Accumulate, Batched, Chain, Combinations, Compress, Count, Cycle, DropWhile, FilterFalse, GroupBy, Islice,
            Pairwise, Permutations, Product, Repeat, StarMap, TakeWhile, ZipLongest, tee,
        },
    },
    value::{VALUE_SIZE, Value},
};

/// The `itertools` callables that are not type objects.
///
/// CPython models all but one of this module's names as classes, and so does
/// Monty — `itertools.count` IS `Type::ItertoolsCount`, constructed through
/// [`construct`]. Only `tee` is a plain function there, and `from_iterable` is
/// reached through the `chain` type rather than the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ItertoolsFunctions {
    Tee,
    /// `chain.from_iterable`, an attribute of the `chain` type rather than a
    /// name in the module — resolved by `Value::py_getattr`.
    #[strum(serialize = "from_iterable")]
    ChainFromIterable,
}

/// The module's type objects, by the name each is bound to.
const ITERTOOLS_TYPES: &[(StaticStrings, Type)] = &[
    (StaticStrings::Count, Type::ItertoolsCount),
    (StaticStrings::Repeat, Type::ItertoolsRepeat),
    (StaticStrings::Pairwise, Type::ItertoolsPairwise),
    (StaticStrings::Compress, Type::ItertoolsCompress),
    (StaticStrings::Islice, Type::ItertoolsIslice),
    (StaticStrings::Chain, Type::ItertoolsChain),
    (StaticStrings::Cycle, Type::ItertoolsCycle),
    (StaticStrings::Takewhile, Type::ItertoolsTakeWhile),
    (StaticStrings::Dropwhile, Type::ItertoolsDropWhile),
    (StaticStrings::Filterfalse, Type::ItertoolsFilterFalse),
    (StaticStrings::Starmap, Type::ItertoolsStarMap),
    (StaticStrings::Accumulate, Type::ItertoolsAccumulate),
    (StaticStrings::Batched, Type::ItertoolsBatched),
    (StaticStrings::ZipLongest, Type::ItertoolsZipLongest),
    (StaticStrings::Combinations, Type::ItertoolsCombinations),
    (
        StaticStrings::CombinationsWithReplacement,
        Type::ItertoolsCombinationsWithReplacement,
    ),
    (StaticStrings::Permutations, Type::ItertoolsPermutations),
    (StaticStrings::Product, Type::ItertoolsProduct),
    (StaticStrings::Groupby, Type::ItertoolsGroupBy),
    (StaticStrings::Grouper, Type::ItertoolsGrouper),
    (StaticStrings::TeeType, Type::ItertoolsTee),
    (StaticStrings::TeeDataObject, Type::ItertoolsTeeDataObject),
];

/// Creates the `itertools` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Itertools);

    for (name, type_) in ITERTOOLS_TYPES {
        module.set_attr(*name, Value::Builtin(Builtins::Type(*type_)), vm);
    }
    module.set_attr(
        StaticStrings::Tee,
        Value::ModuleFunction(ModuleFunctions::Itertools(ItertoolsFunctions::Tee)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to one of the module's two plain functions.
pub(crate) fn call(vm: &mut VM<'_>, function: ItertoolsFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        ItertoolsFunctions::Tee => call_tee(vm, args),
        ItertoolsFunctions::ChainFromIterable => call_chain_from_iterable(vm, args),
    }
}

/// Constructs one of the module's iterators, reached through [`Type::call`].
///
/// # Panics
/// Panics on a type this module does not own, which `Type::call` never passes.
pub(crate) fn construct(type_: Type, vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    match type_ {
        Type::ItertoolsCount => call_count(vm, args),
        Type::ItertoolsRepeat => call_repeat(vm, args),
        Type::ItertoolsPairwise => call_pairwise(vm, args),
        Type::ItertoolsCompress => call_compress(vm, args),
        Type::ItertoolsIslice => call_islice(vm, args),
        Type::ItertoolsChain => call_chain(vm, args),
        Type::ItertoolsCycle => call_cycle(vm, args),
        Type::ItertoolsTakeWhile => call_takewhile(vm, args),
        Type::ItertoolsDropWhile => call_dropwhile(vm, args),
        Type::ItertoolsFilterFalse => call_filterfalse(vm, args),
        Type::ItertoolsStarMap => call_starmap(vm, args),
        Type::ItertoolsAccumulate => call_accumulate(vm, args),
        Type::ItertoolsBatched => call_batched(vm, args),
        Type::ItertoolsZipLongest => call_zip_longest(vm, args),
        Type::ItertoolsCombinations => call_combinations(vm, args),
        Type::ItertoolsCombinationsWithReplacement => call_combinations_with_replacement(vm, args),
        Type::ItertoolsPermutations => call_permutations(vm, args),
        Type::ItertoolsProduct => call_product(vm, args),
        Type::ItertoolsGroupBy => call_groupby(vm, args),
        // Exposed under their CPython names so `type()` and `isinstance()`
        // work, but not constructible here — CPython builds them from the
        // arguments its internals use. See `limitations/itertools.md`.
        Type::ItertoolsGrouper | Type::ItertoolsTee | Type::ItertoolsTeeDataObject => {
            args.drop_with(vm);
            Err(ExcType::type_error_not_callable(&type_.name(vm.heap, vm.interns)))
        }
        other => unreachable!("{other} is not an itertools type"),
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
fn repeat_times(value: &Value, vm: &mut VM<'_>) -> RunResult<usize> {
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
    vm: &mut VM<'_>,
) -> RunResult<(usize, Option<usize>, usize)> {
    match second {
        None => match islice_index(first, vm)? {
            IsliceBound::Unbounded => Ok((0, None, 1)),
            IsliceBound::Index(stop) => Ok((0, Some(stop), 1)),
            IsliceBound::Invalid => Err(ExcType::islice_bad_stop()),
        },
        Some(second) => {
            // A `start` of `None` means "from the beginning", as in a slice.
            let start = match islice_index(first, vm)? {
                IsliceBound::Unbounded => 0,
                IsliceBound::Index(start) => start,
                IsliceBound::Invalid => return Err(ExcType::islice_bad_indices()),
            };
            let stop = match islice_index(second, vm)? {
                IsliceBound::Unbounded => None,
                IsliceBound::Index(stop) => Some(stop),
                IsliceBound::Invalid => return Err(ExcType::islice_bad_indices()),
            };
            // A step of `None` is 1; zero and negatives are rejected outright.
            let step = match third.map(|third| islice_index(third, vm)).transpose()? {
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
fn islice_index(value: &Value, vm: &mut VM<'_>) -> RunResult<IsliceBound> {
    let index = match value {
        Value::None => return Ok(IsliceBound::Unbounded),
        Value::Bool(b) => i64::from(*b),
        // A raising `__index__` propagates; only a type mismatch is `Invalid`,
        // which the caller words per parameter.
        other => match other.as_int(vm) {
            Ok(index) => index,
            Err(RunError::Exc(_)) => return Ok(IsliceBound::Invalid),
            Err(e) => return Err(e),
        },
    };
    Ok(usize::try_from(index).map_or(IsliceBound::Invalid, IsliceBound::Index))
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

/// Argument shape shared by `takewhile`, `dropwhile`, `filterfalse` and
/// `starmap`.
///
/// All four are `PyArg_UnpackTuple(args, name, 2, 2, ...)` in CPython, so both
/// slots are positional-only, arity reads `takewhile expected 2 arguments, got
/// 1`, and keywords are rejected wholesale. The macro embeds the name, hence
/// one struct per callable rather than one shared struct.
macro_rules! callable_and_iterable_args {
    ($struct_name:ident, $py_name:literal) => {
        #[derive(FromArgs)]
        #[from_args(name = $py_name, style = unpack)]
        struct $struct_name {
            #[from_args(pos_only)]
            callable: Value,
            #[from_args(pos_only)]
            iterable: Value,
        }
    };
}

callable_and_iterable_args!(TakeWhileArgs, "takewhile");
callable_and_iterable_args!(DropWhileArgs, "dropwhile");
callable_and_iterable_args!(FilterFalseArgs, "filterfalse");
callable_and_iterable_args!(StarMapArgs, "starmap");

/// `itertools.takewhile(predicate, iterable)` — the leading passing run.
fn call_takewhile(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let TakeWhileArgs { callable, iterable } = TakeWhileArgs::from_args(args, vm)?;
    let (predicate, source) = resolve_source(callable, iterable, vm)?;
    let iter = ItertoolsIter::TakeWhile(TakeWhile::new(predicate, source));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// `itertools.dropwhile(predicate, iterable)` — everything past that run.
fn call_dropwhile(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DropWhileArgs { callable, iterable } = DropWhileArgs::from_args(args, vm)?;
    let (predicate, source) = resolve_source(callable, iterable, vm)?;
    let iter = ItertoolsIter::DropWhile(DropWhile::new(predicate, source));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// `itertools.filterfalse(predicate, iterable)` — the items it rejects.
fn call_filterfalse(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FilterFalseArgs { callable, iterable } = FilterFalseArgs::from_args(args, vm)?;
    let (predicate, source) = resolve_source(callable, iterable, vm)?;
    let iter = ItertoolsIter::FilterFalse(FilterFalse::new(predicate, source));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// `itertools.starmap(function, iterable)` — each item spread as arguments.
fn call_starmap(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let StarMapArgs { callable, iterable } = StarMapArgs::from_args(args, vm)?;
    let (function, source) = resolve_source(callable, iterable, vm)?;
    let iter = ItertoolsIter::StarMap(StarMap::new(function, source));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `accumulate(iterable, func=None, *, initial=None)`.
///
/// Argument Clinic, so both leading slots accept keywords
/// (`accumulate(iterable=[1])` works). Clinic shares `_PyArg_UnpackKeywords`
/// with the named C family, so `c_named` — not the default style, which is the
/// `_PyArg_CheckPositional` wording — gives the `takes at most 2 positional
/// arguments (3 given)` form, pivoting to a total count once kwargs push the
/// overflow past every slot. `func` is never type-checked here: CPython only
/// discovers a non-callable when the second item arrives.
#[derive(FromArgs)]
#[from_args(name = "accumulate", style = c_named)]
struct AccumulateArgs {
    #[from_args(static_string = "IterableArg")]
    iterable: Value,
    #[from_args(default = Value::None)]
    func: Value,
    #[from_args(kw_only, default = Value::None)]
    initial: Value,
}

/// `itertools.accumulate(iterable, func=None, *, initial=None)` — running totals.
fn call_accumulate(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let AccumulateArgs {
        iterable,
        func,
        initial,
    } = AccumulateArgs::from_args(args, vm)?;
    // `func` is held across the resolve so a non-iterable releases it too.
    let (func, source) = match resolve_source(func, iterable, vm) {
        Ok(resolved) => resolved,
        Err(error) => {
            initial.drop_with(vm);
            return Err(error);
        }
    };
    // An explicit `initial=None` is no initial at all, as CPython's `!= Py_None`
    // check makes it — so `accumulate([], initial=None)` yields nothing.
    let initial = match initial {
        Value::None => None,
        initial => Some(initial),
    };
    let iter = ItertoolsIter::Accumulate(Box::new(Accumulate::new(source, func, initial)));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `batched(iterable, n, *, strict=False)`.
///
/// Argument Clinic, so `n` accepts a keyword and a missing one reports
/// `missing required argument 'n' (pos 2)` — see [`AccumulateArgs`] for why
/// that means `c_named`. Both positionals are required, so the overflow says
/// "exactly" where `accumulate`'s says "at most". `n` stays a raw `Value` because it
/// needs `as_int`'s message rather than the binder's, as `repeat`'s `times`
/// does; `strict` is a [`LaxBool`] so CPython's `bool()`-style truth test
/// happens in the binder, which releases the value on both paths.
#[derive(FromArgs)]
#[from_args(name = "batched", style = c_named)]
struct BatchedArgs {
    #[from_args(static_string = "IterableArg")]
    iterable: Value,
    n: Value,
    #[from_args(kw_only, default = LaxBool::new(false))]
    strict: LaxBool,
}

/// `itertools.batched(iterable, n, *, strict=False)` — consecutive n-tuples.
fn call_batched(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let BatchedArgs { iterable, n, strict } = BatchedArgs::from_args(args, vm)?;
    let strict = strict.bool();
    // `n` is validated before the iterable is resolved, matching CPython's
    // clinic converter, which runs over every argument before the body.
    let size = batched_n(&n, vm);
    n.drop_with(vm);
    let size = match size {
        Ok(size) => size,
        Err(error) => {
            iterable.drop_with(vm);
            return Err(error);
        }
    };

    let source = iterable.into_py_iter(vm)?;
    let iter = ItertoolsIter::Batched(Batched::new(source, size, strict));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Coerces `batched`'s `n` and enforces CPython's "at least one" floor.
fn batched_n(value: &Value, vm: &mut VM<'_>) -> RunResult<usize> {
    let n = ssize_arg(value, vm)?;
    if n < 1 {
        Err(ExcType::batched_bad_n())
    } else {
        Ok(n.cast_unsigned())
    }
}

/// Coerces an argument the way CPython's `Py_ssize_t` converters do.
///
/// Needs `&mut VM` because `as_int` dispatches `__index__`, re-entering the
/// interpreter; the caller's other arguments are owned, so that cannot
/// invalidate them. `as_int` raises `OverflowError` past `i64` as the
/// conversion does, and the bound below reports the same for the range between
/// `isize` and `i64` that only a 32-bit host (`wasm32-wasip1`) has —
/// `batched('AB', 2**40)` there.
fn ssize_arg(value: &Value, vm: &mut VM<'_>) -> RunResult<isize> {
    let n = match value {
        Value::Bool(b) => i64::from(*b),
        other => other.as_int(vm)?,
    };
    isize::try_from(n).map_err(|_| ExcType::overflow_c_ssize_t())
}

/// Argument shape for `zip_longest(*iterables, fillvalue=None)`.
///
/// The only adaptor with both `*args` and a keyword. CPython hand-rolls the
/// parse rather than using a parser family, and its rejection names no
/// argument (`zip_longest() got an unexpected keyword argument`) — the derive
/// appends the offending name, which `limitations/itertools.md` records.
#[derive(FromArgs)]
#[from_args(name = "zip_longest")]
struct ZipLongestArgs {
    #[from_args(varargs)]
    iterables: Vec<Value>,
    #[from_args(kw_only, default = Value::None)]
    fillvalue: Value,
}

/// `itertools.zip_longest(*iterables, fillvalue=None)` — zip to the longest.
fn call_zip_longest(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ZipLongestArgs { iterables, fillvalue } = ZipLongestArgs::from_args(args, vm)?;
    // Every argument is resolved up front, unlike `chain`'s lazy ones, so a
    // later non-iterable must release the arguments never reached as well. The
    // `Value::None` swap leaves those in the guard's vec: draining it instead
    // would hand the tail to a Rust `Drop`, which cannot `drop_with`.
    defer_drop_mut!(iterables, vm);
    let mut guard = DropGuard::new(Vec::with_capacity(iterables.len()), vm);
    for slot in iterables.iter_mut() {
        let iterable = mem::replace(slot, Value::None);
        let (resolved, vm) = guard.as_parts_mut();
        match iterable.into_py_iter(vm) {
            Ok(source) => resolved.push(source),
            Err(error) => {
                fillvalue.drop_with(vm);
                return Err(error);
            }
        }
    }
    let (sources, vm) = guard.into_parts();
    let iter = ItertoolsIter::ZipLongest(ZipLongest::new(sources, fillvalue));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `combinations(iterable, r)` and its
/// `_with_replacement` twin.
///
/// Argument Clinic with both parameters keyword-capable, so `c_named` (see
/// [`AccumulateArgs`]), and `at_most_total` because a stray keyword reports
/// `takes at most 2 arguments (3 given)`. `r` stays a raw `Value`: its
/// `Py_ssize_t` conversion runs in the body, ahead of the pool being collected
/// and before the sign check, as the clinic converter and body order them.
macro_rules! iterable_and_r_args {
    ($struct_name:ident, $py_name:literal) => {
        #[derive(FromArgs)]
        #[from_args(name = $py_name, style = c_named, at_most_total)]
        struct $struct_name {
            #[from_args(static_string = "IterableArg")]
            iterable: Value,
            r: Value,
        }
    };
}

iterable_and_r_args!(CombinationsArgs, "combinations");
iterable_and_r_args!(CombinationsWithReplacementArgs, "combinations_with_replacement");

/// `itertools.combinations(iterable, r)` — `r`-length subsequences.
fn call_combinations(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CombinationsArgs { iterable, r } = CombinationsArgs::from_args(args, vm)?;
    build_combinations(iterable, r, false, vm)
}

/// `itertools.combinations_with_replacement(iterable, r)` — the same, with
/// each item allowed to repeat.
fn call_combinations_with_replacement(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CombinationsWithReplacementArgs { iterable, r } = CombinationsWithReplacementArgs::from_args(args, vm)?;
    build_combinations(iterable, r, true, vm)
}

/// Collects the pool and builds either `combinations` flavour.
///
/// `r` is converted before the pool is collected, but its sign is checked
/// after: `combinations(5, -1)` reports the non-iterable, `combinations(5, 'x')`
/// the bad `r`.
fn build_combinations(iterable: Value, r: Value, replacement: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let converted = ssize_arg(&r, vm);
    r.drop_with(vm);
    let r = match converted {
        Ok(r) => r,
        Err(error) => {
            iterable.drop_with(vm);
            return Err(error);
        }
    };
    let pool: Vec<Value> = collect_owned_iterable(iterable, vm)?;
    let mut pool_guard = DropGuard::new(pool, vm);
    let (pool, vm) = pool_guard.as_parts_mut();
    let r = usize::try_from(r).map_err(|_| ExcType::combinatoric_negative_r())?;
    // Without replacement `r` past the pool yields nothing and allocates
    // nothing; with it `r` is unbounded, so the index vector and the result
    // tuples it sizes are preflighted (`combinations_with_replacement('a', 10**9)`).
    // An `r` whose vector no allocation could address is CPython's bare
    // `MemoryError` rather than a limit refusal, and stops `Vec` panicking on
    // a capacity that overflows.
    if replacement && !pool.is_empty() {
        let bytes = index_bytes(r).ok_or_else(ExcType::allocation_too_large)?;
        check_estimated_size(bytes.saturating_add(r.saturating_mul(VALUE_SIZE)), &vm.heap.tracker)?;
    }
    let (pool, vm) = pool_guard.into_parts();
    let iter = ItertoolsIter::Combinations(Box::new(Combinations::new(pool, r, replacement)));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `permutations(iterable, r=None)`.
///
/// Clinic and keyword-capable like [`CombinationsArgs`]. `r` is an `object`
/// to clinic, so the body inspects it — and only once the pool exists.
#[derive(FromArgs)]
#[from_args(name = "permutations", style = c_named, at_most_total)]
struct PermutationsArgs {
    #[from_args(static_string = "IterableArg")]
    iterable: Value,
    #[from_args(default = Value::None)]
    r: Value,
}

/// `itertools.permutations(iterable, r=None)` — `r`-length orderings.
fn call_permutations(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PermutationsArgs { iterable, r } = PermutationsArgs::from_args(args, vm)?;
    let mut r_guard = DropGuard::new(r, vm);
    let pool: Vec<Value> = collect_owned_iterable(iterable, r_guard.ctx())?;
    let (r, vm) = r_guard.into_parts();
    let mut pool_guard = DropGuard::new(pool, vm);
    let (pool, vm) = pool_guard.as_parts_mut();
    let r = permutations_r(r, pool.len(), vm)?;
    let (pool, vm) = pool_guard.into_parts();
    let iter = ItertoolsIter::Permutations(Box::new(Permutations::new(pool, r)));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Resolves `permutations`' `r`: `None` means every item, and only a real
/// `int` is accepted.
///
/// CPython checks the type rather than calling `__index__`, so `1.0` and an
/// `__index__` object are both `Expected int as r`; a `bool` passes as an
/// `int` subclass.
fn permutations_r(r: Value, n: usize, vm: &mut VM<'_>) -> RunResult<usize> {
    defer_drop!(r, vm);
    if matches!(r, Value::None) {
        Ok(n)
    } else if matches!(r.py_type_heap(vm.heap), Type::Int | Type::Bool) {
        usize::try_from(ssize_arg(r, vm)?).map_err(|_| ExcType::combinatoric_negative_r())
    } else {
        Err(ExcType::permutations_bad_r())
    }
}

/// Argument shape for `product(*iterables, repeat=1)`.
///
/// CPython hand-parses this: the positionals are taken as they come, and the
/// keywords alone go through `PyArg_ParseTupleAndKeywords`, whose one-slot
/// `kwlist` gives the derive's `got an unexpected keyword argument 'x'`
/// wording for a bad name. `repeat` stays a raw `Value` for the `Py_ssize_t`
/// conversion in the body.
#[derive(FromArgs)]
#[from_args(name = "product")]
struct ProductArgs {
    #[from_args(varargs)]
    iterables: Vec<Value>,
    #[from_args(kw_only, default = Value::Int(1))]
    repeat: Value,
}

/// `itertools.product(*iterables, repeat=1)` — the cartesian product.
fn call_product(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ProductArgs { iterables, repeat } = ProductArgs::from_args(args, vm)?;
    // Each argument is moved out of the vec as it is collected; the ones a
    // `repeat` of zero never reaches stay in it and are released by the guard,
    // as `zip_longest` does — see there for why not `drain`.
    defer_drop_mut!(iterables, vm);
    let converted = ssize_arg(&repeat, vm);
    repeat.drop_with(vm);
    let repeat = usize::try_from(converted?).map_err(|_| ExcType::product_negative_repeat())?;

    // `repeat=0` never touches the arguments: CPython skips collecting them,
    // so `product(5, repeat=0)` is the single empty tuple, not a `TypeError`.
    let width = if repeat == 0 { 0 } else { iterables.len() };
    let slots = product_slots(width, repeat)?;

    let mut pools_guard = DropGuard::new(Vec::with_capacity(width), vm);
    for slot in iterables.iter_mut().take(width) {
        let iterable = mem::replace(slot, Value::None);
        let (pools, vm) = pools_guard.as_parts_mut();
        let pool: Vec<Value> = collect_owned_iterable(iterable, vm)?;
        pools.push(pool);
    }
    // The index vector and every result tuple scale with `repeat` alone, so
    // they are preflighted (`product('ab', repeat=10**9)`). An empty pool
    // empties the product, and `Product::new` then allocates nothing at all —
    // so `product([], repeat=10**9)` must not be refused for a cost it never
    // pays.
    let (pools, vm) = pools_guard.as_parts_mut();
    if pools.iter().all(|pool| !pool.is_empty()) {
        check_estimated_size(slots.saturating_mul(size_of::<usize>() + VALUE_SIZE), &vm.heap.tracker)?;
    }
    let (pools, vm) = pools_guard.into_parts();
    let iter = ItertoolsIter::Product(Box::new(Product::new(pools, slots)));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// The number of slots `product` will step, rejecting a `repeat` that puts the
/// index vector beyond what a machine integer can address.
///
/// CPython bounds `nargs * repeat * sizeof(Py_ssize_t)` by `PY_SSIZE_T_MAX` and
/// raises before it touches the arguments. The count this returns is the one
/// [`Product::new`] is given, so the multiplication happens once, here, where
/// it is checked.
fn product_slots(width: usize, repeat: usize) -> RunResult<usize> {
    match width.checked_mul(repeat) {
        Some(slots) if index_bytes(slots).is_some() => Ok(slots),
        _ => Err(ExcType::product_repeat_too_large()),
    }
}

/// The byte size of an index vector of `slots` entries, or `None` when that is
/// more than any allocation could address.
fn index_bytes(slots: usize) -> Option<usize> {
    slots
        .checked_mul(size_of::<usize>())
        .filter(|bytes| *bytes <= isize::MAX.cast_unsigned())
}

/// Argument shape for `groupby(iterable, key=None)`.
///
/// Clinic and keyword-capable like [`CombinationsArgs`]; `key` is never
/// type-checked, as CPython discovers a non-callable only when the first
/// item is keyed.
#[derive(FromArgs)]
#[from_args(name = "groupby", style = c_named, at_most_total)]
struct GroupbyArgs {
    #[from_args(static_string = "IterableArg")]
    iterable: Value,
    #[from_args(default = Value::None)]
    key: Value,
}

/// `itertools.groupby(iterable, key=None)` — `(key, group)` per run of equal keys.
fn call_groupby(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let GroupbyArgs { iterable, key } = GroupbyArgs::from_args(args, vm)?;
    let (key, source) = resolve_source(key, iterable, vm)?;
    let iter = ItertoolsIter::GroupBy(Box::new(GroupBy::new(source, key)));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// `itertools.chain.from_iterable(iterable)` — the items of each of
/// `iterable`'s items, back to back.
///
/// `METH_O` in CPython, so keywords are rejected wholesale and the arity
/// wording is `takes exactly one argument (N given)`.
fn call_chain_from_iterable(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let mut positional = args.into_pos_only("chain.from_iterable", vm.heap)?;
    let count = positional.len();
    if count != 1 {
        positional.drop_with(vm);
        return Err(ExcType::type_error_arg_count("chain.from_iterable", 1, count));
    }
    let iterable = positional.next().expect("exactly one positional argument");
    // The outer iterable is resolved now — `chain.from_iterable(5)` raises
    // here — while its items are resolved one at a time, as `chain`'s
    // arguments are.
    let outer = iterable.into_py_iter(vm)?;
    let iter = ItertoolsIter::Chain(Chain::from_iterable(outer));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(iter))))
}

/// Argument shape for `tee(iterable, n=2)`.
///
/// `PyArg_UnpackTuple` with a 1..2 arity, so arity errors read `tee expected at
/// most 2 arguments, got 3`. The blanket keyword rejection is the one in this
/// module that names the function with its module (`itertools.tee()`), hence
/// the `kwarg_error_name`. `n` stays a raw `Value` for the `Py_ssize_t`
/// conversion in the body, as `batched`'s does.
#[derive(FromArgs)]
#[from_args(name = "tee", style = unpack, kwarg_error_name = "itertools.tee")]
struct TeeArgs {
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(pos_only, default)]
    n: Option<Value>,
}

/// `itertools.tee(iterable, n=2)` — `n` iterators over one source.
fn call_tee(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let TeeArgs { iterable, n } = TeeArgs::from_args(args, vm)?;
    let consumers = match n {
        None => Ok(2),
        Some(n) => {
            let converted = tee_n(&n, vm);
            n.drop_with(vm);
            converted
        }
    };
    let consumers = match consumers {
        Ok(consumers) => consumers,
        Err(error) => {
            iterable.drop_with(vm);
            return Err(error);
        }
    };

    // No consumers means nothing to read from, and CPython never looks at the
    // iterable in that case: `tee(5, 0)` is the empty tuple, not a `TypeError`.
    if consumers == 0 {
        iterable.drop_with(vm);
        return Ok(allocate_tuple(TupleVec::new(), vm.heap));
    }

    // Resolved first, then tested: CPython calls `iter()` before it reaches for
    // `__copy__`, so an object whose `__iter__` hands back an existing `_tee`
    // is copied too rather than being drained into a second buffer.
    let source = iterable.into_py_iter(vm)?;
    // An existing `_tee` is copied rather than drained, so the copies replay
    // from where it stands — CPython reaches for `__copy__` for the same
    // reason. Anything else becomes the source of a fresh group.
    let tees = if let Some(tees) = tee::fork_group(&source, consumers, vm) {
        source.drop_with(vm);
        tees
    } else {
        tee::new_group(source, consumers, vm)
    };
    Ok(allocate_tuple(TupleVec::from_vec(tees), vm.heap))
}

/// Coerces `tee`'s `n` and enforces CPython's "not negative" floor.
///
/// A group of `n` costs a heap entry and a buffer position per consumer, plus
/// the tuple slot each is returned in — all of it inside one builtin call, so
/// the whole group is charged before any of it is built. CPython reports an
/// `n` too large as a bare `MemoryError` when its own allocation fails.
fn tee_n(value: &Value, vm: &mut VM<'_>) -> RunResult<usize> {
    let n = ssize_arg(value, vm)?;
    let n = usize::try_from(n).map_err(|_| ExcType::tee_negative_n())?;
    let per_consumer = HEAP_ENTRY_SIZE + size_of::<usize>() + VALUE_SIZE;
    let bytes = n
        .checked_mul(per_consumer)
        .filter(|bytes| *bytes <= isize::MAX.cast_unsigned())
        .ok_or_else(ExcType::allocation_too_large)?;
    check_estimated_size(bytes, &vm.heap.tracker)?;
    Ok(n)
}

/// Resolves the iterable while keeping the callable safe from the error path.
///
/// CPython resolves eagerly for all four, so a non-iterable raises here rather
/// than on the first `next()`. The callable itself is never type-checked: a
/// non-callable is only discovered when the adaptor first applies it.
fn resolve_source(callable: Value, iterable: Value, vm: &mut VM<'_>) -> RunResult<(Value, Value)> {
    let mut guard = DropGuard::new(callable, vm);
    let source = iterable.into_py_iter(guard.ctx())?;
    let (callable, _) = guard.into_parts();
    Ok((callable, source))
}
