//! Implementation of Python's `functools` module.
//!
//! A subset so far: `reduce`, `partial`, `lru_cache`, and `cache`. See
//! `limitations/functools.md` for what diverges from CPython. Unimplemented
//! names are absent from the namespace rather than stubbed, so they raise
//! `AttributeError` up front.

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::FrozenFunction,
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
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Functools, vm.interns);

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
pub(super) fn call(vm: &mut VM<'_>, function: FunctoolsFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        FunctoolsFunctions::Reduce => call_reduce(vm, args),
        FunctoolsFunctions::Cache => call_cache(vm, args).map(CallResult::Value),
        FunctoolsFunctions::LruCache => call_lru_cache_factory(vm, args).map(CallResult::Value),
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
/// Native setup preserves CPython's argument and empty-iterator errors, then a
/// frozen Python frame owns the suspendable callback loop.
fn call_reduce(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ReduceArgs {
        function,
        iterable,
        initial,
    } = ReduceArgs::from_args(args, vm)?;

    // Guard values not consumed by iterator conversion, then transfer them
    // into one setup state once conversion succeeds.
    let mut guard = DropGuard::new((function, initial), vm);
    let iterator = iterable.into_py_iter(guard.ctx()).map_err(not_iterable_error)?;
    let (function, initial) = guard.into_inner();
    let mut guard = DropGuard::new((function, (iterator, initial)), vm);
    let ((_, (iterator, initial)), vm) = guard.as_parts_mut();

    let accumulator = if let Some(initial) = initial.take() {
        initial
    } else {
        // Without `initial` the first item seeds the fold, so a one-item
        // iterable returns that item without ever calling `function`.
        let mut iterator_read = iterator.read(vm);
        match iterator_read.py_next(vm)? {
            Some(first) => first,
            None => return Err(ExcType::reduce_empty_iterable()),
        }
    };

    let ((function, (iterator, initial)), vm) = guard.into_parts();
    debug_assert!(initial.is_none());
    vm.call_frozen_exact(
        FrozenFunction::FunctoolsReduce,
        [
            function,
            iterator,
            accumulator,
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Next)),
            Value::Builtin(Builtins::ExcType(ExcType::StopIteration)),
        ],
    )
}

/// Arguments to the unbounded caching decorator.
#[derive(FromArgs)]
#[from_args(name = "cache", style = def)]
struct CacheArgs {
    #[from_args(pos_only, static_string = "UserFunction")]
    user_function: Value,
}

/// Wraps a callable in an unbounded cache.
fn call_cache(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CacheArgs { user_function } = CacheArgs::from_args(args, vm)?;
    lru_cache::allocate(Some(user_function), None, false, vm)
}

/// Arguments to the configurable caching decorator; the first may itself be callable.
#[derive(FromArgs)]
#[from_args(name = "lru_cache", style = def)]
struct LruCacheArgs {
    #[from_args(default = Value::Int(128))]
    maxsize: Value,
    #[from_args(default = Value::Bool(false))]
    typed: Value,
}

/// Returns a configured decorator, or wraps a callable passed as the first argument.
fn call_lru_cache_factory(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let LruCacheArgs { maxsize, typed } = LruCacheArgs::from_args(args, vm)?;
    defer_drop!(maxsize, vm);
    defer_drop!(typed, vm);
    let typed_flag = typed.py_bool(vm)?;
    match maxsize {
        Value::Int(size) => {
            let size = u64::try_from(*size).unwrap_or(0);
            lru_cache::allocate(None, Some(size), typed_flag, vm)
        }
        Value::Bool(flag) => lru_cache::allocate(None, Some(u64::from(*flag)), typed_flag, vm),
        Value::None => lru_cache::allocate(None, None, typed_flag, vm),
        _ if maxsize.is_callable(vm.heap) => {
            lru_cache::allocate(Some(maxsize.clone_with_heap(vm)), Some(128), typed_flag, vm)
        }
        _ => Err(lru_cache::bad_maxsize_error()),
    }
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
