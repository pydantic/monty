//! Implementation of Python's `functools` module.
//!
//! A subset so far, currently `reduce` and `partial`. See
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
    types::{Module, Type},
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

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `functools` module function.
pub(super) fn call(vm: &mut VM<'_>, function: FunctoolsFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        FunctoolsFunctions::Reduce => call_reduce(vm, args),
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
