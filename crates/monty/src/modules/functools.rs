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
    exception_private::{ExcType, RunResult},
    frozen::FrozenFunction,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{BuiltinType, Module, PyTrait, lru_cache},
    value::Value,
};

/// Acquires and folds an iterable, allowing both its protocol and the reducer to suspend.
pub(crate) const REDUCE_SOURCE: &str = r"
def __monty_functools_reduce(function, iterable, accumulator, has_initial, iter_, next_, stop_iteration, type_error):
    try:
        iterator = iter_(iterable)
    except type_error:
        raise type_error('reduce() arg 2 must support iteration') from None
    if not has_initial:
        try:
            accumulator = next_(iterator)
        except stop_iteration:
            raise type_error('reduce() of empty iterable with no initial value') from None
    while True:
        try:
            item = next_(iterator)
        except stop_iteration:
            return accumulator
        accumulator = function(accumulator, item)
";

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
        Value::Builtin(Builtins::Type(BuiltinType::Partial)),
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
/// Native setup binds the arguments; a frozen Python frame owns acquisition,
/// initial accumulator selection, and the callback loop.
fn call_reduce(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ReduceArgs {
        function,
        iterable,
        initial,
    } = ReduceArgs::from_args(args, vm)?;

    let has_initial = initial.is_some();
    vm.call_frozen_exact(
        FrozenFunction::FunctoolsReduce,
        [
            function,
            iterable,
            initial.unwrap_or(Value::None),
            Value::Bool(has_initial),
            Value::Builtin(Builtins::Type(BuiltinType::Iterator)),
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Next)),
            Value::Builtin(Builtins::ExcType(ExcType::StopIteration)),
            Value::Builtin(Builtins::ExcType(ExcType::TypeError)),
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
        _ if maxsize.is_callable(vm) => {
            lru_cache::allocate(Some(maxsize.clone_with_heap(vm)), Some(128), typed_flag, vm)
        }
        _ => Err(lru_cache::bad_maxsize_error()),
    }
}
