//! Task scheduler for async execution and call ID allocation.
//!
//! # Task Model
//!
//! - Task 0 is the "main task" which uses the VM's stack/frames directly
//! - Spawned tasks (1+) store their own execution context in the Task struct
//! - When switching tasks, the scheduler swaps contexts with the VM

use std::{collections::VecDeque, mem};

use ahash::AHashMap;

use crate::{
    asyncio::{Awaiter, CallId, ExternalFutureState, TaskId},
    exception_private::RunError,
    heap::{ContainsHeap, DropWithHeap, Heap, HeapId, HeapReadOutput, HeapReader},
    intern::FunctionId,
    parse::CodeRange,
    resource::ResourceTracker,
    value::Value,
};

/// Task execution state for async scheduling.
///
/// Tracks whether a task is ready to run, blocked waiting for something,
/// or has completed (successfully or with an error).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum TaskState {
    /// Task is ready to execute (in the ready queue).
    Ready,
    /// Task is blocked waiting for an awaitable on the heap to settle.
    ///
    /// Holds an inc_ref on the awaitable (currently `ExternalFuture` or
    /// `GatherFuture` — `heap.read(id)` recovers the kind) so the heap
    /// entry stays alive while the task is parked. Resolution dispatches
    /// via the awaitable's own awaiter slot (`Awaiter::Task(task_id)` on
    /// `ExternalFuture`, `AwaitedGather.awaiter = Awaiter::Task(task_id)`
    /// on `GatherFuture`) rather than from the variant tag.
    Blocked(HeapId),
    /// Task completed successfully with a return value.
    Completed(Value),
    /// Task failed with an error.
    Failed(RunError),
}

impl DropWithHeap for TaskState {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        match self {
            Self::Ready | Self::Failed(_) => {}
            Self::Blocked(id) => heap.heap_mut().dec_ref(id),
            Self::Completed(value) => value.drop_with_heap(heap),
        }
    }
}

/// A single async task with its own execution context.
///
/// The main task (task 0) doesn't store its own frames/stack - it uses the VM's
/// directly. Spawned tasks store their execution context here so they can be
/// swapped in and out.
///
/// # Context Switching
///
/// When switching away from a non-main task, its context is saved here.
/// When switching to it, the context is loaded into the VM.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Task {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Serialized call frames for this task's execution.
    /// Empty for the main task (which uses VM's frames directly).
    pub frames: Vec<SerializedTaskFrame>,
    /// Operand stack for this task.
    /// Empty for the main task (which uses VM's stack directly).
    pub stack: Vec<Value>,
    /// Exception stack for nested except blocks.
    pub exception_stack: Vec<Value>,
    /// VM-level instruction_ip (for exception table lookup).
    pub instruction_ip: usize,
    /// Coroutine being executed by this task (if any).
    /// Used to mark the coroutine as Completed when the task finishes.
    pub coroutine_id: Option<HeapId>,
    /// GatherFuture this task belongs to (if spawned by gather).
    /// Used to cancel sibling tasks when this task fails. The gather itself
    /// stores the slot-index mapping under `AwaitedGather::pending_children`.
    pub gather_id: Option<HeapId>,
    /// Current execution state.
    pub state: TaskState,
}

impl DropWithHeap for Task {
    fn drop_with_heap<H: ContainsHeap>(mut self, heap: &mut H) {
        for value in self.stack.drain(..) {
            value.drop_with_heap(heap);
        }
        for value in self.exception_stack.drain(..) {
            value.drop_with_heap(heap);
        }
        self.state.drop_with_heap(heap);
        if let Some(coro_id) = self.coroutine_id.take() {
            heap.heap_mut().dec_ref(coro_id);
        }
        if let Some(gid) = self.gather_id.take() {
            heap.heap_mut().dec_ref(gid);
        }
    }
}

/// Serialized call frame for task storage.
///
/// Similar to `SerializedFrame` but used within the scheduler for task context.
/// Cannot store `&Code` references - uses `FunctionId` to look up code on resume.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SerializedTaskFrame {
    /// Which function's code this frame executes (None = module-level).
    pub function_id: Option<FunctionId>,
    /// Instruction pointer within this frame's bytecode.
    pub ip: usize,
    /// Base index into the VM stack for this frame's locals region.
    pub stack_base: usize,
    /// Number of local variable slots (0 for module-level frames).
    pub locals_count: u16,
    /// Base index into the VM-wide `exception_stack` for this frame.
    /// See `CallFrame.exception_stack_base`.
    pub exception_stack_base: usize,
    /// Call site position (for tracebacks).
    pub call_position: Option<CodeRange>,
}

impl Task {
    /// Creates a new task in the Ready state.
    ///
    /// # Arguments
    /// * `id` - Unique task identifier
    /// * `coroutine_id` - Optional HeapId of the coroutine being executed
    /// * `gather_id` - Optional HeapId of the GatherFuture this task belongs to
    pub fn new(id: TaskId, coroutine_id: Option<HeapId>, gather_id: Option<HeapId>) -> Self {
        Self {
            id,
            frames: Vec::new(),
            stack: Vec::new(),
            exception_stack: Vec::new(),
            instruction_ip: 0,
            coroutine_id,
            gather_id,
            state: TaskState::Ready,
        }
    }

    /// Returns true if this task has completed (successfully or with failure).
    #[inline]
    pub fn is_finished(&self) -> bool {
        matches!(self.state, TaskState::Completed(_) | TaskState::Failed(_))
    }
}

/// Scheduler for managing call IDs, async tasks, and external call tracking.
///
/// Always present on the VM (not optional). Owns the `next_call_id` counter
/// used by both sync and async code paths, plus all async-related state:
/// - Task management (creation, scheduling, completion)
/// - External call tracking and resolution
///
/// # Main Task
///
/// Task 0 is the "main task" which executes using the VM's stack/frames directly.
/// It's always created at scheduler initialization but doesn't store its own context
/// (the VM holds it). Spawned tasks (1+) store their context in the Task struct.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Scheduler {
    /// All tasks keyed by their `TaskId`.
    tasks: AHashMap<TaskId, Task>,
    /// Queue of task IDs ready to execute.
    ready_queue: VecDeque<TaskId>,
    /// Currently executing task (None only during task switching).
    current_task: Option<TaskId>,
    /// Counter for generating new task IDs.
    next_task_id: u32,
    /// Counter for external call IDs (always incremented, even for sync resolution).
    next_call_id: u32,
    /// Host-side index mapping each unresolved external call to its
    /// `HeapData::ExternalFuture` entry. The scheduler holds an inc_ref on
    /// each value until the host resolves or fails the call — that ref keeps
    /// the future entry alive between yield and resume even if no awaiter is
    /// holding a `Value::Ref` to it.
    pending_externals: AHashMap<CallId, HeapId>,
    /// Index mapping a spawned coroutine's `HeapId` to the `TaskId` driving
    /// it. Populated in [`Scheduler::spawn`] and removed in
    /// [`Scheduler::cancel_task`]. Lets `GatherFuture` and other call sites
    /// dispatch from coroutine heap id back to the driving task without
    /// scanning all tasks.
    coroutine_to_task: AHashMap<HeapId, TaskId>,
}

impl Scheduler {
    /// Creates a new scheduler with the main task (task 0) as current.
    ///
    /// The main task uses the VM's stack/frames directly and is always present.
    /// It starts as the current task (not in the ready queue) since it runs
    /// immediately without needing to be scheduled.
    pub fn new() -> Self {
        let main_task_id = TaskId::default();
        let mut main_task = Task::new(main_task_id, None, None);
        // Main task starts Running, not Ready (it's the current task, not waiting)
        main_task.state = TaskState::Ready; // Will be set properly when it blocks
        let mut tasks = AHashMap::new();
        tasks.insert(main_task_id, main_task);
        Self {
            tasks,
            ready_queue: VecDeque::new(), // Main task is current, not in ready queue
            current_task: Some(main_task_id),
            next_task_id: 1,
            next_call_id: 0,
            pending_externals: AHashMap::new(),
            coroutine_to_task: AHashMap::new(),
        }
    }

    /// Returns the currently executing task ID.
    ///
    /// Returns `None` only during task switching operations.
    #[inline]
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.current_task
    }

    /// Returns a reference to a task by ID.
    ///
    /// # Panics
    /// Panics if the task ID doesn't exist.
    #[inline]
    pub fn get_task(&self, task_id: TaskId) -> &Task {
        self.tasks.get(&task_id).expect("Scheduler::get_task: task not found")
    }

    /// Returns a mutable reference to a task by ID.
    ///
    /// # Panics
    /// Panics if the task ID doesn't exist.
    #[inline]
    pub fn get_task_mut(&mut self, task_id: TaskId) -> &mut Task {
        self.tasks
            .get_mut(&task_id)
            .expect("Scheduler::get_task_mut: task not found")
    }

    /// Allocates a new CallId for an external function call.
    ///
    /// The counter always increments, even for sync resolution, to keep IDs unique.
    pub fn allocate_call_id(&mut self) -> CallId {
        let id = CallId::new(self.next_call_id);
        self.next_call_id += 1;
        id
    }

    /// Registers a freshly created `ExternalFuture` for `call_id`.
    ///
    /// The scheduler inc_refs `future_id` so the entry stays alive between
    /// the yield to the host and the matching `resolve_future` / `fail_future`
    /// call, even if no awaiter holds a `Value::Ref` to it.
    pub fn add_pending_external(&mut self, call_id: CallId, future_id: HeapId, heap: &Heap<impl ResourceTracker>) {
        heap.inc_ref(future_id);
        let prev = self.pending_externals.insert(call_id, future_id);
        debug_assert!(prev.is_none(), "add_pending_external: CallId already registered");
    }

    /// Removes and returns the `ExternalFuture` heap id for `call_id`, if any.
    ///
    /// The caller becomes responsible for the inc_ref previously held by the
    /// scheduler (typically dec_ref'd once the state transition is committed).
    pub fn take_pending_external(&mut self, call_id: CallId) -> Option<HeapId> {
        self.pending_externals.remove(&call_id)
    }

    /// Marks the current task as `Blocked` on the awaitable at `awaitable_id`.
    ///
    /// The task will be unblocked when the awaitable settles and its awaiter
    /// slot routes back here (`Awaiter::Task(task_id)` on either
    /// `ExternalFuture::Pending` or `AwaitedGather`).
    pub fn block_current_on(&mut self, awaitable_id: HeapId, heap: &Heap<impl ResourceTracker>) {
        if let Some(task_id) = self.current_task {
            let task = self.get_task_mut(task_id);
            heap.inc_ref(awaitable_id);
            task.state = TaskState::Blocked(awaitable_id);
        }
    }

    /// Returns all pending (unresolved) CallIds.
    pub fn pending_call_ids(&self) -> Vec<CallId> {
        self.pending_externals.keys().copied().collect()
    }

    /// Removes a task from the ready queue.
    ///
    /// Used when handling the main task directly (via `prepare_main_task_after_resolve`)
    /// instead of through the normal task switching mechanism.
    pub fn remove_from_ready_queue(&mut self, task_id: TaskId) {
        self.ready_queue.retain(|&id| id != task_id);
    }

    /// Spawns a new task from a coroutine, enforcing one-task-per-coroutine.
    ///
    /// Returns `None` if `coroutine_id` is already driving a task — caught
    /// here because cross-gather reuse can hit two spawns while both
    /// coroutine states are still `New`, so the state check in
    /// `await_coroutine` doesn't catch it. Callers translate `None` into a
    /// `RuntimeError: cannot reuse already awaited coroutine`.
    ///
    /// Both `coroutine_id` and `gather_id` (when present) become **owning**
    /// references held by the new task; the matching `dec_ref` happens in
    /// [`Scheduler::cancel_task`].
    #[must_use]
    pub fn spawn(
        &mut self,
        heap: &Heap<impl ResourceTracker>,
        coroutine_id: HeapId,
        gather_id: Option<HeapId>,
    ) -> Option<TaskId> {
        if self.coroutine_to_task.contains_key(&coroutine_id) {
            return None;
        }

        let task_id = TaskId::new(self.next_task_id);
        self.next_task_id += 1;

        // Take ownership of the heap references — the task now holds an inc_ref'd
        // pointer to its coroutine and (if applicable) its enclosing gather.
        heap.inc_ref(coroutine_id);
        if let Some(gid) = gather_id {
            heap.inc_ref(gid);
        }

        let task = Task::new(task_id, Some(coroutine_id), gather_id);
        self.tasks.insert(task_id, task);
        self.coroutine_to_task.insert(coroutine_id, task_id);
        self.ready_queue.push_back(task_id);

        Some(task_id)
    }

    /// Returns the task driving `coroutine_id`, if any.
    ///
    /// Each spawned task owns exactly one coroutine for its lifetime; this
    /// looks up the inverse mapping populated in [`Scheduler::spawn`].
    #[inline]
    pub fn task_for_coroutine(&self, coroutine_id: HeapId) -> Option<TaskId> {
        self.coroutine_to_task.get(&coroutine_id).copied()
    }

    /// Gets the next ready task from the queue.
    ///
    /// Returns `None` if no tasks are ready.
    pub fn next_ready_task(&mut self) -> Option<TaskId> {
        self.ready_queue.pop_front()
    }

    /// Replaces a task's state, properly releasing any heap references owned
    /// by the previous state.
    pub fn set_state(&mut self, task_id: TaskId, new_state: TaskState, heap: &mut Heap<impl ResourceTracker>) {
        let task = self.get_task_mut(task_id);
        let old_state = mem::replace(&mut task.state, new_state);
        old_state.drop_with_heap(heap);
    }

    /// Adds a task back to the ready queue.
    pub fn make_ready(&mut self, task_id: TaskId, heap: &mut Heap<impl ResourceTracker>) {
        self.set_state(task_id, TaskState::Ready, heap);
        self.ready_queue.push_back(task_id);
    }

    /// Sets the current task.
    pub fn set_current_task(&mut self, task_id: Option<TaskId>) {
        self.current_task = task_id;
    }

    /// Marks a task as failed with an error.
    ///
    /// If the task is part of a gather, returns the gather_id so the caller
    /// can collect siblings from the gather on the heap.
    ///
    /// # Returns
    /// The gather_id if this task belongs to a gather (for sibling lookup).
    pub fn fail_task(
        &mut self,
        task_id: TaskId,
        error: RunError,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Option<HeapId> {
        let gather_id = self.get_task(task_id).gather_id;
        self.set_state(task_id, TaskState::Failed(error), heap);
        gather_id
    }

    /// Cancels a task, fully releasing its resources and removing it from the
    /// scheduler.
    ///
    /// Drops the task's stack, exception stack, any pending `Completed`
    /// result, and tears down any inner gather it was blocked on. After this
    /// call the task no longer exists in `Scheduler::tasks`; its owning
    /// references to its coroutine and (outer) gather are released by the
    /// `Task::drop_with_heap` call at the end.
    pub fn cancel_task(&mut self, task_id: TaskId, heap: &mut HeapReader<'_, impl ResourceTracker>) {
        // No-op if the task has already been removed (idempotent — finalization
        // sites may iterate task ids that include already-cancelled siblings).
        let Some(task) = self.tasks.remove(&task_id) else {
            return;
        };

        // If we're cancelling the current task, clear `current_task` so callers
        // don't try to look up a task that's about to be dropped (e.g.
        // `resume_with_resolved_futures` after `fail_for_call` tore down the
        // gather containing the previously-current task).
        if self.current_task == Some(task_id) {
            self.current_task = None;
        }

        if let Some(coroutine_id) = task.coroutine_id {
            self.coroutine_to_task.remove(&coroutine_id);
        }

        if !task.is_finished() {
            self.ready_queue.retain(|&id| id != task_id);

            // If blocked on an awaitable, dispatch by kind via `heap.read`.
            // For a gather: recursively cancel its task children — external
            // children manage themselves via the owning `Awaiter::GatherSlot`
            // (the gather stays alive until each external resolves and
            // releases its inc_ref), but spawned tasks have no such anchor
            // and would otherwise linger in `self.tasks` holding inc_refs.
            // For an external future: no extra teardown.
            if let TaskState::Blocked(blocked_id) = task.state
                && let HeapReadOutput::GatherFuture(gather) = heap.read(blocked_id)
            {
                let inner_task_ids: Vec<TaskId> = gather
                    .get(heap)
                    .as_awaited()
                    .map(|awaited| {
                        awaited
                            .pending_children
                            .keys()
                            .filter_map(|id| self.coroutine_to_task.get(id).copied())
                            .collect()
                    })
                    .unwrap_or_default();
                drop(gather);
                for inner_task_id in inner_task_ids {
                    self.cancel_task(inner_task_id, heap);
                }
            }
        }

        task.drop_with_heap(heap);
    }

    /// Records a host-side failure for `call_id` and returns the awaiter the
    /// caller should walk to propagate the error.
    ///
    /// Looks up the `ExternalFuture` heap entry, transitions it to `Failed`
    /// with a clone of the error, and yields the awaiter that owned the
    /// future's `Pending` slot — except for `Awaiter::Task(t)` where `t` is a
    /// child of a gather: in that case we tear the gather down here (rather
    /// than leaving the parked task `Failed` for a sibling's resolution to
    /// discover later) and return the gather's awaiter instead, so the
    /// caller's chain walk picks up at the right level.
    ///
    /// The returned `Awaiter` is owned (callers must walk it via
    /// `deliver_awaiter_failure`, which drops every link).
    ///
    /// Returns `None` when there's nothing to propagate (unknown CallId,
    /// already-resolved future, or the future had no awaiter — the failure
    /// is simply cached on the future for replay).
    #[must_use]
    pub fn fail_for_call(
        &mut self,
        call_id: CallId,
        error: &RunError,
        heap: &mut HeapReader<'_, impl ResourceTracker>,
    ) -> Option<Awaiter> {
        let future_id = self.pending_externals.remove(&call_id)?;

        let HeapReadOutput::ExternalFuture(mut fut) = heap.read(future_id) else {
            panic!("pending_externals entry doesn't point to an ExternalFuture")
        };
        let awaiter = match mem::replace(&mut fut.get_mut(heap).state, ExternalFutureState::Failed(error.clone())) {
            ExternalFutureState::Pending { awaiter } => awaiter,
            ExternalFutureState::Resolved(_) | ExternalFutureState::Failed(_) => {
                panic!("fail_for_call: future was already resolved")
            }
        };
        drop(fut);
        heap.dec_ref(future_id);

        match awaiter {
            None => None,
            Some(Awaiter::Task(task_id)) => {
                let gather_id = self.tasks.get(&task_id).and_then(|t| t.gather_id);
                if let Some(gather_id) = gather_id {
                    // The task that was awaiting the future is itself in a
                    // gather. Tear that gather down here so the failure
                    // anchors at the same site as the resolution; return
                    // the gather's awaiter for the caller to chain.
                    let HeapReadOutput::GatherFuture(mut gather_rd) = heap.read(gather_id) else {
                        panic!("gather_id doesn't point to a GatherFuture")
                    };
                    let outer_awaiter = gather_rd.fail(self, heap, error);
                    drop(gather_rd);
                    Some(outer_awaiter)
                } else {
                    Some(Awaiter::Task(task_id))
                }
            }
            Some(Awaiter::GatherSlot { gather, .. }) => {
                let HeapReadOutput::GatherFuture(mut gather_rd) = heap.read(gather) else {
                    panic!("gather_id doesn't point to a GatherFuture")
                };
                let outer_awaiter = gather_rd.fail(self, heap, error);
                drop(gather_rd);
                // Release the inc_ref the destructured `GatherSlot` owned
                // on `gather` (we tore that gather down above).
                heap.dec_ref(gather);
                Some(outer_awaiter)
            }
        }
    }

    /// Returns true if a task has been cancelled or failed.
    #[inline]
    pub fn is_task_failed(&self, task_id: TaskId) -> bool {
        self.tasks
            .get(&task_id)
            .is_some_and(|task| matches!(task.state, TaskState::Failed(_)))
    }

    /// Returns true if a task with `task_id` currently exists in the
    /// scheduler. Cancelled tasks are removed from the map, so this returning
    /// `false` means the task is gone.
    #[inline]
    pub fn has_task(&self, task_id: TaskId) -> bool {
        self.tasks.contains_key(&task_id)
    }

    /// Cleans up all scheduler resources: the pending-future inc_refs and
    /// every remaining task (via [`Scheduler::cancel_task`]).
    pub fn cleanup(&mut self, heap: &mut HeapReader<'_, impl ResourceTracker>) {
        // Release the inc_refs the scheduler holds on each pending future.
        for (_, future_id) in mem::take(&mut self.pending_externals) {
            heap.dec_ref(future_id);
        }
        let task_ids: Vec<TaskId> = self.tasks.keys().copied().collect();
        for task_id in task_ids {
            self.cancel_task(task_id, heap);
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
