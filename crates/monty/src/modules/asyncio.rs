//! Implementation of the `asyncio` module.
//!
//! Provides a minimal implementation of Python's `asyncio` module with:
//! - `run(coro)`: Runs a coroutine to completion, equivalent to `await coro`
//! - `gather(*awaitables)`: Collects coroutines for concurrent execution
//!
//! Other asyncio functions (`create_task`, `sleep`, `wait`, etc.) are not implemented.
//! The host acts as the event loop - Monty yields control when tasks are blocked.

use crate::{
    args::ArgValues,
    asyncio::{GatherFuture, GatherItem},
    defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Module},
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
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Asyncio);

    module.set_attr(
        StaticStrings::Gather,
        Value::ModuleFunction(ModuleFunctions::Asyncio(AsyncioFunctions::Gather)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Run,
        Value::ModuleFunction(ModuleFunctions::Asyncio(AsyncioFunctions::Run)),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}
pub(super) fn call(
    heap: &mut Heap<impl ResourceTracker>,
    functions: AsyncioFunctions,
    args: ArgValues,
) -> RunResult<AttrCallResult> {
    match functions {
        AsyncioFunctions::Gather => gather(heap, args).map(AttrCallResult::Value),
        AsyncioFunctions::Run => run(heap, args),
    }
}

/// Implementation of `asyncio.run(coro)`.
///
/// Runs a single coroutine to completion, equivalent to `await coro` at the top level.
/// Accepts exactly one positional argument (the coroutine) and no keyword arguments.
///
/// Returns `AttrCallResult::AwaitValue` so the VM executes `exec_get_awaitable` on
/// the value, which handles validation that it's actually a coroutine/awaitable.
fn run(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<AttrCallResult> {
    let coroutine = args.get_one_arg("asyncio.run", heap)?;
    Ok(AttrCallResult::AwaitValue(coroutine))
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
pub(crate) fn gather(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (pos_args, kwargs) = args.into_parts();
    defer_drop_mut!(pos_args, heap);

    // TODO: support keyword arguments (e.g. return_exceptions)
    kwargs.not_supported_yet("gather", heap)?;

    // Validate all positional args are awaitable and collect them
    let mut items = Vec::new();
    let mut coroutine_ids_to_cleanup: Vec<HeapId> = Vec::new();

    #[cfg_attr(not(feature = "ref-count-panic"), expect(unused_mut))]
    for mut arg in pos_args {
        match &arg {
            Value::Ref(id) if heap.get(*id).is_coroutine() => {
                coroutine_ids_to_cleanup.push(*id);
                items.push(GatherItem::Coroutine(*id));
                // Transfer ownership to GatherFuture - mark Value as consumed without dec_ref
                #[cfg(feature = "ref-count-panic")]
                arg.dec_ref_forget();
            }
            Value::ExternalFuture(call_id) => {
                items.push(GatherItem::ExternalFuture(*call_id));
                // ExternalFuture is Copy, no refcount to manage
            }
            _ => {
                // Not awaitable - clean up and error
                arg.drop_with_heap(heap);
                // Drop already-collected coroutine refs
                for cid in coroutine_ids_to_cleanup {
                    heap.dec_ref(cid);
                }
                return Err(ExcType::type_error(
                    "An asyncio.Future, a coroutine or an awaitable is required",
                ));
            }
        }
    }

    // Create GatherFuture on heap
    let gather_future = GatherFuture::new(items);
    let id = heap.allocate(HeapData::GatherFuture(gather_future))?;
    Ok(Value::Ref(id))
}
