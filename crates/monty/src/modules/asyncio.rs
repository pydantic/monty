//! Implementation of the `asyncio` module.
//!
//! Provides a minimal implementation of Python's `asyncio` module with:
//! - `run(coro)`: Runs a coroutine to completion, equivalent to `await coro`
//! - `gather(*awaitables)`: Collects coroutines for concurrent execution
//!
//! Other asyncio functions (`create_task`, `sleep`, `wait`, etc.) are not implemented.
//! The host acts as the event loop - Monty yields control when tasks are blocked.

use crate::{
    args::{ArgValues, FromArgs},
    asyncio::GatherFuture,
    bytecode::{CallResult, VM},
    defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::Module,
    value::Value,
};

/// Async Functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum AsyncioFunctions {
    Gather,
    Run,
}

/// Creates the `asyncio` module and allocates it on the heap.
///
/// The module contains only the `gather` function. Other asyncio functions
/// are not implemented as they would require additional VM/scheduler features.
///
/// # Returns
/// A HeapId pointing to the newly allocated module.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Asyncio);

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

    vm.heap.allocate(HeapData::Module(module))
}
pub(super) fn call(
    vm: &mut VM<'_, impl ResourceTracker>,
    functions: AsyncioFunctions,
    args: ArgValues,
) -> RunResult<CallResult> {
    match functions {
        AsyncioFunctions::Gather => gather(vm, args).map(CallResult::Value),
        AsyncioFunctions::Run => run(vm.heap, args),
    }
}

/// Implementation of `asyncio.run(coro)`.
///
/// Runs a single coroutine to completion, equivalent to `await coro` at the top level.
/// Accepts exactly one positional argument (the coroutine) and no keyword arguments.
///
/// Returns `CallResult::AwaitValue` so the VM executes `exec_get_awaitable` on
/// the value, which handles validation that it's actually a coroutine/awaitable.
fn run(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
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
pub(crate) fn gather(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    // TODO: support keyword arguments (e.g. return_exceptions); for now any
    // kwarg is rejected up front by the macro's `kwargs_not_supported_yet`
    // flag with a `NotImplementedError: gather() does not yet support keyword
    // arguments` (CPython would have given a TypeError naming the bad kwarg).
    let GatherArgs { awaitables } = GatherArgs::from_args(args, vm)?;
    defer_drop_mut!(awaitables, vm);

    // Validate all positional args are awaitable and collect their heap ids.
    // Both coroutines and external futures live on the heap; transfer
    // ownership of each arg's HeapId into `items` and forget the `Value` so
    // its `Drop` doesn't dec_ref the entry we just handed to the gather.
    let mut items: Vec<HeapId> = Vec::new();

    #[cfg_attr(not(feature = "memory-model-checks"), expect(unused_mut))]
    for mut arg in awaitables.drain(..) {
        let id = match &arg {
            Value::Ref(id)
                if matches!(
                    vm.heap.get(*id),
                    HeapData::Coroutine(_) | HeapData::ExternalFuture(_) | HeapData::GatherFuture(_)
                ) =>
            {
                Some(*id)
            }
            _ => None,
        };

        if let Some(id) = id {
            items.push(id);
            // Transfer ownership of the heap ref to the gather.
            #[cfg(feature = "memory-model-checks")]
            arg.dec_ref_forget();
        } else {
            arg.drop_with_heap(vm.heap);
            for id in items {
                vm.heap.dec_ref(id);
            }
            return Err(ExcType::type_error(
                "An asyncio.Future, a coroutine or an awaitable is required",
            ));
        }
    }

    let gather_future = GatherFuture::new(items);
    let id = vm.heap.allocate(HeapData::GatherFuture(gather_future))?;
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
