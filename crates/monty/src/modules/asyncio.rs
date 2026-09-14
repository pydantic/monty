//! Implementation of the `asyncio` module.
//!
//! Provides a minimal implementation of Python's `asyncio` module with:
//! - `run(coro)`: Runs a coroutine to completion, equivalent to `await coro`
//! - `gather(*awaitables)`: Collects coroutines for concurrent execution
//! - `sleep(delay, result=None)`: Asks the host to wait, as an awaitable
//!
//! Other asyncio functions (`create_task`, `wait`, etc.) are not implemented.
//! The host acts as the event loop - Monty yields control when tasks are blocked.

use monty_types::{AsyncSleepArgs, MontyObject, OsFunctionCall, sleep_duration_saturating};
use num_traits::ToPrimitive;

use crate::{
    args::{ArgValues, FromArgs},
    asyncio::GatherFuture,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    object_bridge::MontyObjectExt,
    os_dispatch::PostConversionEffect,
    types::Module,
    value::Value,
};

/// Async Functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum AsyncioFunctions {
    Gather,
    Run,
    Sleep,
}

/// Creates the `asyncio` module and allocates it on the heap.
///
/// The module contains only the `run`, `gather` and `sleep` functions. Other asyncio
/// functions are not implemented as they would require additional VM/scheduler features.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Asyncio, vm.interns);

    module.set_attr(
        StaticStrings::Gather,
        Value::ModuleFunction(ModuleFunctions::Asyncio(AsyncioFunctions::Gather)),
        vm,
    );
    module.set_attr(
        StaticStrings::Run,
        Value::ModuleFunction(ModuleFunctions::Asyncio(AsyncioFunctions::Run)),
        vm,
    );
    module.set_attr(
        StaticStrings::Sleep,
        Value::ModuleFunction(ModuleFunctions::Asyncio(AsyncioFunctions::Sleep)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}
pub(super) fn call(vm: &mut VM<'_>, functions: AsyncioFunctions, args: ArgValues) -> RunResult<CallResult> {
    match functions {
        AsyncioFunctions::Gather => gather(vm, args).map(CallResult::Value),
        AsyncioFunctions::Run => run(vm.heap, args),
        AsyncioFunctions::Sleep => sleep(vm, args),
    }
}

/// `asyncio.sleep(delay, result=None)` — an awaitable the host completes.
///
/// Unlike CPython, the call itself suspends to the host rather than returning
/// a coroutine that starts on `await`: the wait is the host's to schedule, and
/// only it knows whether it can run other tasks meanwhile. A host with an
/// event loop should answer with a pending future so sibling tasks keep
/// running; one without can wait inline and answer with `result`, which
/// [`PostConversionEffect::SettleAwaitable`] turns into an already-settled
/// awaitable so the `await` works either way. See `limitations/asyncio.md`.
fn sleep(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let SleepArgs { delay, result } = SleepArgs::from_args(args, vm)?;
    // Converted first so the only value still holding a heap reference past
    // this point is `delay`, which the guard covers.
    let result = MontyObject::export(result, vm);
    defer_drop!(delay, vm);
    let seconds = delay_seconds(delay, vm)?;
    // NaN is the one delay CPython refuses; the rest clamp.
    let delay =
        sleep_duration_saturating(seconds).map_err(|_| ExcType::value_error("Invalid delay: NaN (not a number)"))?;
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::AsyncSleep(AsyncSleepArgs { delay, result }),
        effect: PostConversionEffect::SettleAwaitable.into(),
    })
}

/// `asyncio.sleep(delay, result=None)` — a pure-Python `def` in CPython, so
/// both parameters bind by keyword and neither is type-checked while binding.
#[derive(FromArgs)]
#[from_args(name = "sleep", style = def)]
struct SleepArgs {
    #[from_args(static_string = "Delay")]
    delay: Value,
    #[from_args(static_string = "ResultArg", default = Value::None)]
    result: Value,
}

/// Reads `delay` as float seconds.
///
/// CPython never converts it: it compares `delay <= 0` and hands whatever is
/// left to the loop, so only real numbers work — an `__index__`-able class is
/// rejected here although `time.sleep()` accepts it — and a non-number fails as
/// an unsupported comparison rather than as a bad argument.
fn delay_seconds(delay: &Value, vm: &VM<'_>) -> RunResult<f64> {
    match delay {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Bool(b) => Ok(f64::from(*b)),
        _ => match delay.as_long_int(vm) {
            Some(n) => Ok(n.to_f64().unwrap_or(f64::INFINITY)),
            None => Err(ExcType::type_error_ordering("<=", &delay.py_type_name(vm), "int")),
        },
    }
}

/// Implementation of `asyncio.run(coro)`.
///
/// Runs a single coroutine to completion, equivalent to `await coro` at the top level.
/// Accepts exactly one positional argument (the coroutine) and no keyword arguments.
///
/// Returns `CallResult::AwaitValue` so the VM executes `exec_get_awaitable` on
/// the value, which handles validation that it's actually a coroutine/awaitable.
fn run(heap: &mut Heap, args: ArgValues) -> RunResult<CallResult> {
    let coroutine = args.get_one_arg("asyncio.run", heap)?;
    Ok(CallResult::AwaitValue(coroutine))
}

/// Implementation of `asyncio.gather(*awaitables)`.
///
/// Collects coroutines and external futures for concurrent execution. Does NOT
/// spawn tasks immediately - just validates and stores the references. Tasks are
/// spawned when the returned `GatherFuture` is awaited (in the `Await` opcode handler).
///
/// # Behavior when awaited
///
/// 1. Each coroutine is spawned as a separate Task
/// 2. External futures are tracked for resolution by the host
/// 3. The current task blocks until all items complete
/// 4. Results are collected in order and returned as a list
/// 5. On any task failure, sibling tasks are cancelled and the exception propagates
///
/// # Arguments
/// * `heap` - The heap for allocating the GatherFuture
/// * `args` - Variadic awaitable arguments (coroutines or external futures)
///
/// # Errors
/// Returns `TypeError` if any argument is not awaitable.
pub(crate) fn gather(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    // TODO: support keyword arguments (e.g. return_exceptions); for now any
    // kwarg is rejected up front by the macro's `kwargs_not_supported_yet`
    // flag with a `NotImplementedError: gather() does not yet support keyword
    // arguments` (CPython would have given a TypeError naming the bad kwarg).
    let GatherArgs { awaitables } = GatherArgs::from_args(args, vm)?;
    defer_drop_mut!(awaitables, vm);

    // Validate every argument before transferring any references to the gather,
    // so an invalid later argument needs no raw-HeapId rollback.
    for arg in awaitables.iter() {
        if !matches!(
            arg,
            Value::Ref(id)
                if matches!(
                    vm.heap.get(*id),
                    HeapData::Coroutine(_) | HeapData::ExternalFuture(_) | HeapData::GatherFuture(_)
                )
        ) {
            return Err(ExcType::type_error(
                "An asyncio.Future, a coroutine or an awaitable is required",
            ));
        }
    }

    let items = awaitables
        .drain(..)
        .map(|arg| arg.into_ref_id().expect("validated gather awaitable is heap-backed"))
        .collect();
    let gather_future = GatherFuture::new(items);
    let id = vm.heap.allocate(HeapData::GatherFuture(Box::new(gather_future)));
    Ok(Value::Ref(id))
}

/// `asyncio.gather(*awaitables)` — variadic positional, no kwargs accepted.
///
/// `kwargs_not_supported_yet` rejects any kwarg with the macro's
/// "not yet implemented" `NotImplementedError`, replacing the previous
/// `TypeError: gather() takes no keyword arguments` from `into_pos_only`.
/// When CPython's `return_exceptions=False` is wired up this becomes a
/// regular `kw_only` slot and the flag goes away.
#[derive(FromArgs)]
#[from_args(name = "gather", kwargs_not_supported_yet)]
struct GatherArgs {
    #[from_args(varargs)]
    awaitables: Vec<Value>,
}
