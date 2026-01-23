# Async/Await Implementation Plan for Monty

## Overview

Add async/await support to Monty where the host acts as the event loop. External function calls return `ExternalFuture` objects that can be awaited. When all tasks are blocked on futures, control returns to the host with pending future IDs.

`await` statements in the module scope (and generally outside of async functions) are allowed and should "just work". This is a deliberate deviation from standard Python (which raises `SyntaxError`) to match Jupyter notebook behavior where top-level await is permitted.

## Key Design Decisions

1. **Host is event loop** - Monty yields pending calls, host executes and resumes
2. **Unified execution model** - No separate "async mode" vs "sync mode". External calls always return to host with `FunctionCall`. Host chooses how to handle:
   - **Sync pattern**: Call `snapshot.run(result)` with the actual value (current behavior)
   - **Async pattern**: Call `snapshot.run_pending()` to push an `ExternalFuture`, resolve later
3. **Scheduler created eagerly** - Scheduler is always present (created at VM initialization) to maintain separation of concerns. All async state (call_id counter, pending calls, resolved futures) lives in the Scheduler.
4. **All tasks blocked** - Yield to host only when every task is waiting on external call
5. **Cancel all on exception** - Exception propagates, cancels sibling tasks in a gather
6. **Simplified coroutines** - Async functions must be awaited, no `.send()`/`.throw()`
7. **Arg binding at call time** - `async def` validates arguments on call and errors immediately (CPython behavior)
8. **Sequential integer call IDs** - Simple incrementing counter
9. **Ignore other crates for now** - Skip `crates/monty-python/` and `/crates/monty-type-checking` for now

## Execution Flow

Every external function call returns to the host immediately with a `call_id` for tracking. The host decides how to resume:

**Sync resolution** (current behavior):
```
1. ext_func(args)    -> Returns FunctionCall{name, args, call_id, state} to host
2. Host executes     -> Calls state.run(result) with the actual value
3. Code continues    -> Result pushed to stack, execution continues
```

**Async resolution** (new capability):
```
1. ext_func(args)    -> Returns FunctionCall{name, args, call_id, state} to host
2. Host defers       -> Calls state.run_pending() to resume immediately
3. Code continues    -> Receives ExternalFuture(call_id), may hit more ext calls (goto 1)
4. await future      -> If not resolved, task blocks
5. All tasks blocked -> Returns ResolveFutures{pending: [...], state: FutureSnapshot}
6. Host provides     -> Calls state.resume([(id, result), ...])
                        (can be partial - not all pending calls required)
7. Tasks unblock     -> Continue execution, may return to step 1 or 5
8. All done          -> Returns Complete(result)
```

**Key Points**:
- External calls ALWAYS return to host with `FunctionCall` - host chooses sync or async resolution
- `FunctionCall` now includes `call_id` so host can correlate calls with futures
- `Snapshot::run(result)` pushes the result directly (sync pattern)
- `Snapshot::run_pending()` pushes `ExternalFuture(call_id)` (async pattern)
- `FutureSnapshot::resume()` accepts partial results for incremental resolution and may still yield `FunctionCall`
- Results for failed/cancelled tasks are silently ignored

## Implementation Phases

### Phase 1: Core Types

**File: `crates/monty/src/asyncio.rs` (new)**

All async-related types in one file:
```rust
/// Unique identifier for external function calls (sequential integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId(pub u32);

/// A coroutine object (async function call result).
///
/// Argument binding happens at call time; awaiting starts execution.
pub struct Coroutine {
    pub func_id: FunctionId,
    /// Pre-bound namespace values (sized to function namespace).
    pub namespace: Vec<Value>,
    pub frame_cells: Vec<HeapId>,
    pub state: CoroutineState,
}

/// Coroutine execution state (single-shot semantics).
pub enum CoroutineState {
    New,
    Running,
    Completed,
}

/// A gather() result tracking multiple coroutines/tasks.
pub struct GatherFuture {
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
```
Add comprehensive docstrings for all new structs/enums/functions per repo guidelines.

**File: `crates/monty/src/value.rs`**
- Add `ExternalFuture(CallId)` variant to `Value` enum
- Note: Awaiting an already-awaited `ExternalFuture` raises `RuntimeError: cannot reuse already awaited coroutine` (same as coroutines). Track "awaited" state in scheduler's resolved map.

**File: `crates/monty/src/heap.rs`**
- Add `Coroutine(Coroutine)` variant to `HeapData` enum
- Add `GatherFuture(GatherFuture)` variant to `HeapData` enum

### Phase 2: Function Metadata

**File: `crates/monty/src/function.rs`**
- Add `is_async: bool` field to `Function` struct

**File: `crates/monty/src/parse.rs`**
- Parse `async def` (set `is_async` flag)
- Parse `await expr` expressions
- Allow `await` outside async functions
- Reject `yield` inside `async def` (if unsupported) to match Python parser behavior

**File: `crates/monty/src/expressions.rs`**
- Add `Await(Box<ExprLoc>)` variant to `Expr` enum

### Phase 3: Compilation

**File: `crates/monty/src/bytecode/op.rs`**
Add one new opcode:
```rust
/// Await the TOS value. Handles ExternalFuture, Coroutine, and GatherFuture.
GetAwaitable,
```

**File: `crates/monty/src/bytecode/compiler.rs`**

Compile `await expr`:
1. Compile expression (pushes awaitable onto stack)
2. Emit `GetAwaitable`

Compile async function **call**:
- No special opcode at compile time (calls are dynamic)
- VM call path checks `Function.is_async` and returns a `Coroutine` instead of pushing a frame

Compile async function **definition**:
- Same as regular function, but set `is_async: true` in `Function` struct

**File: `crates/monty/src/bytecode/vm/call.rs`**
- When calling a defined function/closure with `is_async: true`, create a `Coroutine` instead of pushing a frame:
  - Bind arguments immediately (reuse existing binding logic so errors are raised on call)
  - Build a pre-filled namespace `Vec<Value>` (do not register in `Namespaces` yet)
  - Capture `frame_cells`
  - Store namespace values + `frame_cells` in `Coroutine` with `state = New`

### Phase 4: Task Scheduler

**File: `crates/monty/src/bytecode/vm/scheduler.rs` (new)**
```rust
/// Unique identifier for a task (sequential integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u32);

/// Task state for async execution.
pub enum TaskState {
    Ready,
    /// Blocked waiting for an external call to resolve.
    BlockedOnCall(CallId),
    /// Blocked waiting for a GatherFuture to complete.
    BlockedOnGather(HeapId),
    Completed(Value),
    Failed(RunError),
}

/// A single async task with its own execution context.
pub struct Task {
    pub id: TaskId,
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub exception_stack: Vec<Value>,
    /// VM-level instruction_ip (for exception table lookup).
    /// Separate from CallFrame.ip which tracks position within each frame.
    pub instruction_ip: usize,
    /// Coroutine being executed by this task (if any).
    pub coroutine_id: Option<HeapId>,
    pub state: TaskState,
}

/// Scheduler for managing concurrent async tasks and external call tracking.
///
/// Always present (created at VM initialization) to maintain separation of concerns.
/// All async-related state lives here: call IDs, pending calls, resolved futures, tasks.
pub struct Scheduler {
    /// All tasks (main task at index 0, spawned tasks follow).
    tasks: Vec<Task>,
    ready_queue: VecDeque<TaskId>,
    /// Currently executing task (None only during task switching).
    current_task: Option<TaskId>,
    next_task_id: u32,
    /// Counter for external call IDs (always incremented, even for sync resolution).
    next_call_id: u32,
    /// Maps CallId -> pending call data (name + args) for unresolved calls.
    /// Populated when host calls `run_pending()`.
    pending_calls: HashMap<CallId, PendingCallData>,
    /// Maps CallId -> resolved Value. Entry removed when awaited.
    resolved: HashMap<CallId, Value>,
    /// CallIds that have been awaited (to detect double-await).
    consumed: HashSet<CallId>,
}

impl Scheduler {
    /// Create a new scheduler with an empty main task (task 0).
    /// The main task's frames/stack are the VM's frames/stack (not copied into Task).
    pub fn new() -> Self;

    /// Spawn a new task from a coroutine HeapId. Returns the TaskId.
    /// Must set coroutine state to Running, store coroutine_id on the task,
    /// and error if already Running/Completed.
    pub fn spawn(&mut self, coroutine_id: HeapId, heap: &mut Heap) -> Result<TaskId, RunError>;

    /// Allocate a new CallId for an external function call.
    pub fn allocate_call_id(&mut self) -> CallId;

    /// Mark the current task as BlockedOnCall(call_id).
    pub fn block_current_on_call(&mut self, call_id: CallId);

    /// Mark the current task as BlockedOnGather(gather_id).
    pub fn block_current_on_gather(&mut self, gather_id: HeapId);

    /// Resolve a CallId with a value. Unblocks any task waiting on it.
    /// Should remove pending call data and drop its ArgValues.
    pub fn resolve(&mut self, call_id: CallId, value: Value);

    /// Get resolved value for a CallId, if available (and consume it).
    pub fn take_resolved(&mut self, call_id: CallId) -> Option<Value>;

    /// Switch to the next ready task, swapping the VM's stacks/frames.
    /// Returns false if no ready tasks.
    pub fn switch_to_next(&mut self, vm: &mut VM) -> bool;

    /// Get all pending (unresolved) CallIds.
    pub fn pending_call_ids(&self) -> Vec<CallId>;

    /// Mark a task as complete and wake any awaiters/gathers.
    pub fn complete_task(&mut self, task_id: TaskId, result: Value) -> Result<(), RunError>;
}

/// Internal representation of a pending external call (Value-level args).
pub struct PendingCallData {
    pub ext_function_id: ExtFunctionId,
    pub args: ArgValues,
    /// Task that created this call (for ignoring results if task is cancelled).
    pub creator_task: TaskId,
}
```
Implementation note: task switching should swap the current VM stack/frames/exception stack/IP
into the `Task` struct and load the next task's context into the VM before continuing the run loop.

### Phase 5: VM Changes

**File: `crates/monty/src/bytecode/vm/mod.rs`**

Add to `FrameExit`:
```rust
/// All async tasks blocked waiting for external call results.
ResolveFutures(Vec<CallId>),

/// External call now includes call_id for async correlation.
ExternalCall {
    ext_function_id: ExtFunctionId,
    args: ArgValues,
    call_id: CallId,
},
```

Add to VM:
```rust
/// Scheduler for async execution and external call tracking.
/// Always present - created at VM initialization.
scheduler: Scheduler,
```
The scheduler is created in `VM::new()`. The "main task" (task 0) uses the VM's stack/frames
directly rather than copying them into the Task struct - only spawned tasks store their own context.

Add an optional `coroutine_id: Option<HeapId>` to `CallFrame` so that when a coroutine
frame returns, the coroutine can be marked `Completed` (and re-await can error).
Update `VMSnapshot` and VM snapshot/restore to include scheduler state
so async execution can be paused/resumed across host calls.
Include a serializable `SchedulerSnapshot` (tasks, ready queue, current task id,
next ids, pending calls, resolved futures, and task-local IP).

**GetAwaitable opcode handling (pseudo-code):**
```rust
Opcode::GetAwaitable => {
    let awaitable = self.pop();

    match awaitable {
        Value::ExternalFuture(call_id) => {
            // Check for double-await (same error as coroutines)
            if self.scheduler.is_consumed(call_id) {
                return Err(RuntimeError("cannot reuse already awaited coroutine"));
            }
            self.scheduler.mark_consumed(call_id);

            if let Some(result) = self.scheduler.take_resolved(call_id) {
                self.push(result);
            } else {
                self.scheduler.block_current_on_call(call_id);
                return self.switch_or_yield(); // swaps task context or yields ResolveFutures
            }
        }
        Value::Ref(id) => match self.heap.get(id) {
            HeapData::Coroutine(coro) => {
                if coro.state != CoroutineState::New {
                    return Err(RuntimeError("cannot reuse already awaited coroutine"));
                }
                // allocate namespace, move coro.namespace into it, push a new frame
                // mark coroutine Running
                self.start_coroutine(id, coro)?;
            }
            HeapData::GatherFuture(gather) => {
                self.spawn_gather_tasks(id, gather)?;
                self.scheduler.block_current_on_gather(id);
                return self.switch_or_yield();
            }
            _ => return Err(TypeError("object is not awaitable")),
        },
        _ => return Err(TypeError("object is not awaitable")),
    }
}
```
`switch_or_yield()` should swap in the next ready task (and keep the run loop going) or return
`FrameExit::ResolveFutures` if no tasks are ready.

Use exception helpers to match CPython messages for:
- non-awaitable objects: `TypeError: object <type> can't be used in 'await' expression`
- re-awaiting a completed coroutine: `RuntimeError: cannot reuse already awaited coroutine`
- re-awaiting an ExternalFuture: same `RuntimeError` message (treat like coroutines)
On coroutine frame return, mark the coroutine `Completed` using the frame's `coroutine_id`.

**External function calls:**
External calls always return `FrameExit::ExternalCall` to the host with a `call_id`. The host then chooses:
- **Sync resolution**: Call `snapshot.run(result)` to push the result and continue (current behavior)
- **Async resolution**: Call `snapshot.run_pending()` to push `ExternalFuture(call_id)` and continue

When the host uses async resolution:
1. `run_pending()` pushes `ExternalFuture(call_id)` onto the stack
2. If the code `await`s this future before it's resolved, the task blocks
3. When all tasks are blocked, `ResolveFutures` is returned with pending call data
4. Host resolves via `FutureSnapshot::resume()` with results

The `call_id` counter (in Scheduler) is always incremented, even for sync resolution, to keep IDs unique.
Pending call data is stored in the scheduler when `run_pending()` is called.

### Phase 6: API Changes

**File: `crates/monty/src/run.rs`**

Add to `RunProgress`:
```rust
/// All async tasks blocked waiting for external call results.
ResolveFutures {
    /// Pending calls that need resolution before execution can continue.
    pending: Vec<PendingCall>,
    /// Execution state for resumption with future results.
    state: FutureSnapshot<T>,
},
```

Modify `RunProgress::FunctionCall` to include `call_id`:
```rust
FunctionCall {
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
    call_id: u32,  // NEW: unique ID for this call
    state: Snapshot<T>,
},
```
The `call_id` is allocated from the scheduler's monotonically increasing counter.
This allows the host to use either sync resolution (`run(result)`) or async resolution (`run_pending()`)
for any external call.
Update `RunProgress::function_call()` helper to return `call_id` as well.

Extend `Snapshot` to support pending futures:
```rust
pub struct Snapshot<T: ResourceTracker> {
    // ... existing fields ...
    /// The call_id from the most recent FunctionCall (stored internally).
    pending_call_id: Option<u32>,
}

impl<T: ResourceTracker> Snapshot<T> {
    /// Resume by pushing ExternalFuture(call_id) instead of a concrete value.
    /// Uses the call_id stored from the FunctionCall that created this Snapshot.
    /// Host doesn't need to track or pass the call_id.
    pub fn run_pending(self, print: &mut impl PrintWriter) -> Result<RunProgress<T>, MontyException>;
}
```

Add `PendingCall` struct:
```rust
pub struct PendingCall {
    pub call_id: u32,  // Same type as CallId
    pub function_name: String,
    pub args: Vec<MontyObject>,
    pub kwargs: Vec<(MontyObject, MontyObject)>,
}
```
If a coroutine/future reaches the API boundary (e.g., final return value),
render it as a `Repr` string rather than erroring.

Add `FutureSnapshot` type (separate from `Snapshot` used for sync external calls):
```rust
/// Execution state paused while waiting for external future results.
///
/// Unlike `Snapshot` (used for sync external calls), `FutureSnapshot` supports
/// incremental resolution - you can provide partial results and Monty will
/// continue running until all tasks are blocked again.
pub struct FutureSnapshot<T: ResourceTracker> {
    executor: Executor,
    vm_state: VMSnapshot,
    heap: Heap<T>,
    namespaces: Namespaces,
}

impl<T: ResourceTracker> FutureSnapshot<T> {
    /// Resume execution with results for some or all pending futures.
    ///
    /// **Incremental resolution**: You don't need to provide all results at once.
    /// If you provide a partial list, Monty will:
    /// 1. Mark those futures as resolved
    /// 2. Unblock any tasks waiting on those futures
    /// 3. Continue running until all tasks are blocked again
    /// 4. Return `ResolveFutures` with the remaining pending calls
    ///
    /// This allows the host to resolve futures as they complete, rather than
    /// waiting for all of them.
    ///
    /// # Arguments
    /// * `results` - List of (call_id, result) pairs. Can be a subset of pending calls.
    /// * `print` - Writer for print output
    ///
    /// # Returns
    /// * `RunProgress::ResolveFutures` - More futures need resolution
    /// * `RunProgress::FunctionCall` - VM hit another external call
    /// * `RunProgress::Complete` - All tasks completed successfully
    /// * `Err(MontyException)` - An unhandled exception occurred
    pub fn resume(
        self,
        results: Vec<(u32, ExternalResult)>,  // u32 matches CallId
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException>;
}
```

**Incremental Resolution Example:**
```
1. Code creates futures f1, f2, f3
2. All tasks block -> ResolveFutures{pending: [f1, f2, f3]}
3. Host resolves f1 -> resume([(1, result1)])
4. Task using f1 runs, creates f4, blocks on f2
5. All tasks blocked -> ResolveFutures{pending: [f2, f3, f4]}
6. Host resolves f2, f3 -> resume([(2, result2), (3, result3)])
7. Tasks continue...
```

### Phase 7: asyncio.gather()

**Scope**: The `asyncio` module provides **only** `gather()`. Other asyncio functions are explicitly
out of scope: `create_task()`, `sleep()`, `wait()`, `wait_for()`, `shield()`, `timeout()`, etc.
These would require additional VM/scheduler features not covered in this plan.

**File: `crates/monty/src/modules/mod.rs`**
- Add `BuiltinModule::Asyncio` variant
- Register `asyncio` module with only `gather` function

**File: `crates/monty/src/asyncio.rs`** (add gather function to existing file)

```rust
/// asyncio.gather(*awaitables) implementation.
///
/// Does NOT spawn tasks immediately - just collects the coroutines.
/// Tasks are spawned when the GatherFuture is awaited (in GetAwaitable).
///
/// When awaited:
/// 1. Each coroutine_id is spawned as a Task
/// 2. task_ids is populated with the spawned TaskIds
/// 3. Current task blocks until all spawned tasks complete
/// 4. Results are collected in order and returned as a list
/// 5. On any task failure, cancel siblings and propagate the exception
pub fn gather(args: Vec<Value>, heap: &mut Heap) -> Result<Value, RunError> {
    // Validate all args are coroutines
    let mut coroutine_ids = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            Value::Ref(id) if heap.get(id).is_coroutine() => {
                coroutine_ids.push(id);
            }
            _ => return Err(TypeError("gather() expects coroutines")),
        }
    }

    // Create GatherFuture on heap
    let gather = GatherFuture {
        coroutine_ids,
        task_ids: Vec::new(),    // Filled when awaited
        results: Vec::new(),      // Filled as tasks complete
        waiter: None,
    };
    let id = heap.allocate(HeapData::GatherFuture(gather))?;
    Ok(Value::Ref(id))
}
```

### Phase 8: Exception Handling

**Task failure:**
1. Mark task as `TaskState::Failed(error)`
2. If task is part of a gather:
   - Mark all sibling tasks as `Failed` (cancel them)
   - Clean up sibling task resources (drop frames, stack values)
   - Propagate exception to the task that called `await gather(...)`
3. If task is the main task (not in a gather):
   - Return error to host
4. If a task is directly awaited (non-gather), propagate its exception to the awaiting task
5. Store failures as `RunError` (convert to `MontyException` only at the API boundary)

**Pending calls for failed/cancelled tasks:**
- Pending external calls remain in `pending_calls` map
- When host provides results via `FutureSnapshot::resume()`:
  - Look up the CallId
  - If the task that created it is `Failed`, silently ignore the result
  - If the task is still alive, resolve normally

**External function returns error:**
- When host calls `resume([(id, ExternalResult::Error(exc))])`:
  - Find the task blocked on that CallId
  - Raise the exception in that task (same as if Python code raised it)
  - Task failure handling applies (cancel siblings if in gather)

**Reference counting cleanup:**
- When cancelling a task, call `drop_with_heap()` on:
  - All values in the task's stack
  - All values in the task's exception_stack
  - Any captured closures in frames
- When dropping a Coroutine:
  - Drop stored namespace values
  - Drop `frame_cells` refs
- When dropping a GatherFuture:
  - Drop stored results and decrement refs for coroutine/task heap IDs
- When removing a pending call (resolved or cancelled):
  - Drop the stored `ArgValues` to avoid leaks

## Files to Modify

| File | Changes |
|------|---------|
| `crates/monty/src/value.rs` | Add `ExternalFuture(CallId)` variant |
| `crates/monty/src/heap.rs` | Add `Coroutine`, `GatherFuture` variants |
| `crates/monty/src/function.rs` | Add `is_async: bool` field |
| `crates/monty/src/parse.rs` | Parse `async def`, `await` expressions |
| `crates/monty/src/expressions.rs` | Add `Await(Box<ExprLoc>)` variant |
| `crates/monty/src/bytecode/op.rs` | Add `GetAwaitable` opcode |
| `crates/monty/src/bytecode/compiler.rs` | Compile async def + await |
| `crates/monty/src/bytecode/vm/call.rs` | Create `Coroutine` for async functions at call time |
| `crates/monty/src/bytecode/vm/mod.rs` | Scheduler integration, GetAwaitable handling, `call_id` on ExternalCall |
| `crates/monty/src/run.rs` | Add `call_id` to FunctionCall, add `ResolveFutures`, `FutureSnapshot`, `PendingCall`, `Snapshot::run_pending()` |
| `crates/monty/src/object.rs` | Represent `ExternalFuture`/`Coroutine`/`GatherFuture` in outputs (repr fallback) |
| `crates/monty/src/modules/mod.rs` | Add `BuiltinModule::Asyncio`, register `gather` |

## New Files

| File | Purpose |
|------|---------|
| `crates/monty/src/asyncio.rs` | `CallId`, `Coroutine`, `GatherFuture` types + `gather()` function |
| `crates/monty/src/bytecode/vm/scheduler.rs` | `Scheduler`, `Task`, `TaskId`, `TaskState` |

## Verification

1. **Integration tests in `crates/monty/test_cases/`** (prefer consolidation):
   - `async__all.py` with sections for basic await, external futures, gather, cancellation
   - Add separate traceback files only when needed for error cases

2. **Run test suite**:
   ```bash
   make test-ref-count-panic
   make test-py
   ```

### Phase 9: Scheduler Serialization

The scheduler state must be serializable for async execution to be paused/resumed across host calls.

**File: `crates/monty/src/bytecode/vm/scheduler.rs`**

Add serde derives to all scheduler types:
```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Task { ... }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum TaskState { ... }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SerializedTaskFrame { ... }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingCallData { ... }

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Scheduler { ... }
```

**File: `crates/monty/src/bytecode/vm/mod.rs`**

Update `VMSnapshot` to include scheduler state:
```rust
pub struct VMSnapshot {
    stack: Vec<Value>,
    frames: Vec<SerializedFrame>,
    exception_stack: Vec<Value>,
    instruction_ip: usize,
    scheduler: Scheduler,  // NEW: include full scheduler state
}
```

Update `VM::into_snapshot()` to include the scheduler:
```rust
pub fn into_snapshot(self) -> VMSnapshot {
    VMSnapshot {
        stack: self.stack,
        frames: self.frames.into_iter().map(|f| f.serialize()).collect(),
        exception_stack: self.exception_stack,
        instruction_ip: self.instruction_ip,
        scheduler: self.scheduler,  // NEW
    }
}
```

Update `VM::restore()` to restore the scheduler from the snapshot.

### Phase 10: Pending Call Data & Cancelled Task Handling

**Store pending call data when `run_pending()` is called:**

**File: `crates/monty/src/run.rs`**

In `Snapshot::run_pending()`, store the pending call data in the scheduler before pushing the `ExternalFuture`:
```rust
pub fn run_pending(mut self, print: &mut impl PrintWriter) -> Result<RunProgress<T>, MontyException> {
    let call_id = crate::asyncio::CallId::new(self.pending_call_id);

    // Store pending call data in scheduler
    // Note: Need to capture ext_function_id and args from the FunctionCall that created this snapshot
    // This requires storing them in Snapshot when it's created from ExternalCall

    // ... rest of implementation
}
```

This requires updating `Snapshot` to store the `ext_function_id` and `args` from the `ExternalCall` that created it, so they can be added to `pending_calls` when `run_pending()` is called.

**Ignore results for cancelled/failed tasks:**

**File: `crates/monty/src/bytecode/vm/mod.rs`**

Update `resolve_future` to check if the creator task is cancelled:
```rust
pub fn resolve_future(&mut self, call_id: CallId, value: Value) {
    // Check if the creator task has been cancelled
    if let Some(creator_task) = self.scheduler.get_pending_call_creator(call_id) {
        if self.scheduler.is_task_failed(creator_task) {
            // Task was cancelled - silently ignore the result and drop the value
            value.drop_with_heap(self.heap);
            return;
        }
    }
    self.scheduler.resolve(call_id, value);
}
```

### Phase 11: Integration Tests

**File: `crates/monty/test_cases/async__all.py`**

Create comprehensive test file with sections:

```python
# === Basic async/await ===
# Test simple async function definition and await

# === External futures ===
# Test awaiting ExternalFuture values
# (Note: These tests require external function support, may need separate test harness)

# === Coroutine creation ===
# Test that calling async function returns coroutine without executing

# === Coroutine await ===
# Test that awaiting coroutine executes the function body

# === Double await error ===
# Test that re-awaiting a coroutine raises RuntimeError

# === Non-awaitable error ===
# Test that awaiting non-awaitable raises TypeError

# === asyncio.gather basic ===
# Test gather with multiple coroutines

# === gather result ordering ===
# Test that gather returns results in argument order
```

**Separate traceback test files** (as needed for error cases):
- `async__double_await_error.py` - Test "cannot reuse already awaited coroutine" error
- `async__not_awaitable_error.py` - Test "object X can't be used in 'await' expression" error

**Run verification:**
```bash
make test-ref-count-panic
make test-py
```

## Out of Scope

The following are explicitly **not** included in this implementation:

1. **Async generators** - `yield` inside `async def` is a syntax error
2. **asyncio functions** - Only `gather()` is implemented. Not included:
   - `asyncio.create_task()` - would need explicit task creation without await
   - `asyncio.sleep()` - would need timer integration with host
   - `asyncio.wait()`, `asyncio.wait_for()`, `asyncio.shield()`, `asyncio.timeout()`
3. **`async for` / `async with`** - would require `__aiter__`, `__aenter__`/`__aexit__` protocols
4. **Coroutine `.send()` / `.throw()`** - simplified single-shot coroutine model
5. **Task cancellation API** - tasks are only cancelled implicitly when a gather fails

TODO:
* fix test_cases cpython test
* replace mess in `call_method`
* fix ty, fix ty duplicate notifications for ```error[invalid-syntax]: `await` outside of an asynchronous function```
* add `asyncio` to typeshed
