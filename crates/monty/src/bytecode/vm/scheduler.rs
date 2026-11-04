//! Task scheduler for async execution and call ID allocation.
//!
//! # Task Model
//!
//! The current task's context lives in the VM; all other contexts live in their tasks.
//! Task 0 is the main task. Spawned tasks (1+) deliver results to their gather or complete
//! their Python-visible task handle.

use std::{collections::VecDeque, mem};

use ahash::AHashMap;
use smallvec::{SmallVec, smallvec};

use super::{FrameNamespace, exceptions::CaughtException};
use crate::{
    asyncio::{Awaiter, CallId, TaskId},
    exception_private::RunResult,
    heap::{ContainsHeap, DropWithContext, Heap, HeapId, HeapReadOutput, HeapReader},
    intern::{FunctionId, StringId},
    value::Value,
};

/// Live tasks are runnable or blocked; completion removes the task from the scheduler.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum TaskState {
    /// Queued or currently executing. `Err` must be raised before executing more bytecode.
    Ready(RunResult<()>),
    /// Owns an inc_ref on the awaitable; its awaiter identifies the task to wake.
    Blocked(HeapId),
}

impl<C: ContainsHeap> DropWithContext<C> for TaskState {
    fn drop_with(self, heap: &mut C) {
        match self {
            Self::Ready(_) => {}
            Self::Blocked(id) => heap.heap_mut().dec_ref(id),
        }
    }
}

/// A live async task. Its execution context is saved here while another task runs.
/// The current task's frames and stacks live in the VM, including for the main task.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Task {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Saved frames; empty while this task is current or has not started.
    pub frames: Vec<SerializedTaskFrame>,
    /// Saved operand stack; the current task uses the VM's stack.
    pub stack: Vec<Value>,
    /// Exception stack for nested except blocks.
    pub exception_stack: Vec<CaughtException>,
    /// VM-level instruction_ip (for exception table lookup).
    pub instruction_ip: usize,
    /// Owned coroutine reference for a spawned task; the main task has none.
    pub coroutine_id: Option<HeapId>,
    /// Result recipient, owning an inc_ref for a gather. An already-settled gather
    /// discards the result. The main task has no awaiter.
    pub awaiter: Option<Awaiter>,
    /// Python-visible `asyncio.Task` handle for a task created by
    /// `asyncio.create_task`, owned by the scheduler until it completes, fails or is cancelled.
    #[serde(default)]
    pub python_handle: Option<HeapId>,
    /// Current execution state.
    state: TaskState,
}

impl<C: ContainsHeap> DropWithContext<C> for Task {
    fn drop_with(mut self, heap: &mut C) {
        self.stack.drain(..).drop_with(heap);
        self.exception_stack.drain(..).drop_with(heap);
        for frame in self.frames.drain(..) {
            frame.namespace.drop_with(heap);
        }
        self.state.drop_with(heap);
        if let Some(coro_id) = self.coroutine_id.take() {
            heap.heap_mut().dec_ref(coro_id);
        }
        if let Some(awaiter) = self.awaiter.take() {
            awaiter.drop_with(heap);
        }
        if let Some(handle) = self.python_handle.take() {
            heap.heap_mut().dec_ref(handle);
        }
    }
}

/// Serialized call frame for task storage.
///
/// Similar to `SerializedFrame` but used within the scheduler for task context.
/// Cannot store `&Code` references - uses `FunctionId` to look up code on resume.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
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
    /// Caller's bytecode offset at the call site (for tracebacks). See
    /// `CallFrame.call_offset`.
    pub call_offset: Option<u32>,
    /// Whether this frame is a class `__init__` or an imported module body (see `CallFrame.is_initializer`).
    pub is_initializer: bool,
    /// Frame namespace, owning its dict references (see `CallFrame.namespace`).
    pub namespace: Option<Box<FrameNamespace>>,
    /// The `from ... import` in progress, if the frame parked inside one.
    #[serde(default)]
    pub import_from_module: Option<StringId>,
}

impl Task {
    /// Creates a runnable task, taking ownership of its coroutine and awaiter references.
    pub fn new(id: TaskId, coroutine_id: Option<HeapId>, awaiter: Option<Awaiter>) -> Self {
        Self {
            id,
            frames: Vec::new(),
            stack: Vec::new(),
            exception_stack: Vec::new(),
            instruction_ip: 0,
            coroutine_id,
            awaiter,
            python_handle: None,
            state: TaskState::Ready(Ok(())),
        }
    }
}

/// Owns live tasks, pending external futures, and call IDs for both sync and async execution.
/// A blocked task is queued once when its awaitable settles, for either a value or an error.
/// Selecting that task consumes its queue entry and resume result before bytecode can run.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Scheduler {
    /// All tasks keyed by their `TaskId`.
    tasks: AHashMap<TaskId, Task>,
    /// Tasks ready to execute or raise a pending exception.
    ready_queue: ReadyQueue,
    /// Task whose context is in the VM, or `None` when that context has been discarded.
    current_task: Option<TaskId>,
    /// Counter for generating new task IDs.
    next_task_id: u32,
    /// Counter for automatic Task names. CPython's running task is `Task-1`,
    /// so the first user-created task is `Task-2`.
    #[serde(default = "default_next_task_name")]
    next_task_name: u32,
    /// Counter for external call IDs (always incremented, even for sync resolution).
    next_call_id: u32,
    /// Host-side index mapping each unresolved external call to its
    /// `HeapData::ExternalFuture` entry. The scheduler holds an inc_ref on
    /// each value until the host resolves or fails the call — that ref keeps
    /// the future entry alive between yield and resume even if no awaiter is
    /// holding a `Value::Ref` to it.
    pending_externals: AHashMap<CallId, HeapId>,
    /// Index mapping a spawned coroutine's `HeapId` to the `TaskId` driving
    /// it. Populated by [`Scheduler::spawn_gather_child`] and
    /// [`Scheduler::spawn_python_task`], and removed in
    /// [`Scheduler::cancel_task`]. Lets `GatherFuture` and other call sites
    /// dispatch from coroutine heap id back to the driving task without
    /// scanning all tasks.
    coroutine_to_task: AHashMap<HeapId, TaskId>,
}

impl Scheduler {
    /// Creates the main task as current; it needs no queue entry to start executing.
    pub fn new() -> Self {
        let main_task_id = TaskId::default();
        let main_task = Task::new(main_task_id, None, None);
        let mut tasks = AHashMap::new();
        tasks.insert(main_task_id, main_task);
        Self {
            tasks,
            ready_queue: ReadyQueue::default(), // Main task is current, not in ready queue
            current_task: Some(main_task_id),
            next_task_id: 1,
            next_task_name: default_next_task_name(),
            next_call_id: 0,
            pending_externals: AHashMap::new(),
            coroutine_to_task: AHashMap::new(),
        }
    }

    /// Identifies the VM's loaded context, even when that task is blocked on a host call.
    #[inline]
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.current_task
    }

    /// Whether awaiting the current call can proceed without delaying other work.
    pub fn can_await_eagerly(&self) -> bool {
        self.ready_queue.is_empty() && self.pending_externals.is_empty()
    }

    /// Returns the main task, whose context is saved here whenever another
    /// task is loaded; `None` once it has been cancelled.
    pub fn main_task(&self) -> Option<&Task> {
        self.tasks.get(&TaskId::default())
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

    /// Allocates the next automatic `Task-N` name. The running task is
    /// `Task-1`, so the first user-created task is `Task-2`.
    pub fn allocate_task_name(&mut self) -> String {
        let name = format!("Task-{}", self.next_task_name);
        self.next_task_name += 1;
        name
    }

    /// Registers a freshly created `ExternalFuture` for `call_id`.
    ///
    /// The scheduler inc_refs `future_id` so the entry stays alive between
    /// the yield to the host and the matching `resolve_future` / `fail_future`
    /// call, even if no awaiter holds a `Value::Ref` to it.
    pub fn add_pending_external(&mut self, call_id: CallId, future_id: HeapId, heap: &Heap) {
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
    pub fn block_current_on(&mut self, awaitable_id: HeapId, heap: &Heap) {
        if let Some(task_id) = self.current_task {
            let task = self.get_task_mut(task_id);
            debug_assert!(matches!(task.state, TaskState::Ready(Ok(()))));
            heap.inc_ref(awaitable_id);
            task.state = TaskState::Blocked(awaitable_id);
        }
    }

    /// Returns all pending (unresolved) CallIds.
    pub fn pending_call_ids(&self) -> Vec<CallId> {
        self.pending_externals.keys().copied().collect()
    }

    /// Removes the queue entry when delivering directly to an exiting task's waiter.
    ///
    /// O(1): the slot is left stale and skipped when it reaches the queue front.
    pub fn remove_from_ready_queue(&mut self, task_id: TaskId) {
        self.ready_queue.remove(task_id);
    }

    /// Spawns a task that reports its result to `gather`, enforcing one-task-per-coroutine.
    ///
    /// Returns `None` if `coroutine_id` is already driving a task — caught here
    /// because cross-gather reuse can hit two spawns while both coroutine states
    /// are still `New`, so the state check in `await_coroutine` doesn't catch it.
    /// Callers translate `None` into `RuntimeError: cannot reuse already awaited coroutine`.
    /// `coroutine_id` and `gather_id` become **owning** references held by the
    /// new task; the matching `dec_ref` happens in [`Scheduler::cancel_task`].
    pub fn spawn_gather_child(&mut self, heap: &Heap, coroutine_id: HeapId, gather_id: HeapId) -> Option<TaskId> {
        self.spawn_inner(heap, coroutine_id, SpawnTarget::Gather(gather_id))
    }

    /// Spawns the task behind a Python-visible `asyncio.Task` handle.
    ///
    /// `coroutine_id` and `handle` become **owning** references held by the new
    /// task; the matching `dec_ref` happens in [`Scheduler::cancel_task`].
    pub fn spawn_python_task(&mut self, heap: &Heap, coroutine_id: HeapId, handle: HeapId) -> Option<TaskId> {
        self.spawn_inner(heap, coroutine_id, SpawnTarget::Handle(handle))
    }

    /// Shared construction for both spawn targets, after the duplicate check.
    fn spawn_inner(&mut self, heap: &Heap, coroutine_id: HeapId, target: SpawnTarget) -> Option<TaskId> {
        if self.coroutine_to_task.contains_key(&coroutine_id) {
            return None;
        }

        let task_id = TaskId::new(self.next_task_id);
        self.next_task_id += 1;

        // Take ownership of the heap references — the task now holds an inc_ref'd
        // pointer to its coroutine and to its wake-up target. The slot is keyed
        // by the coroutine's own id, which is what `resolve_child` looks up.
        heap.inc_ref(coroutine_id);
        let (awaiter, python_handle) = match target {
            SpawnTarget::Gather(gather) => {
                heap.inc_ref(gather);
                (
                    Some(Awaiter::GatherSlot {
                        gather,
                        source: coroutine_id,
                    }),
                    None,
                )
            }
            SpawnTarget::Handle(handle) => {
                heap.inc_ref(handle);
                (None, Some(handle))
            }
        };

        let mut task = Task::new(task_id, Some(coroutine_id), awaiter);
        task.python_handle = python_handle;

        self.tasks.insert(task_id, task);
        self.coroutine_to_task.insert(coroutine_id, task_id);
        self.ready_queue.push_back(task_id);

        Some(task_id)
    }

    /// Gets the next ready task from the queue.
    ///
    /// Stale entries are discarded on the way; returns `None` if no tasks are ready.
    pub fn next_ready_task(&mut self) -> Option<TaskId> {
        self.ready_queue.pop_front()
    }

    /// Requeues the current task so another ready task can take the next turn.
    ///
    /// The current task stays runnable; its context is saved when the next task
    /// activates. Used after an asynchronous host wait, where the waiting task
    /// is ready again but should not starve tasks created while it waited.
    pub fn requeue_current(&mut self) {
        if let Some(task_id) = self.current_task {
            self.ready_queue.push_back(task_id);
        }
    }

    /// Wakes a blocked task with a value already on its stack, or an exception to raise.
    pub fn make_ready(&mut self, task_id: TaskId, result: RunResult<()>, heap: &mut Heap) {
        let task = self.get_task_mut(task_id);
        debug_assert!(matches!(task.state, TaskState::Blocked(_)));
        let old_state = mem::replace(&mut task.state, TaskState::Ready(result));
        old_state.drop_with(heap);
        self.ready_queue.push_back(task_id);
    }

    /// Takes the selected task's resume result exactly once, before executing its context.
    pub fn take_resume_result(&mut self, task_id: TaskId) -> RunResult<()> {
        match &mut self.get_task_mut(task_id).state {
            TaskState::Ready(result) => mem::replace(result, Ok(())),
            TaskState::Blocked(_) => panic!("cannot resume a blocked task"),
        }
    }

    /// Sets the current task.
    pub fn set_current_task(&mut self, task_id: Option<TaskId>) {
        self.current_task = task_id;
    }

    /// Removes a task and releases its saved context and heap references.
    /// Cancels tasks under any gather it is still blocked on, using an iterative walk
    /// so deeply nested gathers cannot overflow the native stack during teardown.
    pub fn cancel_task(&mut self, task_id: TaskId, heap: &mut HeapReader<'_>) {
        let mut pending: SmallVec<[TaskId; 4]> = smallvec![task_id];
        while let Some(task_id) = pending.pop() {
            self.cancel_one(task_id, heap, &mut pending);
        }
    }

    /// Cancels one task, queueing the tasks spawned under any gather it was
    /// blocked on for [`Scheduler::cancel_task`] to drain.
    ///
    /// Dropping this task ahead of the children it queued is sound: each owns
    /// an inc_ref on that same gather (see [`Scheduler::spawn_gather_child`]).
    fn cancel_one(&mut self, task_id: TaskId, heap: &mut HeapReader<'_>, pending: &mut SmallVec<[TaskId; 4]>) {
        // No-op if the task has already been removed (idempotent — finalization
        // sites may iterate task ids that include already-cancelled siblings).
        let Some(task) = self.tasks.remove(&task_id) else {
            return;
        };

        // The VM must discard this task's loaded context before activating another task.
        if self.current_task == Some(task_id) {
            self.current_task = None;
        }

        if let Some(coroutine_id) = task.coroutine_id {
            self.coroutine_to_task.remove(&coroutine_id);
        }

        self.ready_queue.remove(task_id);

        // Blocked on a gather: queue the tasks spawned under it. An
        // external future needs no extra teardown.
        if let TaskState::Blocked(blocked_id) = task.state {
            self.queue_gather_tasks(blocked_id, heap, pending);
        }

        task.drop_with(heap);
    }

    /// Queues every task spawned under the gather `root`, walking nested
    /// gathers iteratively.
    ///
    /// A gather item can itself be a gather (`gather(gather(coro()))`), whose
    /// tasks are just as orphaned as direct coroutine children if left in
    /// `self.tasks` — they would keep running and then deliver a result to the
    /// task cancelled here. External children *are* left alone: the owning
    /// `Awaiter::GatherSlot` anchors them.
    ///
    /// Must run while the cancelled task still holds its `Blocked` inc_ref on
    /// `root`, since the walk takes no references of its own: each nested
    /// gather is kept alive by its parent's `items`, and the parent in turn by
    /// the `Awaiter::GatherSlot` inc_ref that nested child holds.
    fn queue_gather_tasks(&self, root: HeapId, heap: &HeapReader<'_>, pending: &mut SmallVec<[TaskId; 4]>) {
        // Gathers nest as a tree — a gather may only be awaited once, so the
        // walk cannot revisit a node and terminates.
        let mut gathers: SmallVec<[HeapId; 4]> = smallvec![root];
        while let Some(gather_id) = gathers.pop() {
            // Coroutine and external children land here too; only gathers have
            // children of their own to walk.
            let HeapReadOutput::GatherFuture(gather) = heap.read(gather_id) else {
                continue;
            };
            if let Some(awaited) = gather.get(heap).as_awaited() {
                for child_id in awaited.pending_children.keys() {
                    match self.coroutine_to_task.get(child_id) {
                        Some(&task_id) => pending.push(task_id),
                        None => gathers.push(*child_id),
                    }
                }
            }
            drop(gather);
        }
    }

    /// Whether this task still exists and is waiting for an awaitable to settle.
    #[inline]
    pub fn is_blocked(&self, task_id: TaskId) -> bool {
        self.tasks
            .get(&task_id)
            .is_some_and(|task| matches!(task.state, TaskState::Blocked(_)))
    }

    /// Number of tasks the scheduler still holds.
    ///
    /// Test-only: a task whose await has already failed must not stay parked,
    /// and a finished run tears the scheduler down, so this count while
    /// suspended is the only way a test can see one.
    #[cfg(feature = "test-hooks")]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Cleans up all scheduler resources: the pending-future inc_refs and
    /// every remaining task (via [`Scheduler::cancel_task`]).
    pub fn cleanup(&mut self, heap: &mut HeapReader<'_>) {
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

/// Default for scheduler dumps written before automatic Task names existed.
fn default_next_task_name() -> u32 {
    2
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Wake-up target of a task spawned from a coroutine.
///
/// The two variants own different references; passing them separately keeps a
/// mistyped swap from compiling.
#[derive(Clone, Copy)]
enum SpawnTarget {
    /// Report completion through a slot in this gather.
    Gather(HeapId),
    /// Complete this Python-visible `asyncio.Task` handle.
    Handle(HeapId),
}

/// FIFO of runnable tasks with O(1) removal.
///
/// Removal drops the task from `live`; its slot is skipped when it reaches the
/// front. Pushing an already-live task gives it a newer generation, moving it to
/// the back. Dumps write only live entries, keeping the wire shape a list of ids.
#[derive(Debug, Default)]
struct ReadyQueue {
    /// Slots in FIFO order; a slot is live only while `live` holds its generation.
    entries: VecDeque<(TaskId, u64)>,
    /// Generation of each queued task's live slot.
    live: AHashMap<TaskId, u64>,
    /// Source of slot generations.
    next_generation: u64,
}

impl ReadyQueue {
    /// Appends a live slot, invalidating any earlier slot for the same task.
    fn push_back(&mut self, task_id: TaskId) {
        let generation = self.next_generation;

        self.next_generation = self.next_generation.wrapping_add(1);
        self.live.insert(task_id, generation);
        self.entries.push_back((task_id, generation));
    }

    /// Drops the task's live slot in O(1); the stale entry is skipped on pop.
    fn remove(&mut self, task_id: TaskId) {
        self.live.remove(&task_id);
    }

    /// Pops the next live task, discarding stale entries.
    fn pop_front(&mut self) -> Option<TaskId> {
        while let Some((task_id, generation)) = self.entries.pop_front() {
            if self.live.get(&task_id) == Some(&generation) {
                self.live.remove(&task_id);
                return Some(task_id);
            }
        }

        None
    }

    /// Whether any task is ready; stale entries do not count.
    fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
}

impl serde::Serialize for ReadyQueue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Collect first so the sequence has an exact length, matching the previous
        // `VecDeque<TaskId>` encoding.
        let ids: Vec<TaskId> = self
            .entries
            .iter()
            .filter(|&&(task_id, generation)| self.live.get(&task_id) == Some(&generation))
            .map(|&(task_id, _)| task_id)
            .collect();
        serializer.collect_seq(ids)
    }
}

impl<'de> serde::Deserialize<'de> for ReadyQueue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let ids = <Vec<TaskId> as serde::Deserialize>::deserialize(deserializer)?;
        let mut queue = Self::default();

        for task_id in ids {
            queue.push_back(task_id);
        }

        Ok(queue)
    }
}
