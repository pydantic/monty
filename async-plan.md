# Async/Await Implementation Plan for Monty

## Overview

Add async/await support to Monty where the host acts as the event loop. External function calls return `ExternalFuture` objects that can be awaited. When all tasks are blocked on futures, control returns to the host with pending future IDs.

## Key Design Decisions

1. **Host is event loop** - Monty yields pending calls, host executes and resumes
2. **ExternalFuture model** - External calls return futures, results provided later
3. **All tasks blocked** - Yield to host only when every task is waiting on external call
4. **Cancel all on exception** - Exception propagates, cancels sibling tasks
5. **Simplified coroutines** - Async functions must be awaited, no `.send()`/`.throw()`
6. **Sequential integer call IDs** - Simple incrementing counter
3. **Ignore other crates for now**: Ignore `crates/monty-python/` and `/crates/monty-type-checking` for now, we'll fix that later

## Execution Flow

Every external function call returns to the host immediately (same as sync mode), but with a `call_id` for tracking:

```
1. ext_func(args)    -> Returns FunctionCall{name, args, call_id, state} to host
2. Host starts func  -> Calls state.run(ExternalFuture(call_id))
3. Code continues    -> Gets ExternalFuture(call_id) object, may hit more ext calls (goto 1)
4. await future      -> If not resolved, task blocks
5. All tasks blocked -> Returns ResolveFutures{pending: [...], state: FutureSnapshot}
6. Host provides     -> Calls state.resume([(id, result), ...])
                        (can be partial - not all pending calls required)
7. Tasks unblock     -> Continue execution, may return to step 1 or 5
8. All done          -> Returns Complete(result)
```

**Key Points**:
- External calls ALWAYS return to host with `FunctionCall` (consistent with sync mode)
- `FunctionCall` now includes `call_id` so host can correlate calls with futures
- `FutureSnapshot::resume()` accepts partial results for incremental resolution
- Results for failed/cancelled tasks are silently ignored

## Implementation Phases

### Phase 1: Core Types

**File: `crates/monty/src/asyncio.rs` (new)**

All async-related types in one file:
```rust
/// Unique identifier for external function calls (sequential integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId(pub u32);

/// A coroutine object (unevaluated async function call).
pub struct Coroutine {
    pub func_id: FunctionId,
    pub cells: Vec<HeapId>,      // For closures
    pub defaults: Vec<Value>,    // Evaluated defaults
    pub args: ArgValues,         // Call arguments
}

/// A gather() result tracking multiple coroutines/tasks.
pub struct GatherFuture {
    /// Coroutine HeapIds to spawn (set at creation).
    pub coroutine_ids: Vec<HeapId>,
    /// TaskIds of spawned tasks (set when awaited).
    pub task_ids: Vec<TaskId>,
    /// Results from each task, in order (filled as tasks complete).
    pub results: Vec<Option<Value>>,
}
```

**File: `crates/monty/src/value.rs`**
- Add `ExternalFuture(CallId)` variant to `Value` enum

**File: `crates/monty/src/heap.rs`**
- Add `Coroutine(Coroutine)` variant to `HeapData` enum
- Add `GatherFuture(GatherFuture)` variant to `HeapData` enum

### Phase 2: Function Metadata

**File: `crates/monty/src/function.rs`**
- Add `is_async: bool` field to `Function` struct

**File: `crates/monty/src/parse.rs`**
- Parse `async def` (set `is_async` flag)
- Parse `await expr` expressions

**File: `crates/monty/src/expressions.rs`**
- Add `Await(Box<ExprLoc>)` variant to `Expr` enum

### Phase 3: Compilation

**File: `crates/monty/src/bytecode/op.rs`**
Add two new opcodes:
```rust
/// Await the TOS value. Handles ExternalFuture, Coroutine, and GatherFuture.
GetAwaitable,

/// Create a coroutine object from an async function call.
/// Operand: u16 func_id (same as MakeFunction)
/// Stack: [defaults..., cells...] -> [Coroutine]
MakeCoroutine,
```

**File: `crates/monty/src/bytecode/compiler.rs`**

Compile `await expr`:
1. Compile expression (pushes awaitable onto stack)
2. Emit `GetAwaitable`

Compile async function **call**:
- When calling a function marked `is_async`, emit `MakeCoroutine` instead of `CallFunction`
- `MakeCoroutine` creates a `Coroutine` heap object with captured args, does NOT execute

Compile async function **definition**:
- Same as regular function, but set `is_async: true` in `Function` struct

### Phase 4: Task Scheduler

**File: `crates/monty/src/bytecode/vm/scheduler.rs` (new)**
```rust
/// Unique identifier for a task (sequential integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u32);

/// Task state for async execution.
pub enum TaskState {
    Ready,
    Blocked(CallId),
    Completed(Value),
    Failed,
}

/// A single async task with its own execution context.
pub struct Task {
    pub id: TaskId,
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub exception_stack: Vec<Value>,
    pub state: TaskState,
}

/// Scheduler for managing concurrent async tasks.
///
/// Created lazily on first async operation (await or async function call).
/// When None, VM operates in sync mode (current behavior).
pub struct Scheduler {
    tasks: Vec<Task>,
    ready_queue: VecDeque<TaskId>,
    current_task: Option<TaskId>,
    next_task_id: u32,
    next_call_id: u32,
    /// Maps CallId -> (ext_func_id, args) for pending calls
    pending_calls: HashMap<CallId, (ExtFunctionId, ArgValues)>,
    /// Maps CallId -> resolved Value
    resolved: HashMap<CallId, Value>,
}

impl Scheduler {
    /// Create scheduler, moving current VM state into the "main" task.
    pub fn new_with_main_task(frames: Vec<CallFrame>, stack: Vec<Value>) -> Self;

    /// Spawn a new task from a coroutine. Returns the TaskId.
    pub fn spawn(&mut self, coroutine: &Coroutine, heap: &mut Heap) -> TaskId;

    /// Allocate a new CallId for an external function call.
    pub fn allocate_call_id(&mut self) -> CallId;

    /// Mark the current task as blocked on a CallId.
    pub fn block_current(&mut self, call_id: CallId);

    /// Resolve a CallId with a value. Unblocks any task waiting on it.
    pub fn resolve(&mut self, call_id: CallId, value: Value);

    /// Get resolved value for a CallId, if available.
    pub fn get_resolved(&self, call_id: CallId) -> Option<Value>;

    /// Switch to the next ready task. Returns false if no ready tasks.
    pub fn switch_to_next(&mut self) -> bool;

    /// Get all pending (unresolved) CallIds.
    pub fn pending_call_ids(&self) -> Vec<CallId>;
}
```

### Phase 5: VM Changes

**File: `crates/monty/src/bytecode/vm/mod.rs`**

Add to `FrameExit`:
```rust
/// All async tasks blocked waiting for external call results.
ResolveFutures(Vec<CallId>),
```

Add to VM:
```rust
/// Optional scheduler for async execution (None for sync mode).
scheduler: Option<Box<Scheduler>>,
```

**GetAwaitable opcode handling:**
```rust
Opcode::GetAwaitable => {
    let awaitable = self.pop();

    // Ensure scheduler exists (lazy init on first await)
    self.ensure_scheduler();

    match awaitable {
        Value::ExternalFuture(call_id) => {
            if let Some(result) = self.scheduler().get_resolved(call_id) {
                self.push(result);
            } else {
                // Block current task on this future
                self.scheduler().block_current(call_id);
                return self.yield_or_switch();
            }
        }
        Value::Ref(id) => {
            match self.heap.get(id) {
                HeapData::Coroutine(coro) => {
                    // Execute coroutine inline (push its frames onto current task)
                    self.execute_coroutine(coro)?;
                }
                HeapData::GatherFuture(gather) => {
                    // Spawn a task for each coroutine in the gather
                    for coro_id in &gather.coroutine_ids {
                        self.scheduler().spawn(coro_id, &mut self.heap);
                    }
                    // Block current task until all gather tasks complete
                    self.scheduler().block_on_gather(id);
                    return self.yield_or_switch();
                }
                _ => return Err(TypeError("object is not awaitable")),
            }
        }
        _ => return Err(TypeError("object is not awaitable")),
    }
}

/// Try to switch to another ready task, or yield to host if all blocked.
fn yield_or_switch(&mut self) -> Result<FrameExit, RunError> {
    if self.scheduler().switch_to_next() {
        // Continue running in the new task
        Ok(FrameExit::Continue) // Internal signal to keep running
    } else {
        // All tasks blocked - yield to host
        Ok(FrameExit::ResolveFutures(self.scheduler().pending_call_ids()))
    }
}
```

**External function call in async context:**
- When calling external function inside async def:
  1. Allocate `CallId` from scheduler
  2. Store `(call_id, ext_func_id, args)` in pending_calls
  3. Return `FrameExit::ExternalCall` with call_id
  4. On resume, push `ExternalFuture(call_id)` and continue

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

Add `PendingCall` struct:
```rust
pub struct PendingCall {
    pub call_id: u32,  // Same type as CallId
    pub function_name: String,
    pub args: Vec<MontyObject>,
    pub kwargs: Vec<(MontyObject, MontyObject)>,
}
```

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

**File: `crates/monty/src/modules/mod.rs`**
- Add `BuiltinModule::Asyncio` variant
- Register `asyncio` module with `gather` function

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
    };
    let id = heap.allocate(HeapData::GatherFuture(gather))?;
    Ok(Value::Ref(id))
}
```

### Phase 8: Exception Handling

**Task failure:**
1. Mark task as `TaskState::Failed`
2. If task is part of a gather:
   - Mark all sibling tasks as `Failed` (cancel them)
   - Clean up sibling task resources (drop frames, stack values)
   - Propagate exception to the task that called `await gather(...)`
3. If task is the main task (not in a gather):
   - Return error to host

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

## Files to Modify

| File | Changes |
|------|---------|
| `crates/monty/src/value.rs` | Add `ExternalFuture(CallId)` variant |
| `crates/monty/src/heap.rs` | Add `Coroutine`, `GatherFuture` variants |
| `crates/monty/src/function.rs` | Add `is_async: bool` field |
| `crates/monty/src/parse.rs` | Parse `async def`, `await` expressions |
| `crates/monty/src/expressions.rs` | Add `Await(Box<ExprLoc>)` variant |
| `crates/monty/src/bytecode/op.rs` | Add `GetAwaitable`, `MakeCoroutine` opcodes |
| `crates/monty/src/bytecode/compiler.rs` | Compile async def, await, MakeCoroutine |
| `crates/monty/src/bytecode/vm/mod.rs` | Add scheduler field, GetAwaitable/MakeCoroutine handling |
| `crates/monty/src/run.rs` | Add `call_id` to FunctionCall, add `ResolveFutures`, `FutureSnapshot`, `PendingCall` |
| `crates/monty/src/modules/mod.rs` | Add `BuiltinModule::Asyncio`, register `gather` |

## New Files

| File | Purpose |
|------|---------|
| `crates/monty/src/asyncio.rs` | `CallId`, `Coroutine`, `GatherFuture` types + `gather()` function |
| `crates/monty/src/bytecode/vm/scheduler.rs` | `Scheduler`, `Task`, `TaskId`, `TaskState` |

## Verification

1. **Unit tests**: Add tests for each new opcode and type
2. **Integration tests in `crates/monty/test_cases/`**:
   - `async__basic.py` - Simple async def and await
   - `async__external.py` - External function calls with futures
   - `async__gather.py` - asyncio.gather() usage
   - `async__exception.py` - Exception handling and cancellation

3. **Run test suite**:
   ```bash
   make test-ref-count-panic
   make test-py
   ```
