//! Bytecode virtual machine for executing compiled Python code.
//!
//! The VM uses a stack-based execution model with an operand stack for computation
//! and a call stack for function frames. Each frame owns its instruction pointer (IP).

mod async_exec;
mod attr;
mod binary;
mod call;
mod collections;
mod compare;
mod exceptions;
mod format;
mod scheduler;

use std::cmp::Ordering;

pub(crate) use call::CallResult;
use scheduler::Scheduler;

use crate::{
    MontyObject,
    args::ArgValues,
    asyncio::{CallId, TaskId},
    bytecode::{code::Code, op::Opcode},
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithHeap, Heap, HeapData, HeapGuard, HeapId},
    heap_data::{Closure, FunctionDefaults, HeapDataMut},
    intern::{FunctionId, Interns, StringId},
    io::PrintWriter,
    modules::BuiltinModule,
    os::OsFunction,
    parse::CodeRange,
    resource::ResourceTracker,
    types::{LongInt, MontyIter, PyTrait, iter::advance_on_heap},
    value::{BitwiseOp, EitherStr, Value},
};

/// Result of executing Await opcode.
///
/// Indicates what the VM should do after awaiting a value:
/// - `ValueReady`: the awaited value resolved immediately, push it
/// - `FramePushed`: a new frame was pushed for coroutine execution
/// - `Yield`: all tasks blocked, yield to caller with pending futures
enum AwaitResult {
    /// The awaited value resolved immediately (e.g., resolved ExternalFuture).
    ValueReady(Value),
    /// A new frame was pushed to execute a coroutine.
    FramePushed,
    /// All tasks are blocked - yield to caller with pending futures.
    Yield(Vec<CallId>),
}

/// Tries an operation and handles exceptions, reloading cached frame state.
///
/// Use this in the main run loop where `cached_frame`
/// are used. After catching an exception, reloads the cache since the handler
/// may be in a different frame.
macro_rules! try_catch_sync {
    ($self:expr, $cached_frame:ident, $expr:expr) => {
        if let Err(e) = $expr {
            if let Some(result) = $self.handle_exception(e) {
                return Err(result);
            }
            // Exception was caught - handler may be in different frame, reload cache
            reload_cache!($self, $cached_frame);
        }
    };
}

/// Handles an exception and reloads cached frame state if caught.
///
/// Use this in the main run loop where `cached_frame`
/// are used. After catching an exception, reloads the cache since the handler
/// may be in a different frame.
///
/// Wrapped in a block to allow use in match arm expressions.
macro_rules! catch_sync {
    ($self:expr, $cached_frame:ident, $err:expr) => {{
        if let Some(result) = $self.handle_exception($err) {
            return Err(result);
        }
        // Exception was caught - handler may be in different frame, reload cache
        reload_cache!($self, $cached_frame);
    }};
}

/// Fetches a byte from bytecode using cached code/ip, advancing ip.
///
/// Used in the run loop for fast operand fetching without frame access.
macro_rules! fetch_byte {
    ($cached_frame:expr) => {{
        let byte = $cached_frame.code.bytecode()[$cached_frame.ip];
        $cached_frame.ip += 1;
        byte
    }};
}

/// Fetches a u8 operand using cached code/ip.
macro_rules! fetch_u8 {
    ($cached_frame:expr) => {
        fetch_byte!($cached_frame)
    };
}

/// Fetches an i8 operand using cached code/ip.
macro_rules! fetch_i8 {
    ($cached_frame:expr) => {{ i8::from_ne_bytes([fetch_byte!($cached_frame)]) }};
}

/// Fetches a u16 operand (little-endian) using cached code/ip.
macro_rules! fetch_u16 {
    ($cached_frame:expr) => {{
        let lo = $cached_frame.code.bytecode()[$cached_frame.ip];
        let hi = $cached_frame.code.bytecode()[$cached_frame.ip + 1];
        $cached_frame.ip += 2;
        u16::from_le_bytes([lo, hi])
    }};
}

/// Fetches an i16 operand (little-endian) using cached code/ip.
macro_rules! fetch_i16 {
    ($cached_frame:expr) => {{
        let lo = $cached_frame.code.bytecode()[$cached_frame.ip];
        let hi = $cached_frame.code.bytecode()[$cached_frame.ip + 1];
        $cached_frame.ip += 2;
        i16::from_le_bytes([lo, hi])
    }};
}

/// Reloads cached frame state from the current frame.
///
/// Call this after any operation that modifies the frame stack (calls, returns,
/// exception handling).
macro_rules! reload_cache {
    ($self:expr, $cached_frame:ident) => {{
        $cached_frame = $self.new_cached_frame();
    }};
}

/// Applies a relative jump offset to the cached IP.
///
/// Uses checked arithmetic to safely compute the new IP, panicking if the
/// jump would result in a negative or overflowing instruction pointer.
macro_rules! jump_relative {
    ($ip:expr, $offset:expr) => {{
        let ip_i64 = i64::try_from($ip).expect("instruction pointer exceeds i64");
        let new_ip = ip_i64 + i64::from($offset);
        $ip = usize::try_from(new_ip).expect("jump resulted in negative or overflowing IP");
    }};
}

/// Handles the result of a load operation that may yield a `FrameExit::NameLookup`.
///
/// `load_local` and `load_global` return `Result<Option<FrameExit>, RunError>`:
/// - `Ok(None)`: load succeeded, value is on the stack
/// - `Ok(Some(FrameExit::NameLookup { .. }))`: unresolved name, yield to host
/// - `Err(e)`: exception (e.g., UnboundLocalError)
macro_rules! handle_load_result {
    ($self:expr, $cached_frame:ident, $result:expr) => {
        match $result {
            Ok(None) => {}
            Ok(Some(frame_exit)) => {
                $self.current_frame_mut().ip = $cached_frame.ip;
                return Ok(frame_exit);
            }
            Err(e) => catch_sync!($self, $cached_frame, e),
        }
    };
}

/// Handles the result of a call operation that returns `CallResult`.
///
/// This macro eliminates the repetitive pattern of matching on `CallResult`
/// variants that appears in LoadAttr, CallFunction, CallFunctionKw, CallAttr,
/// CallAttrKw, and CallFunctionExtended opcodes.
///
/// Actions taken for each variant:
/// - `Push(value)`: Push the value onto the stack
/// - `FramePushed`: Reload the cached frame (a new frame was pushed)
/// - `External(ext_id, args)`: Return `FrameExit::ExternalCall` to yield to host
/// - `OsCall(func, args)`: Return `FrameExit::OsCall` to yield to host
/// - `MethodCall(name, args)`: Return `FrameExit::MethodCall` to yield to host
/// - `AwaitValue(value)`: Push value, then implicitly await it via `exec_get_awaitable`
/// - `Err(err)`: Handle the exception via `catch_sync!`
macro_rules! handle_call_result {
    ($self:expr, $cached_frame:ident, $result:expr) => {
        match $result {
            Ok(CallResult::Value(result)) => $self.push(result),
            Ok(CallResult::FramePushed) => reload_cache!($self, $cached_frame),
            Ok(CallResult::External(name, args)) => {
                let call_id = $self.allocate_call_id();
                let name_load_ip = $self.ext_function_load_ip.take();
                // Sync cached IP back to frame before snapshot for resume
                $self.current_frame_mut().ip = $cached_frame.ip;
                return Ok(FrameExit::ExternalCall {
                    function_name: name,
                    args,
                    call_id,
                    name_load_ip,
                });
            }
            Ok(CallResult::OsCall(func, args)) => {
                let call_id = $self.allocate_call_id();
                // Sync cached IP back to frame before snapshot for resume
                $self.current_frame_mut().ip = $cached_frame.ip;
                return Ok(FrameExit::OsCall {
                    function: func,
                    args,
                    call_id,
                });
            }
            Ok(CallResult::MethodCall(method_name, args)) => {
                let call_id = $self.allocate_call_id();
                // Sync cached IP back to frame before snapshot for resume
                $self.current_frame_mut().ip = $cached_frame.ip;
                return Ok(FrameExit::MethodCall {
                    method_name,
                    args,
                    call_id,
                });
            }
            Ok(CallResult::AwaitValue(value)) => {
                // Push the value and implicitly await it (used by asyncio.run())
                $self.push(value);
                $self.current_frame_mut().ip = $cached_frame.ip;
                match $self.exec_get_awaitable() {
                    Ok(AwaitResult::ValueReady(value)) => {
                        $self.push(value);
                    }
                    Ok(AwaitResult::FramePushed) => {
                        reload_cache!($self, $cached_frame);
                    }
                    Ok(AwaitResult::Yield(pending_calls)) => {
                        return Ok(FrameExit::ResolveFutures(pending_calls));
                    }
                    Err(e) => {
                        catch_sync!($self, $cached_frame, e);
                    }
                }
            }
            Err(err) => catch_sync!($self, $cached_frame, err),
        }
    };
}

/// Result of VM execution.
pub enum FrameExit {
    /// Execution completed successfully with a return value.
    Return(Value),

    /// Execution paused for an external function call.
    ///
    /// The caller should execute the external function and call `resume()`
    /// with the result. The `call_id` allows the host to use async resolution
    /// by calling `run_pending()` instead of `run(result)`.
    ExternalCall {
        /// Name of the external function to call (interned or heap-owned).
        function_name: EitherStr,
        /// Arguments for the external function (includes both positional and keyword args).
        args: ArgValues,
        /// Unique ID for this call, used for async correlation.
        call_id: CallId,
        /// Optional bytecode IP of the load instruction that produced this `ExtFunction`.
        ///
        /// When a `LoadGlobalCallable`/`LoadLocalCallable` opcode auto-injects an `ExtFunction`
        /// for an undefined name, the load instruction's IP is saved here. In standard execution
        /// (without external function support), this IP is used to restore the frame pointer
        /// before raising `NameError`, so the traceback points to the name rather than the call.
        name_load_ip: Option<usize>,
    },

    /// Execution paused for an os function call.
    ///
    /// The caller should execute a function corresponding to the `os_call` and call `resume()`
    /// with the result. The `call_id` allows the host to use async resolution
    /// by calling `run_pending()` instead of `run(result)`.
    OsCall {
        /// ID of the os function to call.
        function: OsFunction,
        /// Arguments for the external function (includes both positional and keyword args).
        args: ArgValues,
        /// Unique ID for this call, used for async correlation.
        call_id: CallId,
    },

    /// Execution paused for a dataclass method call.
    ///
    /// The caller should invoke the method on the original Python dataclass and call
    /// `resume()` with the result. The `method_name` is the attribute name (e.g.
    /// `"distance"`) and `args` includes the dataclass instance as the first argument
    /// (`self`).
    MethodCall {
        /// Method name (e.g., "distance").
        method_name: EitherStr,
        /// Arguments including the dataclass instance as the first positional arg.
        args: ArgValues,
        /// Unique ID for this call, used for async correlation.
        call_id: CallId,
    },

    /// All tasks are blocked waiting for external futures to resolve.
    ///
    /// The caller must resolve the pending CallIds before calling `resume()`.
    /// This happens when await is called on an ExternalFuture that hasn't
    /// been resolved yet, and there are no other ready tasks to switch to.
    ResolveFutures(Vec<CallId>),

    /// Execution paused for an unresolved name lookup.
    ///
    /// When the VM encounters an `Undefined` value in a `LocalUnassigned` slot
    /// (module level) or a global slot, it yields to the host to resolve the name.
    /// The host can return a value to cache in the slot, or indicate the name is
    /// truly undefined (which will raise `NameError`).
    ///
    /// This enables auto-detection of external functions without requiring upfront
    /// declaration: unresolved names are lazily resolved by the host at runtime.
    NameLookup {
        /// The interned name being looked up.
        name_id: StringId,
        /// The namespace slot where the resolved value should be cached.
        namespace_slot: u16,
        /// Whether this is a global slot (true) or a local/function slot (false).
        is_global: bool,
    },
}

/// A single function activation record.
///
/// Each frame represents one level in the call stack and owns its own
/// instruction pointer. This design avoids sync bugs on call/return.
#[derive(Debug)]
pub struct CallFrame<'code> {
    /// Bytecode being executed.
    code: &'code Code,

    /// Instruction pointer within this frame's bytecode.
    ip: usize,

    /// Base index into the VM stack for this frame's locals region.
    ///
    /// The frame's locals occupy `stack[stack_base..stack_base + locals_count]`,
    /// and operands are pushed above that.
    stack_base: usize,

    /// Number of local variable slots in this frame.
    ///
    /// Zero for module-level frames (globals are stored separately).
    /// For function frames, this equals `func.namespace_size`.
    locals_count: u16,

    /// Function ID (for tracebacks). None for module-level code.
    function_id: Option<FunctionId>,

    /// Call site position (for tracebacks).
    call_position: Option<CodeRange>,

    /// When this frame returns (or exits with an exception) the VM should exit the run loop
    /// and return to the caller. Supports `evaluate_function`.
    should_return: bool,
}

impl<'code> CallFrame<'code> {
    /// Creates a new call frame for module-level code.
    ///
    /// Module frames have `locals_count = 0` because module-level variables
    /// are stored in the VM's `globals` vec, not in the stack.
    pub fn new_module(code: &'code Code) -> Self {
        Self {
            code,
            ip: 0,
            stack_base: 0,
            locals_count: 0,
            function_id: None,
            call_position: None,
            should_return: false,
        }
    }

    /// Creates a new call frame for a function call.
    ///
    /// The frame's locals occupy `stack[stack_base..stack_base + locals_count]`.
    /// Operands are pushed above the locals region.
    pub fn new_function(
        code: &'code Code,
        stack_base: usize,
        locals_count: u16,
        function_id: FunctionId,
        call_position: Option<CodeRange>,
    ) -> Self {
        Self {
            code,
            ip: 0,
            stack_base,
            locals_count,
            function_id: Some(function_id),
            call_position,
            should_return: false,
        }
    }
}

/// Cached state of the VM derived from the current frame as an optimization.
///
/// Holds the hot fields from the current `CallFrame` to avoid repeated
/// `frames.last()` lookups in the main opcode loop.
#[derive(Debug, Copy, Clone)]
pub struct CachedFrame<'code> {
    /// Bytecode being executed.
    code: &'code Code,

    /// Instruction pointer within this frame's bytecode.
    ip: usize,

    /// Base index into the VM stack for this frame's locals.
    stack_base: usize,
}

impl<'code> From<&CallFrame<'code>> for CachedFrame<'code> {
    fn from(frame: &CallFrame<'code>) -> Self {
        Self {
            code: frame.code,
            ip: frame.ip,
            stack_base: frame.stack_base,
        }
    }
}

/// Serializable representation of a call frame.
///
/// Cannot store `&Code` (a reference) — instead stores `FunctionId` to look up
/// the pre-compiled Code object on resume. Module-level code uses `None`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedFrame {
    /// Which function's code this frame executes (None = module-level).
    function_id: Option<FunctionId>,

    /// Instruction pointer within this frame's bytecode.
    ip: usize,

    /// Base index into the VM stack for this frame's locals region.
    stack_base: usize,

    /// Number of local variable slots (0 for module-level frames).
    locals_count: u16,

    /// Call site position (for tracebacks).
    call_position: Option<CodeRange>,
}

impl CallFrame<'_> {
    /// Converts this frame to a serializable representation.
    fn serialize(&self) -> SerializedFrame {
        assert!(
            !self.should_return,
            "cannot serialize frame marked for return - not yet supported"
        );
        SerializedFrame {
            function_id: self.function_id,
            ip: self.ip,
            stack_base: self.stack_base,
            locals_count: self.locals_count,
            call_position: self.call_position,
        }
    }
}

/// VM state for pause/resume at external function calls.
///
/// **Ownership:** This struct OWNS the values (refcounts were already incremented).
/// Must be used with the serialized Heap - HeapId values are indices into that heap.
///
/// **Usage:** When the VM pauses for an external call, call `into_snapshot()` to
/// create this snapshot. The snapshot can be serialized and stored. On resume,
/// use `restore()` to reconstruct the VM and continue execution.
///
/// Note: This struct does not implement `Clone` because `Value` uses manual
/// reference counting. Snapshots transfer ownership - they are not copied.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct VMSnapshot {
    /// Operand stack — locals and operands interleaved per frame.
    ///
    /// Each function frame's locals occupy `stack[frame.stack_base..frame.stack_base + frame.locals_count]`,
    /// with operands pushed above.
    pub(crate) stack: Vec<Value>,

    /// Module-level (global) variable storage.
    pub(crate) globals: Vec<Value>,

    /// Call frames (serializable form — stores FunctionId, not &Code).
    frames: Vec<SerializedFrame>,

    /// Stack of exceptions being handled for nested except blocks.
    ///
    /// When entering an except handler, the exception is pushed onto this stack.
    /// When exiting via `ClearException`, the top is popped. This allows nested
    /// except handlers to restore the outer exception context.
    exception_stack: Vec<Value>,

    /// IP of the instruction that caused the pause (for exception handling).
    instruction_ip: usize,

    /// Scheduler state (always present).
    ///
    /// Contains call ID counter, task state, pending calls, and resolved futures.
    scheduler: Scheduler,
}

// ============================================================================
// Virtual Machine
// ============================================================================

/// The bytecode virtual machine.
///
/// Executes compiled bytecode using a stack-based execution model.
/// The instruction pointer (IP) lives in each `CallFrame`, not here,
/// to avoid sync bugs on call/return.
///
/// # Lifetimes
/// * `'a` - Lifetime of the heap, namespaces, and interns
/// * `'p` - Lifetime of the print writer's internal references
pub struct VM<'a, 'p, T: ResourceTracker> {
    /// Operand stack — locals and operands interleaved per frame.
    ///
    /// Each function frame's locals occupy `stack[frame.stack_base..frame.stack_base + frame.locals_count]`,
    /// with operands pushed above. Module-level frames have `locals_count = 0`
    /// because globals are stored separately.
    pub(crate) stack: Vec<Value>,

    /// Module-level (global) variable storage.
    ///
    /// Indexed by slot number from `LoadGlobal`/`StoreGlobal` opcodes.
    /// Separated from the stack because globals persist across function calls
    /// and are accessed via dedicated opcodes.
    pub(crate) globals: Vec<Value>,

    /// Call stack — function frames (each frame has its own IP).
    frames: Vec<CallFrame<'a>>,

    /// Heap for reference-counted objects.
    pub(crate) heap: &'a mut Heap<T>,

    /// Interned strings/bytes.
    pub(crate) interns: &'a Interns,

    /// Print output writer, borrowed so callers retain access to collected output.
    pub(crate) print_writer: PrintWriter<'p>,

    /// Stack of exceptions being handled for nested except blocks.
    ///
    /// Used by bare `raise` to re-raise the current exception.
    /// When entering an except handler, the exception is pushed onto this stack.
    /// When exiting via `ClearException`, the top is popped. This allows nested
    /// except handlers to restore the outer exception context.
    exception_stack: Vec<Value>,

    /// IP of the instruction being executed (for exception table lookup).
    ///
    /// Updated at the start of each instruction before operands are fetched.
    /// This allows us to find the correct exception handler when an error occurs.
    instruction_ip: usize,

    /// Scheduler for task management and call ID allocation.
    ///
    /// Always present — owns `next_call_id` (used by both sync and async paths)
    /// plus async task state. Internal collections don't allocate until first use,
    /// so sync-only code pays only for the main task entry.
    scheduler: Scheduler,

    /// Module-level code (for restoring main task frames).
    ///
    /// Stored here because the main task's frames have `function_id: None` and
    /// need a reference to the module code when being restored after task switching.
    module_code: Option<&'a Code>,

    /// Bytecode IP of the most recent `LoadGlobalCallable`/`LoadLocalCallable` that
    /// pushed an `ExtFunction` for an undefined name.
    ///
    /// Used to restore the frame IP when standard execution converts an `ExternalCall`
    /// back to a `NameError`, so the traceback points to the name reference rather than
    /// the call expression.
    ext_function_load_ip: Option<usize>,
}

impl<'a, 'p, T: ResourceTracker> VM<'a, 'p, T> {
    /// Creates a new VM with the given runtime context.
    pub fn new(
        globals: Vec<Value>,
        heap: &'a mut Heap<T>,
        interns: &'a Interns,
        print_writer: PrintWriter<'p>,
    ) -> Self {
        Self {
            stack: Vec::with_capacity(64),
            globals,
            frames: Vec::with_capacity(16),
            heap,
            interns,
            print_writer,
            exception_stack: Vec::new(),
            instruction_ip: 0,
            scheduler: Scheduler::new(),
            ext_function_load_ip: None, // Set by LoadGlobalCallable/LoadLocalCallable
            module_code: None,
        }
    }

    /// Reconstructs a VM from a snapshot.
    ///
    /// The heap must already be deserialized. `FunctionId` values
    /// in frames are used to look up pre-compiled `Code` objects from the `Interns`.
    /// The `module_code` is used for frames with `function_id = None`.
    ///
    /// # Arguments
    /// * `snapshot` - The VM snapshot to restore
    /// * `module_code` - Compiled module code (for frames with function_id = None)
    /// * `heap` - The deserialized heap
    /// * `interns` - Interns for looking up function code
    /// * `print_writer` - Writer for print output
    pub fn restore(
        snapshot: VMSnapshot,
        module_code: &'a Code,
        heap: &'a mut Heap<T>,
        interns: &'a Interns,
        print_writer: PrintWriter<'p>,
    ) -> Self {
        // Reconstruct call frames from serialized form
        let frames: Vec<CallFrame<'_>> = snapshot
            .frames
            .into_iter()
            .map(|sf| {
                let code = match sf.function_id {
                    Some(func_id) => &interns.get_function(func_id).code,
                    None => module_code,
                };
                CallFrame {
                    code,
                    ip: sf.ip,
                    stack_base: sf.stack_base,
                    locals_count: sf.locals_count,
                    function_id: sf.function_id,
                    call_position: sf.call_position,
                    should_return: false,
                }
            })
            .collect();

        // Restore recursion depth to match the number of active function frames.
        // During serialization, recursion_depth is transient (defaults to 0),
        // but cleanup paths call decr_recursion_depth for each non-root frame.
        let current_frame_depth = frames.len().saturating_sub(1); // Subtract 1 for root frame which doesn't contribute to depth
        heap.set_recursion_depth(current_frame_depth);

        Self {
            stack: snapshot.stack,
            globals: snapshot.globals,
            frames,
            heap,
            interns,
            print_writer,
            exception_stack: snapshot.exception_stack,
            instruction_ip: snapshot.instruction_ip,
            scheduler: snapshot.scheduler,
            module_code: Some(module_code),
            ext_function_load_ip: None,
        }
    }
    /// Consumes the VM and creates a snapshot for pause/resume.
    ///
    /// **Ownership transfer:** This method takes `self` by value, consuming the VM.
    /// The snapshot owns all Values (refcounts already correct from the live VM).
    /// The heap and namespaces must be serialized alongside this snapshot.
    ///
    /// This is NOT a clone - it's a transfer. After calling this, the original VM
    /// is gone and only the snapshot (+ serialized heap/namespaces) represents the state.
    pub fn snapshot(self) -> VMSnapshot {
        VMSnapshot {
            // Move values directly — no clone, no refcount increment needed
            // (the VM owned them, now the snapshot owns them)
            stack: self.stack,
            globals: self.globals,
            frames: self.frames.into_iter().map(|f| f.serialize()).collect(),
            exception_stack: self.exception_stack,
            instruction_ip: self.instruction_ip,
            scheduler: self.scheduler,
        }
    }

    /// Pushes an initial frame for module-level code and runs the VM.
    pub fn run_module(&mut self, code: &'a Code) -> Result<FrameExit, RunError> {
        // Store module code for restoring main task frames during task switching
        self.module_code = Some(code);
        self.push_frame(CallFrame::new_module(code))?;
        self.run()
    }

    /// Cleans up VM state before the VM is dropped.
    ///
    /// This method must be called before the VM goes out of scope to ensure
    /// proper reference counting cleanup for any exception values and scheduler state.
    pub fn cleanup(&mut self) {
        // Drop all exceptions in the exception stack
        self.exception_stack.drain(..).drop_with_heap(self.heap);
        // Clean up current task's stack values and frame cell references
        self.cleanup_current_task();
        // Clean up scheduler state (task stacks, pending calls, resolved values, frame cells)
        self.scheduler.cleanup(self.heap);
        self.globals.drain(..).drop_with_heap(self.heap);
    }

    /// Returns the `stack_base` of the current (topmost) call frame.
    ///
    /// Used by `NameLookup` resolution to determine which stack region to cache
    /// resolved values into when the lookup originated from a function scope.
    pub fn current_stack_base(&self) -> usize {
        self.frames
            .last()
            .expect("VM should have at least one frame")
            .stack_base
    }

    /// Takes ownership of the globals vector, replacing it with an empty vec.
    ///
    /// Used by the REPL to reclaim globals after VM execution completes,
    /// before calling `cleanup()` (which would destroy them in ref-count-panic mode).
    pub fn take_globals(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.globals)
    }

    /// Allocates a new `CallId` for an external function call.
    fn allocate_call_id(&mut self) -> CallId {
        self.scheduler.allocate_call_id()
    }

    /// Returns true if we're on the main task (or no async at all).
    ///
    /// This is used to determine whether a `ReturnValue` at the last frame means
    /// module-level completion (return to host) or spawned task completion
    /// (handle task completion and switch).
    fn is_main_task(&self) -> bool {
        self.scheduler.current_task_id().is_none_or(TaskId::is_main)
    }

    /// Main execution loop.
    ///
    /// Fetches opcodes from the current frame's bytecode and executes them.
    /// Returns when execution completes, an error occurs, or an external
    /// call is needed.
    ///
    /// Uses locally cached `code` and `ip` variables to avoid repeated
    /// `frames.last_mut().expect()` calls during operand fetching. The cache
    /// is reloaded after any operation that modifies the frame stack.
    pub fn run(&mut self) -> Result<FrameExit, RunError> {
        // Cache frame state locally to avoid repeated frames.last_mut() calls.
        // The Code reference has lifetime 'a (lives in Interns), independent of frame borrow.
        let mut cached_frame: CachedFrame<'a> = self.new_cached_frame();

        loop {
            // Check time limit and trigger GC if needed at each instruction.
            // For NoLimitTracker, these are inlined no-ops that compile away.
            self.heap.check_time()?;

            if self.heap.should_gc() {
                // Sync IP before GC for safety
                self.current_frame_mut().ip = cached_frame.ip;
                self.run_gc();
            }

            // Track instruction IP for exception table lookup
            self.instruction_ip = cached_frame.ip;

            // Fetch opcode using cached values (no frame access)
            let opcode = {
                let byte = cached_frame.code.bytecode()[cached_frame.ip];
                cached_frame.ip += 1;
                Opcode::try_from(byte).expect("invalid opcode in bytecode")
            };

            match opcode {
                // ============================================================
                // Stack Operations
                // ============================================================
                Opcode::Pop => {
                    let value = self.pop();
                    value.drop_with_heap(self);
                }
                Opcode::Dup => {
                    let value = self.peek().clone_with_heap(self);
                    self.push(value);
                }
                Opcode::Dup2 => {
                    let len = self.stack.len();
                    let first = self.stack[len - 2].clone_with_heap(self);
                    let second = self.stack[len - 1].clone_with_heap(self);
                    self.push(first);
                    self.push(second);
                }
                Opcode::Rot2 => {
                    // Swap top two: [a, b] → [b, a]
                    let len = self.stack.len();
                    self.stack.swap(len - 1, len - 2);
                }
                Opcode::Rot3 => {
                    // Rotate top three: [a, b, c] → [c, a, b]
                    // Uses in-place rotation without cloning
                    let len = self.stack.len();
                    // Move c out, then shift a→b→c, then put c at a's position
                    // Equivalent to: [..rest, a, b, c] → [..rest, c, a, b]
                    self.stack[len - 3..].rotate_right(1);
                }
                // Constants & Literals
                Opcode::LoadConst => {
                    let idx = fetch_u16!(cached_frame);
                    let value = cached_frame.code.constants().get(idx);
                    // Handle InternLongInt specially - convert to heap-allocated LongInt
                    if let Value::InternLongInt(long_int_id) = value {
                        let bi = self.interns.get_long_int(*long_int_id).clone();
                        match LongInt::new(bi).into_value(self.heap) {
                            Ok(v) => self.push(v),
                            Err(e) => catch_sync!(self, cached_frame, RunError::from(e)),
                        }
                    } else {
                        self.push(value.clone_with_heap(self));
                    }
                }
                Opcode::LoadNone => self.push(Value::None),
                Opcode::LoadTrue => self.push(Value::Bool(true)),
                Opcode::LoadFalse => self.push(Value::Bool(false)),
                Opcode::LoadSmallInt => {
                    let n = fetch_i8!(cached_frame);
                    self.push(Value::Int(i64::from(n)));
                }
                // Variables - Specialized Local Loads (no operand)
                Opcode::LoadLocal0 => handle_load_result!(self, cached_frame, self.load_local(&cached_frame, 0)),
                Opcode::LoadLocal1 => handle_load_result!(self, cached_frame, self.load_local(&cached_frame, 1)),
                Opcode::LoadLocal2 => handle_load_result!(self, cached_frame, self.load_local(&cached_frame, 2)),
                Opcode::LoadLocal3 => handle_load_result!(self, cached_frame, self.load_local(&cached_frame, 3)),
                // Variables - General Local Operations
                Opcode::LoadLocal => {
                    let slot = u16::from(fetch_u8!(cached_frame));
                    handle_load_result!(self, cached_frame, self.load_local(&cached_frame, slot));
                }
                Opcode::LoadLocalW => {
                    let slot = fetch_u16!(cached_frame);
                    handle_load_result!(self, cached_frame, self.load_local(&cached_frame, slot));
                }
                Opcode::StoreLocal => {
                    let slot = u16::from(fetch_u8!(cached_frame));
                    self.store_local(&cached_frame, slot);
                }
                Opcode::StoreLocalW => {
                    let slot = fetch_u16!(cached_frame);
                    self.store_local(&cached_frame, slot);
                }
                Opcode::DeleteLocal => {
                    let slot = u16::from(fetch_u8!(cached_frame));
                    self.delete_local(&cached_frame, slot);
                }
                Opcode::DeleteGlobal => {
                    let slot = fetch_u16!(cached_frame);
                    self.delete_global(slot);
                }
                // Variables - Callable-context Local Loads
                Opcode::LoadLocalCallable => {
                    let slot = u16::from(fetch_u8!(cached_frame));
                    let name_id = StringId::from_index(fetch_u16!(cached_frame));
                    self.load_local_callable(&cached_frame, slot, name_id);
                }
                Opcode::LoadLocalCallableW => {
                    let slot = fetch_u16!(cached_frame);
                    let name_id = StringId::from_index(fetch_u16!(cached_frame));
                    self.load_local_callable(&cached_frame, slot, name_id);
                }
                // Variables - Global Operations
                Opcode::LoadGlobal => {
                    let slot = fetch_u16!(cached_frame);
                    handle_load_result!(self, cached_frame, self.load_global(slot));
                }
                Opcode::LoadGlobalCallable => {
                    let slot = fetch_u16!(cached_frame);
                    let name_id = StringId::from_index(fetch_u16!(cached_frame));
                    self.load_global_callable(slot, name_id);
                }
                Opcode::StoreGlobal => {
                    let slot = fetch_u16!(cached_frame);
                    self.store_global(slot);
                }
                // Variables - Cell Operations (closures)
                Opcode::LoadCell => {
                    let slot = fetch_u16!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.load_cell(&cached_frame, slot));
                }
                Opcode::StoreCell => {
                    let slot = fetch_u16!(cached_frame);
                    self.store_cell(&cached_frame, slot);
                }
                // Binary Operations - route through exception handling for tracebacks
                Opcode::BinaryAdd => try_catch_sync!(self, cached_frame, self.binary_add()),
                Opcode::BinarySub => try_catch_sync!(self, cached_frame, self.binary_sub()),
                Opcode::BinaryMul => try_catch_sync!(self, cached_frame, self.binary_mult()),
                Opcode::BinaryDiv => try_catch_sync!(self, cached_frame, self.binary_div()),
                Opcode::BinaryFloorDiv => try_catch_sync!(self, cached_frame, self.binary_floordiv()),
                Opcode::BinaryMod => try_catch_sync!(self, cached_frame, self.binary_mod()),
                Opcode::BinaryPow => try_catch_sync!(self, cached_frame, self.binary_pow()),
                // Bitwise operations - only work on integers
                Opcode::BinaryAnd => try_catch_sync!(self, cached_frame, self.binary_and()),
                Opcode::BinaryOr => try_catch_sync!(self, cached_frame, self.binary_or()),
                Opcode::BinaryXor => try_catch_sync!(self, cached_frame, self.binary_xor()),
                Opcode::BinaryLShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::LShift));
                }
                Opcode::BinaryRShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::RShift));
                }
                Opcode::BinaryMatMul => try_catch_sync!(self, cached_frame, self.binary_matmul()),
                // Comparison Operations
                Opcode::CompareEq => try_catch_sync!(self, cached_frame, self.compare_eq()),
                Opcode::CompareNe => try_catch_sync!(self, cached_frame, self.compare_ne()),
                Opcode::CompareLt => try_catch_sync!(self, cached_frame, self.compare_ord(Ordering::is_lt)),
                Opcode::CompareLe => try_catch_sync!(self, cached_frame, self.compare_ord(Ordering::is_le)),
                Opcode::CompareGt => try_catch_sync!(self, cached_frame, self.compare_ord(Ordering::is_gt)),
                Opcode::CompareGe => try_catch_sync!(self, cached_frame, self.compare_ord(Ordering::is_ge)),
                Opcode::CompareIs => self.compare_is(false),
                Opcode::CompareIsNot => self.compare_is(true),
                Opcode::CompareIn => try_catch_sync!(self, cached_frame, self.compare_in(false)),
                Opcode::CompareNotIn => try_catch_sync!(self, cached_frame, self.compare_in(true)),
                Opcode::CompareModEq => {
                    let const_idx = fetch_u16!(cached_frame);
                    let k = cached_frame.code.constants().get(const_idx);
                    try_catch_sync!(self, cached_frame, self.compare_mod_eq(k));
                }
                // Unary Operations
                Opcode::UnaryNot => {
                    let value = self.pop();
                    let result = !value.py_bool(self);
                    value.drop_with_heap(self);
                    self.push(Value::Bool(result));
                }
                Opcode::UnaryNeg => {
                    // Unary minus - negate numeric value
                    let value = self.pop();
                    match value {
                        Value::Int(n) => {
                            // Use checked_neg to handle i64::MIN overflow
                            if let Some(negated) = n.checked_neg() {
                                self.push(Value::Int(negated));
                            } else {
                                // i64::MIN negated overflows to LongInt
                                let li = -LongInt::from(n);
                                match li.into_value(self.heap) {
                                    Ok(v) => self.push(v),
                                    Err(e) => catch_sync!(self, cached_frame, RunError::from(e)),
                                }
                            }
                        }
                        Value::Float(f) => self.push(Value::Float(-f)),
                        Value::Bool(b) => self.push(Value::Int(if b { -1 } else { 0 })),
                        Value::Ref(id) => {
                            if let HeapData::LongInt(li) = self.heap.get(id) {
                                let negated = -LongInt::new(li.inner().clone());
                                value.drop_with_heap(self);
                                match negated.into_value(self.heap) {
                                    Ok(v) => self.push(v),
                                    Err(e) => catch_sync!(self, cached_frame, RunError::from(e)),
                                }
                            } else {
                                let value_type = value.py_type(self.heap);
                                value.drop_with_heap(self);
                                catch_sync!(self, cached_frame, ExcType::unary_type_error("-", value_type));
                            }
                        }
                        _ => {
                            let value_type = value.py_type(self.heap);
                            value.drop_with_heap(self);
                            catch_sync!(self, cached_frame, ExcType::unary_type_error("-", value_type));
                        }
                    }
                }
                Opcode::UnaryPos => {
                    // Unary plus - converts bools to int, no-op for other numbers
                    let value = self.pop();
                    match value {
                        Value::Int(_) | Value::Float(_) => self.push(value),
                        Value::Bool(b) => self.push(Value::Int(i64::from(b))),
                        Value::Ref(id) => {
                            if matches!(self.heap.get(id), HeapData::LongInt(_)) {
                                // LongInt - return as-is (value already has correct refcount)
                                self.push(value);
                            } else {
                                let value_type = value.py_type(self.heap);
                                value.drop_with_heap(self);
                                catch_sync!(self, cached_frame, ExcType::unary_type_error("+", value_type));
                            }
                        }
                        _ => {
                            let value_type = value.py_type(self.heap);
                            value.drop_with_heap(self);
                            catch_sync!(self, cached_frame, ExcType::unary_type_error("+", value_type));
                        }
                    }
                }
                Opcode::UnaryInvert => {
                    // Bitwise NOT
                    let value = self.pop();
                    match value {
                        Value::Int(n) => self.push(Value::Int(!n)),
                        Value::Bool(b) => self.push(Value::Int(!i64::from(b))),
                        Value::Ref(id) => {
                            if let HeapData::LongInt(li) = self.heap.get(id) {
                                // LongInt bitwise NOT: ~x = -(x + 1)
                                let inverted = -(li.inner() + 1i32);
                                value.drop_with_heap(self);
                                match LongInt::new(inverted).into_value(self.heap) {
                                    Ok(v) => self.push(v),
                                    Err(e) => catch_sync!(self, cached_frame, RunError::from(e)),
                                }
                            } else {
                                let value_type = value.py_type(self.heap);
                                value.drop_with_heap(self);
                                catch_sync!(self, cached_frame, ExcType::unary_type_error("~", value_type));
                            }
                        }
                        _ => {
                            let value_type = value.py_type(self.heap);
                            value.drop_with_heap(self);
                            catch_sync!(self, cached_frame, ExcType::unary_type_error("~", value_type));
                        }
                    }
                }
                // In-place Operations - route through exception handling
                Opcode::InplaceAdd => try_catch_sync!(self, cached_frame, self.inplace_add()),
                // Other in-place ops use the same logic as binary ops for now
                Opcode::InplaceSub => try_catch_sync!(self, cached_frame, self.binary_sub()),
                Opcode::InplaceMul => try_catch_sync!(self, cached_frame, self.binary_mult()),
                Opcode::InplaceDiv => try_catch_sync!(self, cached_frame, self.binary_div()),
                Opcode::InplaceFloorDiv => try_catch_sync!(self, cached_frame, self.binary_floordiv()),
                Opcode::InplaceMod => try_catch_sync!(self, cached_frame, self.binary_mod()),
                Opcode::InplacePow => try_catch_sync!(self, cached_frame, self.binary_pow()),
                Opcode::InplaceAnd => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::And));
                }
                Opcode::InplaceOr => try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::Or)),
                Opcode::InplaceXor => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::Xor));
                }
                Opcode::InplaceLShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::LShift));
                }
                Opcode::InplaceRShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::RShift));
                }
                // Collection Building - route through exception handling
                Opcode::BuildList => {
                    let count = fetch_u16!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.build_list(count));
                }
                Opcode::BuildTuple => {
                    let count = fetch_u16!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.build_tuple(count));
                }
                Opcode::BuildDict => {
                    let count = fetch_u16!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.build_dict(count));
                }
                Opcode::BuildSet => {
                    let count = fetch_u16!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.build_set(count));
                }
                Opcode::FormatValue => {
                    let flags = fetch_u8!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.format_value(flags));
                }
                Opcode::BuildFString => {
                    let count = fetch_u16!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.build_fstring(count));
                }
                Opcode::BuildSlice => {
                    try_catch_sync!(self, cached_frame, self.build_slice());
                }
                Opcode::ListExtend => {
                    try_catch_sync!(self, cached_frame, self.list_extend());
                }
                Opcode::ListToTuple => {
                    try_catch_sync!(self, cached_frame, self.list_to_tuple());
                }
                Opcode::DictMerge => {
                    let func_name_id = fetch_u16!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.dict_merge(func_name_id));
                }
                // PEP 448 literal building
                Opcode::DictUpdate => {
                    let depth = fetch_u8!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.dict_update(depth));
                }
                Opcode::SetExtend => {
                    let depth = fetch_u8!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.set_extend(depth));
                }
                // Comprehension Building - append/add/set items during iteration
                Opcode::ListAppend => {
                    let depth = fetch_u8!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.list_append(depth));
                }
                Opcode::SetAdd => {
                    let depth = fetch_u8!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.set_add(depth));
                }
                Opcode::DictSetItem => {
                    let depth = fetch_u8!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.dict_set_item(depth));
                }
                // Subscript & Attribute - route through exception handling
                Opcode::BinarySubscr => {
                    let index = self.pop();
                    let obj = self.pop();
                    let result = obj.py_getitem(&index, self);
                    obj.drop_with_heap(self);
                    index.drop_with_heap(self);
                    match result {
                        Ok(v) => self.push(v),
                        Err(e) => catch_sync!(self, cached_frame, e),
                    }
                }
                Opcode::StoreSubscr => {
                    // Stack order: value, obj, index (TOS)
                    let index = self.pop();
                    let mut obj = self.pop();
                    let value = self.pop();
                    let result = obj.py_setitem(index, value, self);
                    obj.drop_with_heap(self);
                    if let Err(e) = result {
                        catch_sync!(self, cached_frame, e);
                    }
                }
                Opcode::LoadAttr => {
                    let name_idx = fetch_u16!(cached_frame);
                    let name_id = StringId::from_index(name_idx);
                    handle_call_result!(self, cached_frame, self.load_attr(name_id));
                }
                Opcode::LoadAttrImport => {
                    let name_idx = fetch_u16!(cached_frame);
                    let name_id = StringId::from_index(name_idx);
                    handle_call_result!(self, cached_frame, self.load_attr_import(name_id));
                }
                Opcode::StoreAttr => {
                    let name_idx = fetch_u16!(cached_frame);
                    let name_id = StringId::from_index(name_idx);
                    try_catch_sync!(self, cached_frame, self.store_attr(name_id));
                }
                // Control Flow - use cached_frame.ip directly for jumps
                Opcode::Jump => {
                    let offset = fetch_i16!(cached_frame);
                    jump_relative!(cached_frame.ip, offset);
                }
                Opcode::JumpIfTrue => {
                    let offset = fetch_i16!(cached_frame);
                    let cond = self.pop();
                    if cond.py_bool(self) {
                        jump_relative!(cached_frame.ip, offset);
                    }
                    cond.drop_with_heap(self);
                }
                Opcode::JumpIfFalse => {
                    let offset = fetch_i16!(cached_frame);
                    let cond = self.pop();
                    if !cond.py_bool(self) {
                        jump_relative!(cached_frame.ip, offset);
                    }
                    cond.drop_with_heap(self);
                }
                Opcode::JumpIfTrueOrPop => {
                    let offset = fetch_i16!(cached_frame);
                    if self.peek().py_bool(self) {
                        jump_relative!(cached_frame.ip, offset);
                    } else {
                        let value = self.pop();
                        value.drop_with_heap(self);
                    }
                }
                Opcode::JumpIfFalseOrPop => {
                    let offset = fetch_i16!(cached_frame);
                    if self.peek().py_bool(self) {
                        let value = self.pop();
                        value.drop_with_heap(self);
                    } else {
                        jump_relative!(cached_frame.ip, offset);
                    }
                }
                // Iteration - route through exception handling
                Opcode::GetIter => {
                    let value = self.pop();
                    // Create a MontyIter from the value and store on heap
                    match MontyIter::new(value, self) {
                        Ok(iter) => match self.heap.allocate(HeapData::Iter(iter)) {
                            Ok(heap_id) => self.push(Value::Ref(heap_id)),
                            Err(e) => catch_sync!(self, cached_frame, e.into()),
                        },
                        Err(e) => catch_sync!(self, cached_frame, e),
                    }
                }
                Opcode::ForIter => {
                    let offset = fetch_i16!(cached_frame);
                    // Peek at the iterator on TOS and extract heap_id
                    let Value::Ref(heap_id) = *self.peek() else {
                        return Err(RunError::internal("ForIter: expected iterator ref on stack"));
                    };

                    // Use advance_iterator which avoids std::mem::replace overhead
                    // by using a two-phase approach: read state, get value, update index
                    match advance_on_heap(self.heap, heap_id, self.interns) {
                        Ok(Some(value)) => self.push(value),
                        Ok(None) => {
                            // Iterator exhausted - pop it and jump to end
                            let iter = self.pop();
                            iter.drop_with_heap(self);
                            jump_relative!(cached_frame.ip, offset);
                        }
                        Err(e) => {
                            // Error during iteration (e.g., dict size changed)
                            let iter = self.pop();
                            iter.drop_with_heap(self);
                            catch_sync!(self, cached_frame, e);
                        }
                    }
                }
                // Function Calls - sync IP before call, reload cache after frame changes
                Opcode::CallFunction => {
                    let arg_count = fetch_u8!(cached_frame) as usize;

                    // Sync IP before call (call_function may access frame for traceback)
                    self.current_frame_mut().ip = cached_frame.ip;

                    handle_call_result!(self, cached_frame, self.exec_call_function(arg_count));
                }
                Opcode::CallBuiltinFunction => {
                    // Fetch operands: builtin_id (u8) + arg_count (u8)
                    let builtin_id = fetch_u8!(cached_frame);
                    let arg_count = fetch_u8!(cached_frame) as usize;

                    // Sync IP before call (builtins like map() may call evaluate_function
                    // which pushes frames and runs a nested run() loop)
                    self.current_frame_mut().ip = cached_frame.ip;

                    match self.exec_call_builtin_function(builtin_id, arg_count) {
                        Ok(result) => self.push(result),
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                Opcode::CallBuiltinType => {
                    // Fetch operands: type_id (u8) + arg_count (u8)
                    let type_id = fetch_u8!(cached_frame);
                    let arg_count = fetch_u8!(cached_frame) as usize;

                    match self.exec_call_builtin_type(type_id, arg_count) {
                        Ok(result) => self.push(result),
                        // IP sync deferred to error path (no frame push possible)
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                Opcode::CallFunctionKw => {
                    // Fetch operands: pos_count, kw_count, then kw_count name indices
                    let pos_count = fetch_u8!(cached_frame) as usize;
                    let kw_count = fetch_u8!(cached_frame) as usize;

                    // Read keyword name StringIds
                    let mut kwname_ids = Vec::with_capacity(kw_count);
                    for _ in 0..kw_count {
                        kwname_ids.push(StringId::from_index(fetch_u16!(cached_frame)));
                    }

                    // Sync IP before call (call_function may access frame for traceback)
                    self.current_frame_mut().ip = cached_frame.ip;

                    handle_call_result!(self, cached_frame, self.exec_call_function_kw(pos_count, kwname_ids));
                }
                Opcode::CallAttr => {
                    // CallAttr: u16 name_id, u8 arg_count
                    // Stack: [obj, arg1, arg2, ..., argN] -> [result]
                    let name_idx = fetch_u16!(cached_frame);
                    let arg_count = fetch_u8!(cached_frame) as usize;
                    let name_id = StringId::from_index(name_idx);

                    // Sync IP before call (may yield to host for OS/external calls)
                    self.current_frame_mut().ip = cached_frame.ip;

                    handle_call_result!(self, cached_frame, self.exec_call_attr(name_id, arg_count));
                }
                Opcode::CallAttrKw => {
                    // CallAttrKw: u16 name_id, u8 pos_count, u8 kw_count, then kw_count u16 name indices
                    // Stack: [obj, pos_args..., kw_values...] -> [result]
                    let name_idx = fetch_u16!(cached_frame);
                    let pos_count = fetch_u8!(cached_frame) as usize;
                    let kw_count = fetch_u8!(cached_frame) as usize;
                    let name_id = StringId::from_index(name_idx);

                    // Read keyword name StringIds
                    let mut kwname_ids = Vec::with_capacity(kw_count);
                    for _ in 0..kw_count {
                        kwname_ids.push(StringId::from_index(fetch_u16!(cached_frame)));
                    }

                    // Sync IP before call (may yield to host for OS/external calls)
                    self.current_frame_mut().ip = cached_frame.ip;

                    handle_call_result!(
                        self,
                        cached_frame,
                        self.exec_call_attr_kw(name_id, pos_count, kwname_ids)
                    );
                }
                Opcode::CallFunctionExtended => {
                    let flags = fetch_u8!(cached_frame);
                    let has_kwargs = (flags & 0x01) != 0;

                    // Sync IP before call
                    self.current_frame_mut().ip = cached_frame.ip;

                    handle_call_result!(self, cached_frame, self.exec_call_function_extended(has_kwargs));
                }
                Opcode::CallAttrExtended => {
                    let name_idx = fetch_u16!(cached_frame);
                    let flags = fetch_u8!(cached_frame);
                    let name_id = StringId::from_index(name_idx);
                    let has_kwargs = (flags & 0x01) != 0;

                    // Sync IP before call (may yield to host for OS/external calls)
                    self.current_frame_mut().ip = cached_frame.ip;

                    handle_call_result!(self, cached_frame, self.exec_call_attr_extended(name_id, has_kwargs));
                }
                // Function Definition
                Opcode::MakeFunction => {
                    let func_idx = fetch_u16!(cached_frame);
                    let defaults_count = fetch_u8!(cached_frame) as usize;
                    let func_id = FunctionId::from_index(func_idx);

                    if defaults_count == 0 {
                        // No defaults - use inline Value::Function (no heap allocation)
                        self.push(Value::DefFunction(func_id));
                    } else {
                        // Pop default values from stack (drain maintains order: first pushed = first in vec)
                        let defaults = self.pop_n(defaults_count);

                        // Create FunctionDefaults on heap and push reference
                        let heap_id = self
                            .heap
                            .allocate(HeapData::FunctionDefaults(FunctionDefaults { func_id, defaults }))?;
                        self.push(Value::Ref(heap_id));
                    }
                }
                Opcode::MakeClosure => {
                    let func_idx = fetch_u16!(cached_frame);
                    let defaults_count = fetch_u8!(cached_frame) as usize;
                    let cell_count = fetch_u8!(cached_frame) as usize;
                    let func_id = FunctionId::from_index(func_idx);

                    // Pop cells from stack (pushed after defaults, so on top)
                    // Cells are Value::Ref pointing to HeapData::Cell
                    // We use individual pops which reverses order, so we need to reverse back
                    let mut cells = Vec::with_capacity(cell_count);
                    for _ in 0..cell_count {
                        // mut needed for dec_ref_forget when ref-count-panic feature is enabled
                        #[cfg_attr(not(feature = "ref-count-panic"), expect(unused_mut))]
                        let mut cell_val = self.pop();
                        match &cell_val {
                            Value::Ref(heap_id) => {
                                // Keep the reference - the Closure will own the HeapId
                                cells.push(*heap_id);
                                // Mark the Value as dereferenced since Closure takes ownership
                                // of the reference count (we don't call drop_with_heap because
                                // we're not decrementing the refcount, just transferring it)
                                #[cfg(feature = "ref-count-panic")]
                                cell_val.dec_ref_forget();
                            }
                            _ => {
                                return Err(RunError::internal("MakeClosure: expected cell reference on stack"));
                            }
                        }
                    }
                    // Reverse to get original order (individual pops reverse the order)
                    cells.reverse();

                    // Pop default values from stack (drain maintains order: first pushed = first in vec)
                    let defaults = self.pop_n(defaults_count);

                    // Create Closure on heap and push reference
                    let heap_id = self.heap.allocate(HeapData::Closure(Closure {
                        func_id,
                        cells,
                        defaults,
                    }))?;
                    self.push(Value::Ref(heap_id));
                }
                // Exception Handling
                Opcode::Raise => {
                    let exc = self.pop();
                    let error = self.make_exception(exc, true); // is_raise=true, hide caret
                    catch_sync!(self, cached_frame, error);
                }
                Opcode::Reraise => {
                    // Pop the current exception from the stack to re-raise it
                    // If caught, handle_exception will push it back
                    let error = if let Some(exc) = self.exception_stack.pop() {
                        self.make_exception(exc, true) // is_raise=true for reraise
                    } else {
                        // No active exception - create a RuntimeError
                        SimpleException::new_msg(ExcType::RuntimeError, "No active exception to reraise").into()
                    };
                    catch_sync!(self, cached_frame, error);
                }
                Opcode::ClearException => {
                    // Pop the current exception from the stack
                    // This restores the previous exception context (if any)
                    if let Some(exc) = self.exception_stack.pop() {
                        exc.drop_with_heap(self);
                    }
                }
                Opcode::CheckExcMatch => {
                    // Stack: [exception, exc_type] -> [exception, bool]
                    let exc_type = self.pop();
                    let exception = self.peek();
                    let result = self.check_exc_match(exception, &exc_type);
                    exc_type.drop_with_heap(self);
                    let result = result?;
                    self.push(Value::Bool(result));
                }
                // Return - reload cache after popping frame
                Opcode::ReturnValue => {
                    let value = self.pop();
                    if self.frames.len() == 1 {
                        // Last frame - check if this is main task or spawned task
                        let is_main_task = self.is_main_task();

                        if is_main_task {
                            // Module-level return - we're done
                            return Ok(FrameExit::Return(value));
                        }

                        // Spawned task completed - handle task completion
                        let result = self.handle_task_completion(value);
                        match result {
                            Ok(AwaitResult::ValueReady(v)) => {
                                self.push(v);
                            }
                            Ok(AwaitResult::FramePushed) => {
                                // Switched to another task - reload cache
                                reload_cache!(self, cached_frame);
                            }
                            Ok(AwaitResult::Yield(pending)) => {
                                // All tasks blocked - return to host
                                return Ok(FrameExit::ResolveFutures(pending));
                            }
                            Err(e) => {
                                catch_sync!(self, cached_frame, e);
                            }
                        }
                        continue;
                    }
                    // Pop current frame and push return value
                    if self.pop_frame() {
                        // This frame indicated evaluation should stop - return to host with value
                        // e.g. `evaluate_function`
                        return Ok(FrameExit::Return(value));
                    }
                    self.push(value);
                    // Reload cache from parent frame
                    reload_cache!(self, cached_frame);
                }
                // Async/Await
                Opcode::Await => {
                    // Sync IP before exec (may push new frame for coroutine)
                    self.current_frame_mut().ip = cached_frame.ip;
                    let result = self.exec_get_awaitable();
                    match result {
                        Ok(AwaitResult::ValueReady(value)) => {
                            self.push(value);
                        }
                        Ok(AwaitResult::FramePushed) => {
                            // Reload cache after pushing a new frame
                            reload_cache!(self, cached_frame);
                        }
                        Ok(AwaitResult::Yield(pending_calls)) => {
                            // All tasks are blocked - return control to host
                            return Ok(FrameExit::ResolveFutures(pending_calls));
                        }
                        Err(e) => {
                            catch_sync!(self, cached_frame, e);
                        }
                    }
                }
                // Unpacking - route through exception handling
                Opcode::UnpackSequence => {
                    let count = fetch_u8!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.unpack_sequence(count));
                }
                Opcode::UnpackEx => {
                    let before = fetch_u8!(cached_frame) as usize;
                    let after = fetch_u8!(cached_frame) as usize;
                    try_catch_sync!(self, cached_frame, self.unpack_ex(before, after));
                }
                // Special
                Opcode::Nop => {
                    // No operation
                }
                // Module Operations
                Opcode::LoadModule => {
                    let module_id = fetch_u8!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.load_module(module_id));
                }
                Opcode::RaiseImportError => {
                    // Fetch the module name from the constant pool and raise ModuleNotFoundError
                    let const_idx = fetch_u16!(cached_frame);
                    let module_name = cached_frame.code.constants().get(const_idx);
                    // The constant should be an InternString from compile_import/compile_import_from
                    let name_str = match module_name {
                        Value::InternString(id) => self.interns.get_str(*id),
                        _ => "<unknown>",
                    };
                    let error = ExcType::module_not_found_error(name_str);
                    catch_sync!(self, cached_frame, error);
                }
            }
        }
    }

    /// Loads a built-in module and pushes it onto the stack.
    fn load_module(&mut self, module_id: u8) -> RunResult<()> {
        let module = BuiltinModule::from_repr(module_id).expect("unknown module id");

        // Create the module on the heap using pre-interned strings
        let heap_id = module.create(self)?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Resumes execution after an external call completes.
    ///
    /// Pushes the return value onto the stack and continues execution.
    pub fn resume(&mut self, obj: MontyObject) -> Result<FrameExit, RunError> {
        let value = obj
            .to_value(self)
            .map_err(|e| SimpleException::new(ExcType::RuntimeError, Some(format!("invalid return type: {e}"))))?;
        self.push(value);
        self.run()
    }

    /// Sets the instruction IP used for exception table lookup and traceback generation.
    ///
    /// Used by `run()` to restore the IP to the load instruction's position before
    /// raising `NameError` for auto-injected `ExtFunction` values, so the traceback
    /// points to the name reference rather than the call expression.
    pub fn set_instruction_ip(&mut self, ip: usize) {
        self.instruction_ip = ip;
    }

    /// Resumes execution after an external call raised an exception.
    ///
    /// Uses the exception handling mechanism to try to catch the exception.
    /// If caught, continues execution at the handler. If not, propagates the error.
    pub fn resume_with_exception(&mut self, error: RunError) -> Result<FrameExit, RunError> {
        // Use the normal exception handling mechanism
        // handle_exception returns None if caught, Some(error) if not caught
        if let Some(uncaught_error) = self.handle_exception(error) {
            return Err(uncaught_error);
        }
        // Exception was caught, continue execution
        self.run()
    }

    // ========================================================================
    // Stack Operations
    // ========================================================================

    /// Pushes a value onto the operand stack.
    #[inline]
    pub(crate) fn push(&mut self, value: Value) {
        self.stack.push(value);
    }

    /// Pops a value from the operand stack.
    #[inline]
    pub(super) fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack underflow")
    }

    /// Peeks at the top of the operand stack without removing it.
    #[inline]
    pub(super) fn peek(&self) -> &Value {
        self.stack.last().expect("stack underflow")
    }

    /// Pops n values from the stack in reverse order (first popped is last in vec).
    pub(super) fn pop_n(&mut self, n: usize) -> Vec<Value> {
        let start = self.stack.len() - n;
        self.stack.drain(start..).collect()
    }

    // ========================================================================
    // Frame Operations
    // ========================================================================

    /// Returns a reference to the current (topmost) call frame.
    #[inline]
    pub(crate) fn current_frame(&self) -> &CallFrame<'a> {
        self.frames.last().expect("no active frame")
    }

    /// Creates a new cached frame from the current frame.
    #[inline]
    pub(super) fn new_cached_frame(&self) -> CachedFrame<'a> {
        self.current_frame().into()
    }

    /// Returns a mutable reference to the current call frame.
    #[inline]
    pub(super) fn current_frame_mut(&mut self) -> &mut CallFrame<'a> {
        self.frames.last_mut().expect("no active frame")
    }

    /// Pushes the given frame onto the call stack.
    ///
    /// Returns an error if the recursion depth limit is exceeded by pushing this frame.
    pub(super) fn push_frame(&mut self, frame: CallFrame<'a>) -> RunResult<()> {
        // root frame doesn't count towards recursion depth, so only check if there's already a frame on the stack
        if !self.frames.is_empty()
            && let Err(e) = self.heap.incr_recursion_depth()
        {
            self.cleanup_frame_state(&frame);
            return Err(e.into());
        }
        self.frames.push(frame);

        Ok(())
    }

    /// Pops the current frame from the call stack.
    ///
    /// Cleans up the frame's stack region and namespace (except for global namespace).
    /// Syncs `instruction_ip` to the parent frame's IP so that exception handling
    /// looks up handlers in the correct frame's exception table.
    ///
    /// Returns `true` if this frame indicated evaluation should stop when popped.
    pub(super) fn pop_frame(&mut self) -> bool {
        let frame = self.frames.pop().expect("no frame to pop");
        self.cleanup_frame_state(&frame);
        // Sync instruction_ip to the parent frame so exception table lookups
        // target the correct frame after returning from a nested run() call.
        if let Some(parent) = self.frames.last() {
            self.instruction_ip = parent.ip;
        }
        // Decrement recursion depth if this wasn't the root frame
        if !self.frames.is_empty() {
            self.heap.decr_recursion_depth();
        }
        frame.should_return
    }

    fn cleanup_frame_state(&mut self, frame: &CallFrame<'_>) {
        // Clean up frame's stack region (locals + operands).
        // Locals occupy stack[frame.stack_base..frame.stack_base + frame.locals_count],
        // operands are above that. Draining from stack_base covers both.
        self.stack
            .drain(frame.stack_base..)
            .for_each(|value| value.drop_with_heap(&mut *self.heap));

        // Track freed memory for locals
        if frame.locals_count > 0 {
            let size = frame.locals_count as usize * std::mem::size_of::<Value>();
            self.heap.tracker_mut().on_free(|| size);
        }
    }

    /// Cleans up all frames and stack values for the current task.
    ///
    /// Used when a task completes or fails and we need to switch to another task.
    /// Drains the stack with proper `drop_with_heap` for each value (since locals
    /// are inlined on the stack), then cleans up each frame's cell references.
    pub(super) fn cleanup_current_task(&mut self) {
        self.stack.drain(..).drop_with_heap(self.heap);
        self.frames.clear();
    }

    /// Runs garbage collection with proper GC roots.
    ///
    /// GC roots include values in the stack (locals + operands), globals, and exception stack.
    fn run_gc(&mut self) {
        // Collect roots from all reachable values
        let stack_roots = self.stack.iter().filter_map(Value::ref_id);
        let globals_roots = self.globals.iter().filter_map(Value::ref_id);
        let exc_roots = self.exception_stack.iter().filter_map(Value::ref_id);

        // Collect all roots into a vec to avoid lifetime issues
        let roots: Vec<HeapId> = stack_roots.chain(globals_roots).chain(exc_roots).collect();

        self.heap.collect_garbage(roots);
    }

    /// Returns the current source position for traceback generation.
    ///
    /// Uses `instruction_ip` which is set at the start of each instruction in the run loop,
    /// ensuring accurate position tracking even when using cached IP for bytecode fetching.
    pub(super) fn current_position(&self) -> CodeRange {
        let frame = self.current_frame();
        // Use instruction_ip which points to the start of the current instruction
        // (set at the beginning of each loop iteration in run())
        frame
            .code
            .location_for_offset(self.instruction_ip)
            .map(crate::bytecode::code::LocationEntry::range)
            .unwrap_or_default()
    }

    // ========================================================================
    // Variable Operations
    // ========================================================================

    /// Loads a local variable and pushes it onto the stack.
    ///
    /// For true locals (assigned somewhere in the function), returns `UnboundLocalError`
    /// if accessed before assignment. For unassigned names (never assigned in this scope),
    /// returns `NameLookup` to signal that the host should resolve the name.
    ///
    /// Returns `Ok(None)` for normal loads, `Ok(Some(FrameExit::NameLookup))` when
    /// the host needs to resolve an unknown name, or `Err` for true unbound locals.
    fn load_local(&mut self, cached_frame: &CachedFrame<'a>, slot: u16) -> Result<Option<FrameExit>, RunError> {
        let value = &self.stack[cached_frame.stack_base + slot as usize];

        // Check for undefined value — raise appropriate error based on whether
        // this is a true local (assigned somewhere) or an undefined reference
        if matches!(value, Value::Undefined) {
            let name = cached_frame.code.local_name(slot);
            if cached_frame.code.is_assigned_local(slot) {
                // True local accessed before assignment
                return Err(self.unbound_local_error(slot, name));
            }
            // Name doesn't exist in any scope — yield to host for resolution.
            let name_id = name.expect("LocalUnassigned should always have a name");
            return Ok(Some(FrameExit::NameLookup {
                name_id,
                namespace_slot: slot,
                is_global: false,
            }));
        }

        self.push(value.clone_with_heap(self.heap));
        Ok(None)
    }

    /// Loads a local variable in call context, pushing `ExtFunction` for undefined names.
    ///
    /// Unlike `load_local`, this never yields `NameLookup`. When the variable is undefined
    /// (a `LocalUnassigned` name), it pushes `Value::ExtFunction(name_id)` so that the
    /// subsequent `CallFunction` opcode can yield `FunctionCall` instead.
    fn load_local_callable(&mut self, cached_frame: &CachedFrame<'a>, slot: u16, name_id: StringId) {
        let value = &self.stack[cached_frame.stack_base + slot as usize];

        if matches!(value, Value::Undefined) {
            // LocalUnassigned in call context — push ExtFunction for the host to handle.
            self.ext_function_load_ip = Some(self.instruction_ip);
            self.push(Value::ExtFunction(name_id));
        } else {
            self.push(value.clone_with_heap(self.heap));
        }
    }

    /// Loads a global variable in call context, pushing `ExtFunction` for undefined names.
    ///
    /// Unlike `load_global`, this never yields `NameLookup`. When the variable is undefined,
    /// it pushes `Value::ExtFunction(name_id)` so that the subsequent `CallFunction` opcode
    /// can yield `FunctionCall` instead.
    fn load_global_callable(&mut self, slot: u16, name_id: StringId) {
        let value = self.globals[slot as usize].clone_with_heap(self.heap);

        if matches!(value, Value::Undefined) {
            // Save the load instruction's IP so NameError tracebacks point to the name
            self.ext_function_load_ip = Some(self.instruction_ip);
            self.push(Value::ExtFunction(name_id));
        } else {
            self.push(value);
        }
    }

    /// Creates an UnboundLocalError for a local variable accessed before assignment.
    fn unbound_local_error(&self, slot: u16, name: Option<StringId>) -> RunError {
        let name_str = match name {
            Some(id) => self.interns.get_str(id).to_string(),
            None => format!("<local {slot}>"),
        };
        ExcType::unbound_local_error(&name_str).into()
    }

    /// Creates a NameError for an undefined global variable.
    fn name_error(&self, slot: u16, name: Option<StringId>) -> RunError {
        let name_str = match name {
            Some(id) => self.interns.get_str(id).to_string(),
            None => format!("<global {slot}>"),
        };
        ExcType::name_error(&name_str).into()
    }

    /// Pops the top of stack and stores it in a local variable.
    fn store_local(&mut self, cached_frame: &CachedFrame<'a>, slot: u16) {
        let value = self.pop();
        let target = &mut self.stack[cached_frame.stack_base + slot as usize];
        let old_value = std::mem::replace(target, value);
        old_value.drop_with_heap(self);
    }

    /// Deletes a local variable (sets it to Undefined).
    fn delete_local(&mut self, cached_frame: &CachedFrame<'a>, slot: u16) {
        let target = &mut self.stack[cached_frame.stack_base + slot as usize];
        let old_value = std::mem::replace(target, Value::Undefined);
        old_value.drop_with_heap(self);
    }

    /// Loads a global variable and pushes it onto the stack.
    ///
    /// When the variable is undefined, yields `NameLookup` to the host for resolution
    /// instead of immediately raising `NameError`. This allows the host to provide
    /// external function bindings lazily.
    fn load_global(&mut self, slot: u16) -> Result<Option<FrameExit>, RunError> {
        let value = self.globals[slot as usize].clone_with_heap(self.heap);

        // Check for undefined value — raise appropriate error or yield to host
        if matches!(value, Value::Undefined) {
            let name = self.current_frame().code.local_name(slot);

            // If the name is registered as an assigned local (e.g. a module-level
            // variable or comprehension loop variable), raise UnboundLocalError
            // immediately rather than yielding NameLookup.
            if self.current_frame().code.is_assigned_local(slot) {
                return Err(self.unbound_local_error(slot, name));
            }

            let Some(name_id) = name else {
                // No name available — raise NameError directly
                return Err(self.name_error(slot, None));
            };
            Ok(Some(FrameExit::NameLookup {
                name_id,
                namespace_slot: slot,
                is_global: true,
            }))
        } else {
            self.push(value);
            Ok(None)
        }
    }

    /// Pops the top of stack and stores it in a global variable.
    fn store_global(&mut self, slot: u16) {
        let value = self.pop();
        let old_value = std::mem::replace(&mut self.globals[slot as usize], value);
        old_value.drop_with_heap(self);
    }

    /// Deletes a global variable (sets it to Undefined).
    fn delete_global(&mut self, slot: u16) {
        let old_value = std::mem::replace(&mut self.globals[slot as usize], Value::Undefined);
        old_value.drop_with_heap(self);
    }

    /// Loads from a closure cell and pushes onto the stack.
    ///
    /// The cell `HeapId` is read from the frame's local variable slot on the stack
    /// (cells are stored as `Value::Ref(cell_id)` at known positions in the locals region).
    /// Returns a `NameError` if the cell value is undefined (free variable not bound).
    fn load_cell(&mut self, cached_frame: &CachedFrame<'a>, slot: u16) -> RunResult<()> {
        let cell_id = self.cell_id_from_local(cached_frame, slot);
        let value = match self.heap.get(cell_id) {
            HeapData::Cell(c) => c.0.clone_with_heap(self),
            _ => panic!("LoadCell: entry is not a Cell"),
        };

        // Check for undefined value - raise NameError for unbound free variable
        if matches!(value, Value::Undefined) {
            value.drop_with_heap(self);
            let name = cached_frame.code.local_name(slot);
            return Err(self.free_var_error(name));
        }

        self.push(value);
        Ok(())
    }

    /// Extracts the cell `HeapId` from a local variable slot on the stack.
    ///
    /// Cell variables are stored as `Value::Ref(cell_id)` in the frame's locals region.
    fn cell_id_from_local(&self, cached_frame: &CachedFrame<'_>, slot: u16) -> HeapId {
        match &self.stack[cached_frame.stack_base + slot as usize] {
            Value::Ref(cell_id) => *cell_id,
            other => panic!("LoadCell/StoreCell: expected cell reference in local slot {slot}, found {other:?}"),
        }
    }

    /// Creates a NameError for an unbound free variable.
    fn free_var_error(&self, name: Option<StringId>) -> RunError {
        let name_str = match name {
            Some(id) => self.interns.get_str(id).to_string(),
            None => "<free var>".to_string(),
        };
        ExcType::name_error_free_variable(&name_str).into()
    }

    /// Pops the top of stack and stores it in a closure cell.
    ///
    /// The cell `HeapId` is read from the frame's local variable slot on the stack.
    fn store_cell(&mut self, cached_frame: &CachedFrame<'_>, slot: u16) {
        let value = self.pop();
        // The guard will clean up the new value if we panic, or the old value if we swap
        let mut guard = HeapGuard::new(value, self);
        let (value, this) = guard.as_parts_mut();

        let cell_id = this.cell_id_from_local(cached_frame, slot);
        match this.heap.get_mut(cell_id) {
            HeapDataMut::Cell(c) => std::mem::swap(&mut c.0, value),
            _ => panic!("StoreCell: entry is not a Cell"),
        }
    }
}

// `heap` is not a public field on VM, so this implementation needs to go here rather than in `heap.rs`
impl<T: ResourceTracker> ContainsHeap for VM<'_, '_, T> {
    type ResourceTracker = T;
    fn heap(&self) -> &Heap<T> {
        self.heap
    }
    fn heap_mut(&mut self) -> &mut Heap<T> {
        self.heap
    }
}
