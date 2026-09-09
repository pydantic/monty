//! Implementation of Python's `functools` module.
//!
//! A subset so far: `reduce`, `partial`, `lru_cache` and `cache`. See
//! `limitations/functools.md` for what diverges from CPython. Unimplemented
//! names are absent from the namespace rather than stubbed, so they raise
//! `AttributeError` up front.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{Module, PyTrait, Type, lru_cache},
    value::Value,
};

/// `functools` module functions, each a Python-visible callable.
///
/// `partial` is absent because it is exposed as a type object rather than a
/// function, so `type(p) is functools.partial` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum FunctoolsFunctions {
    Reduce,
    Cache,
    LruCache,
}

/// Creates the `functools` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Functools);

    module.set_attr(
        StaticStrings::Reduce,
        Value::ModuleFunction(ModuleFunctions::Functools(FunctoolsFunctions::Reduce)),
        vm,
    );
    module.set_attr(
        StaticStrings::Partial,
        Value::Builtin(Builtins::Type(Type::Partial)),
        vm,
    );
    module.set_attr(
        StaticStrings::Cache,
        Value::ModuleFunction(ModuleFunctions::Functools(FunctoolsFunctions::Cache)),
        vm,
    );
    module.set_attr(
        StaticStrings::LruCache,
        Value::ModuleFunction(ModuleFunctions::Functools(FunctoolsFunctions::LruCache)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `functools` module function.
pub(super) fn call(vm: &mut VM<'_>, function: FunctoolsFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        FunctoolsFunctions::Reduce => call_reduce(vm, args),
        FunctoolsFunctions::Cache => call_cache(vm, args),
        FunctoolsFunctions::LruCache => call_lru_cache_factory(vm, args),
    }
}

/// Argument shape for `reduce(function, iterable, /[, initial])`.
///
/// CPython's Argument Clinic signature makes the first two positional-only
/// while `initial` is also accepted by keyword, and counts positionals plus
/// keywords together for the maximum (`at_most_total`): `reduce(f, x, 0,
/// initial=1)` reports four arguments.
#[derive(FromArgs)]
#[from_args(name = "reduce", at_most_total)]
struct ReduceArgs {
    #[from_args(pos_only)]
    function: Value,
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(default)]
    initial: Option<Value>,
}

/// `functools.reduce(function, iterable, /[, initial])` — fold `function` over
/// `iterable` from the left.
///
/// `function` is called through [`VM::evaluate_function`], which cannot suspend
/// to the host, so an external function or `os` callback used here raises
/// `NotImplementedError` as it does under `map()`.
fn call_reduce(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ReduceArgs {
        function,
        iterable,
        initial,
    } = ReduceArgs::from_args(args, vm)?;
    defer_drop!(function, vm);

    // `into_py_iter` consumes `iterable`, so `initial` is guarded across the
    // conversion and reclaimed once it succeeds.
    let mut guard = DropGuard::new(initial, vm);
    let iter = iterable.into_py_iter(guard.ctx()).map_err(not_iterable_error)?;
    let (initial, vm) = guard.into_parts();
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    let accumulator = match initial {
        Some(initial) => initial,
        // Without `initial` the first item seeds the fold, so a one-item
        // iterable returns that item without ever calling `function`.
        None => match iter.py_next(vm)? {
            Some(first) => first,
            None => return Err(ExcType::reduce_empty_iterable()),
        },
    };

    let mut acc_guard = DropGuard::new(accumulator, vm);
    {
        let (accumulator, vm) = acc_guard.as_parts_mut();
        while let Some(item) = iter.py_next(vm)? {
            // Move the accumulator into the call rather than cloning it: on
            // error `evaluate_function` releases the arguments, and the
            // placeholder left behind needs no cleanup.
            let previous = mem::replace(accumulator, Value::None);
            *accumulator = vm.evaluate_function("reduce()", function, ArgValues::Two(previous, item))?;
        }
    }
    Ok(acc_guard.into_inner())
}

/// Rewrites the `TypeError` from a second argument that is not iterable.
///
/// CPython only replaces a `TypeError` here, so an `__iter__` that raises
/// anything else propagates unchanged.
#[cold]
fn not_iterable_error(error: RunError) -> RunError {
    match &error {
        RunError::Exc(raise) if raise.exc.exc_type() == ExcType::TypeError => ExcType::reduce_not_iterable(),
        _ => error,
    }
}

/// Argument shape for `cache(user_function, /)`.
#[derive(FromArgs)]
#[from_args(name = "cache", style = def)]
struct CacheArgs {
    #[from_args(pos_only)]
    user_function: Value,
}

/// `functools.cache(user_function)` — an unbounded [`LruCache`], the shorthand
/// CPython defines as `lru_cache(maxsize=None)`.
fn call_cache(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CacheArgs { user_function } = CacheArgs::from_args(args, vm)?;
    lru_cache::allocate(Some(user_function), None, false, vm)
}

/// Argument shape for `lru_cache(maxsize=128, typed=False)`.
///
/// Both are plain `Value`s: `maxsize` doubles as the decorated function in the
/// bare `@lru_cache` form, and CPython only tests `typed` for truthiness.
#[derive(FromArgs)]
#[from_args(name = "lru_cache", style = def)]
struct LruCacheArgs {
    #[from_args(default = Value::Int(i64::from(DEFAULT_MAXSIZE)))]
    maxsize: Value,
    #[from_args(default = Value::Bool(false))]
    typed: Value,
}

/// CPython's default entry ceiling for `lru_cache`.
const DEFAULT_MAXSIZE: u32 = 128;

/// `functools.lru_cache(maxsize=128, typed=False)`.
///
/// Returns the cached function directly when used bare (`@lru_cache`, where
/// `maxsize` *is* the decorated function), and otherwise a decorator holding
/// the settings until the function to cache arrives.
fn call_lru_cache_factory(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let LruCacheArgs { maxsize, typed } = LruCacheArgs::from_args(args, vm)?;
    defer_drop!(maxsize, vm);
    defer_drop!(typed, vm);
    let typed_flag = typed.py_bool(vm)?;

    // CPython tests for an int first, so `lru_cache(True)` is a size, not a
    // (callable-less) mistake.
    match maxsize {
        Value::Int(_) | Value::Bool(_) => decorator(Some(maxsize_from_int(maxsize)), typed_flag, vm),
        // The bare `@lru_cache` form: `maxsize` is the function to wrap.
        _ if maxsize.is_callable(vm.heap) => {
            let func = maxsize.clone_with_heap(vm);
            lru_cache::allocate(Some(func), Some(DEFAULT_MAXSIZE), typed_flag, vm)
        }
        Value::None => decorator(None, typed_flag, vm),
        _ => Err(ExcType::lru_cache_bad_maxsize()),
    }
}

/// Builds the decorator a parameterized `lru_cache(...)` hands back: a
/// wrapper holding the settings until the function to cache arrives.
fn decorator(maxsize: Option<u32>, typed: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    lru_cache::allocate(None, maxsize, typed, vm)
}

/// Clamps an integer `maxsize` to what the cache stores: negatives cache
/// nothing, as CPython's `if maxsize < 0: maxsize = 0` does.
fn maxsize_from_int(maxsize: &Value) -> u32 {
    let size = match maxsize {
        Value::Int(size) => *size,
        Value::Bool(flag) => i64::from(*flag),
        _ => unreachable!("only called for an int-like maxsize"),
    };
    u32::try_from(size).unwrap_or(if size < 0 { 0 } else { u32::MAX })
}
