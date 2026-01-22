//! Async/await support types for Monty.
//!
//! This module contains all async-related types including coroutines, futures,
//! and task identifiers. The host acts as the event loop - external function
//! calls return `ExternalFuture` objects that can be awaited.

use crate::{heap::HeapId, intern::FunctionId, value::Value};

/// Unique identifier for external function calls.
///
/// Sequential integers allocated by the scheduler. Used to correlate
/// external function calls with their results when the host resolves them.
/// The counter always increments, even for sync resolution, to keep IDs unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct CallId(pub u32);

impl CallId {
    /// Creates a new CallId from a raw value.
    #[inline]
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw u32 value.
    #[inline]
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Unique identifier for an async task.
///
/// Sequential integers allocated by the scheduler. Task 0 is always the main task
/// which uses the VM's stack/frames directly. Spawned tasks (1+) store their own context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskId(pub u32);

#[expect(dead_code, reason = "used in later phases")]
impl TaskId {
    /// Creates a new TaskId from a raw value.
    #[inline]
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw u32 value.
    #[inline]
    pub fn raw(self) -> u32 {
        self.0
    }

    /// Returns true if this is the main task (task 0).
    #[inline]
    pub fn is_main(self) -> bool {
        self.0 == 0
    }
}

/// Coroutine execution state (single-shot semantics).
///
/// Coroutines in Monty follow single-shot semantics - they can only be awaited once.
/// This differs from Python generators which can be resumed multiple times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum CoroutineState {
    /// Coroutine has been created but not yet awaited.
    New,
    /// Coroutine is currently executing (has been awaited).
    Running,
    /// Coroutine has finished execution.
    Completed,
}

/// A coroutine object representing an async function call result.
///
/// Created when an `async def` function is called. Argument binding happens at call time;
/// awaiting the coroutine starts execution. Coroutines use single-shot semantics -
/// they can only be awaited once.
///
/// # Namespace Layout
///
/// The `namespace` vector is pre-sized to match the function's namespace size and contains:
/// ```text
/// [params...][cell_vars...][free_vars...][locals...]
/// ```
/// - Parameter slots are filled with bound argument values at call time
/// - Cell/free var slots contain `Value::Ref` to captured cells
/// - Local slots start as `Value::Undefined`
///
/// When the coroutine is awaited, this namespace is registered with the VM's `Namespaces`
/// and a new frame is pushed to execute the async function body.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Coroutine {
    /// The async function to execute.
    pub func_id: FunctionId,
    /// Pre-bound namespace values (sized to function namespace).
    /// Contains bound parameters, captured cells, and uninitialized locals.
    pub namespace: Vec<Value>,
    /// HeapIds of captured cells from enclosing scopes.
    /// These are passed to the frame when execution starts.
    pub frame_cells: Vec<HeapId>,
    /// Current execution state.
    pub state: CoroutineState,
}

#[expect(dead_code, reason = "used in later phases")]
impl Coroutine {
    /// Creates a new coroutine for an async function call.
    ///
    /// # Arguments
    /// * `func_id` - The async function to execute
    /// * `namespace` - Pre-bound namespace with parameters and captured variables
    /// * `frame_cells` - HeapIds of captured cells from enclosing scopes
    pub fn new(func_id: FunctionId, namespace: Vec<Value>, frame_cells: Vec<HeapId>) -> Self {
        Self {
            func_id,
            namespace,
            frame_cells,
            state: CoroutineState::New,
        }
    }

    /// Returns true if this coroutine can be awaited (state is New).
    #[inline]
    pub fn can_await(&self) -> bool {
        self.state == CoroutineState::New
    }
}

/// A gather() result tracking multiple coroutines/tasks.
///
/// Created by `asyncio.gather(*awaitables)`. Does NOT spawn tasks immediately -
/// tasks are spawned when the GatherFuture is awaited in GetAwaitable.
///
/// # Lifecycle
///
/// 1. **Creation**: `gather(coro1, coro2, ...)` stores coroutine HeapIds
/// 2. **Await**: `await gather_future` spawns tasks and blocks the current task
/// 3. **Completion**: As tasks complete, results are stored in order
/// 4. **Return**: When all tasks complete, returns list of results
///
/// # Error Handling
///
/// On any task failure, sibling tasks are cancelled and the exception propagates
/// to the task that awaited the gather.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct GatherFuture {
    /// Coroutine HeapIds to spawn (set at creation).
    pub coroutine_ids: Vec<HeapId>,
    /// TaskIds of spawned tasks (set when awaited).
    /// Indices align with coroutine_ids/results.
    pub task_ids: Vec<TaskId>,
    /// Results from each task, in order (filled as tasks complete).
    pub results: Vec<Option<Value>>,
    /// Task waiting on this gather (set when awaited).
    pub waiter: Option<TaskId>,
}

impl GatherFuture {
    /// Creates a new GatherFuture with the given coroutines.
    ///
    /// # Arguments
    /// * `coroutine_ids` - HeapIds of coroutines to run concurrently
    pub fn new(coroutine_ids: Vec<HeapId>) -> Self {
        let count = coroutine_ids.len();
        Self {
            coroutine_ids,
            task_ids: Vec::with_capacity(count),
            results: (0..count).map(|_| None).collect(),
            waiter: None,
        }
    }

    /// Returns true if all tasks have completed.
    #[inline]
    #[expect(dead_code, reason = "used in later phases")]
    pub fn is_complete(&self) -> bool {
        self.results.iter().all(Option::is_some)
    }

    /// Returns the number of tasks.
    #[inline]
    pub fn task_count(&self) -> usize {
        self.coroutine_ids.len()
    }
}
