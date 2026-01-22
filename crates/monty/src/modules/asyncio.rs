//! Implementation of the `asyncio` module.
//!
//! Provides a minimal implementation of Python's `asyncio` module with only:
//! - `gather(*awaitables)`: Collects coroutines for concurrent execution
//!
//! Other asyncio functions (`create_task`, `sleep`, `wait`, etc.) are not implemented.
//! The host acts as the event loop - Monty yields control when tasks are blocked.

use crate::{
    args::ArgValues,
    asyncio::GatherFuture,
    builtins::BuiltinsFunctions,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::Module,
    value::Value,
};

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

    // asyncio.gather - the only function we implement
    module.set_attr(
        StaticStrings::Gather,
        Value::Builtin(crate::builtins::Builtins::Function(BuiltinsFunctions::AsyncioGather)),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}

/// Implementation of `asyncio.gather(*awaitables)`.
///
/// Collects coroutines for concurrent execution. Does NOT spawn tasks immediately -
/// just validates and stores the coroutine references. Tasks are spawned when the
/// returned `GatherFuture` is awaited (in the `GetAwaitable` opcode handler).
///
/// # Behavior when awaited
///
/// 1. Each coroutine is spawned as a separate Task
/// 2. The current task blocks until all spawned tasks complete
/// 3. Results are collected in order and returned as a list
/// 4. On any task failure, sibling tasks are cancelled and the exception propagates
///
/// # Arguments
/// * `heap` - The heap for allocating the GatherFuture
/// * `args` - Variadic coroutine arguments
///
/// # Errors
/// Returns `TypeError` if any argument is not a coroutine.
pub(crate) fn gather(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (pos_args, kwargs) = args.into_parts();

    // gather() doesn't accept keyword arguments
    if !kwargs.is_empty() {
        kwargs.drop_with_heap(heap);
        for arg in pos_args {
            arg.drop_with_heap(heap);
        }
        return Err(ExcType::type_error("gather() takes no keyword arguments"));
    }

    // Validate all positional args are coroutines and collect their HeapIds
    let mut coroutine_ids = Vec::new();
    for arg in pos_args {
        match &arg {
            Value::Ref(id) if heap.get(*id).is_coroutine() => {
                coroutine_ids.push(*id);
                // Don't drop - the GatherFuture will own these references
            }
            _ => {
                // Not a coroutine - clean up and error
                arg.drop_with_heap(heap);
                // Drop already-collected coroutine refs
                for cid in coroutine_ids {
                    heap.dec_ref(cid);
                }
                return Err(ExcType::type_error(
                    "gather() argument must be a coroutine or awaitable",
                ));
            }
        }
    }

    // Create GatherFuture on heap
    let gather_future = GatherFuture::new(coroutine_ids);
    let id = heap.allocate(HeapData::GatherFuture(gather_future))?;
    Ok(Value::Ref(id))
}
