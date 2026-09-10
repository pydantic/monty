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
mod context_manager;
mod exceptions;
mod format;
mod recursion;
mod scheduler;

use std::mem;

pub(crate) use attr::PendingLookupEffect;
pub(crate) use call::CallResult;
use monty_types::{InvalidInputError, MontyObject, MontyUuid, OsFunctionCall, PrintWriter};
pub(crate) use recursion::{ContainsVM, RecursionToken};
use scheduler::Scheduler;

use crate::{
    args::ArgValues,
    asyncio::{CallId, TaskId},
    builtins::Builtins,
    bytecode::{
        code::{Code, LocationEntry},
        op::{Opcode, decode_assert_flags},
    },
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, Heap, HeapData, HeapId, HeapReadOutput, HeapReader},
    heap_data::{CellValue, Closure, FunctionDefaults},
    intern::{FunctionId, Interns, StaticStrings, StringId},
    modules::{StandardLib, json::JsonStringCache, re::RePatternCache},
    object_bridge::MontyObjectExt,
    os_dispatch::{PendingOsEffect, listdir_names, release_pending_effect},
    parse::CodeRange,
    types::{
        Dict, LongInt, PyTrait,
        file::{apply_buffer_store, apply_write_position},
    },
    value::{EitherStr, Value},
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

/// Yields to the host when an exception left no task to run.
///
/// A spawned task whose exception nobody could receive is discarded with no
/// successor loaded, leaving the parked frame `cleanup_current_task` installs,
/// which dispatch must not execute. See [`VM::yield_parked`] for what is
/// handed back.
macro_rules! yield_if_parked {
    ($self:expr) => {
        if $self.current_frame.is_parked {
            return $self.yield_parked();
        }
    };
}

/// Tries an operation and routes a Python exception through the active frames.
macro_rules! try_catch {
    ($self:expr, $expr:expr) => {
        if let Err(e) = $expr {
            if let Some(result) = $self.handle_exception(e) {
                return Err(result);
            }
            yield_if_parked!($self);
        }
    };
}

/// Routes a Python exception through the active frames.
///
/// Wrapped in a block to allow use in match arm expressions.
macro_rules! catch {
    ($self:expr, $err:expr) => {{
        if let Some(result) = $self.handle_exception($err) {
            return Err(result);
        }
        yield_if_parked!($self);
    }};
}

/// Applies a relative jump offset to the current frame IP.
///
/// Uses checked arithmetic to safely compute the new IP, panicking if the
/// jump would result in a negative or overflowing instruction pointer.
macro_rules! jump_relative {
    ($ip:expr, $offset:expr) => {{
        $ip = $ip
            .checked_add_signed($offset.into())
            .expect("jump resulted in negative or overflowing IP");
    }};
}

/// Handles the result of a load operation that may yield a `FrameExit::NameLookup`.
///
/// `load_local` and `load_global` return `Result<Option<FrameExit>, RunError>`:
/// - `Ok(None)`: load succeeded, value is on the stack
/// - `Ok(Some(FrameExit::NameLookup { .. }))`: unresolved name, yield to host
/// - `Err(e)`: exception (e.g., UnboundLocalError)
macro_rules! handle_load_result {
    ($self:expr, $result:expr) => {
        match $result {
            Ok(None) => {}
            Ok(Some(frame_exit)) => return Ok(frame_exit),
            Err(e) => catch!($self, e),
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
/// - `FramePushed`: Continue with the newly current frame
/// - `External(ext_id, args)`: Return `FrameExit::ExternalCall` to yield to host
/// - `OsCall(call)`: Return `FrameExit::OsCall` to yield to host
/// - `MethodCall { .. }`: Return `FrameExit::MethodCall` to yield to host
/// - `AttrLookup { .. }`: Return `FrameExit::AttrLookup` to yield to host
/// - `AwaitValue(value)`: Push value, then implicitly await it via `exec_get_awaitable`
/// - `Err(err)`: Handle the exception via `catch!`
macro_rules! handle_call_result {
    ($self:expr, $result:expr) => {
        match $result {
            Ok(CallResult::Value(result)) => $self.push(result),
            Ok(CallResult::FramePushed) => {}
            Ok(CallResult::External(name, args)) => {
                let call_id = $self.allocate_call_id();
                let name_load_ip = $self.ext_function_load_ip.take();
                return Ok(FrameExit::ExternalCall {
                    function_name: name,
                    args,
                    call_id,
                    name_load_ip,
                });
            }
            Ok(CallResult::OsCall(function_call)) => {
                let call_id = $self.allocate_call_id();
                return Ok(FrameExit::OsCall {
                    function_call,
                    call_id,
                    effect: None,
                });
            }
            Ok(CallResult::OsCallWithEffect { call, effect }) => {
                let call_id = $self.allocate_call_id();
                // Not armed here — this exit may still be rejected on its
                // way out, and only a dispatched call earns a `resume`.
                return Ok(FrameExit::OsCall {
                    function_call: call,
                    call_id,
                    effect: Some(effect),
                });
            }
            Ok(CallResult::MethodCall { name, args, object_id }) => {
                let call_id = $self.allocate_call_id();
                return Ok(FrameExit::MethodCall {
                    method_name: name,
                    args,
                    call_id,
                    object_id,
                });
            }
            Ok(CallResult::AttrLookup {
                name,
                class_name,
                object_id,
                type_object,
                effect,
            }) => {
                return Ok(FrameExit::AttrLookup {
                    name,
                    class_name,
                    object_id,
                    type_object,
                    effect,
                });
            }
            Ok(CallResult::AwaitValue(value)) => {
                // Push the value and implicitly await it (used by asyncio.run())
                $self.push(value);
                match $self.exec_get_awaitable() {
                    Ok(AwaitResult::ValueReady(value)) => $self.push(value),
                    Ok(AwaitResult::FramePushed) => {}
                    Ok(AwaitResult::Yield(pending_calls)) => {
                        return Ok(FrameExit::ResolveFutures(pending_calls));
                    }
                    Err(e) => catch!($self, e),
                }
            }
            Err(err) => catch!($self, err),
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
        /// When a `LoadGlobalCallable` opcode auto-injects an `ExtFunction`
        /// for an undefined name, the load instruction's IP is saved here. In standard execution
        /// (without external function support), this IP is used to restore the frame pointer
        /// before raising `NameError`, so the traceback points to the name rather than the call.
        name_load_ip: Option<usize>,
    },

    /// Execution paused for an os function call.
    ///
    /// The caller should execute a function corresponding to the variant
    /// carried in `function_call` and call `resume()` with the result. The
    /// `call_id` allows the host to use async resolution by calling
    /// `run_pending()` instead of `run(result)`. `function_call` carries the
    /// typed args directly — no separate `args: ArgValues` field is needed.
    OsCall {
        /// Typed dispatch value carrying the OS function variant and its args.
        function_call: OsFunctionCall,
        /// Unique ID for this call, used for async correlation.
        call_id: CallId,
        /// Post-processing for this call's result, armed on
        /// [`VM::pending_os_effect`] only once the call reaches the host
        /// (`convert_frame_exit`); dropping the exit releases it instead.
        effect: Option<PendingOsEffect>,
    },

    /// Execution paused for a host-routed call: a method call on a host
    /// class instance, or on a host class type (a classmethod, or
    /// construction — spelled `__call__`).
    ///
    /// The caller should invoke the method on the original host object
    /// (routed by the `object_id` uuid) and call `resume()` with the
    /// result. The receiver is NOT included in `args`.
    MethodCall {
        /// Method name (e.g. "distance", or "__call__" for construction).
        method_name: EitherStr,
        /// Arguments for the call (the receiver is not among them).
        args: ArgValues,
        /// Unique ID for this call, used for async correlation.
        call_id: CallId,
        /// Uuid of the routed receiver (instance or class type).
        object_id: MontyUuid,
    },

    /// Execution paused for a lazy attribute lookup on a host-backed object
    /// (a class instance, or a class type when `type_object` is true).
    ///
    /// Produced by `obj.attr` / `Type.attr` (and `getattr()` / `hasattr()`)
    /// when `attr` is public and missing from the eager attrs. Resumed by
    /// value like `NameLookup` (no call_id); an "undefined" answer raises
    /// `AttributeError` naming `class_name` unless `effect` says otherwise.
    AttrLookup {
        /// The attribute name being looked up.
        name: EitherStr,
        /// Class name for the AttributeError message.
        class_name: String,
        /// Uuid of the object whose attribute is read.
        object_id: MontyUuid,
        /// True for a lookup on a class type object — selects CPython's
        /// `type object '...' has no attribute ...` message on failure.
        type_object: bool,
        /// How `getattr()` / `hasattr()` consume the answer; `None` for
        /// `obj.attr`. The only heap reference this exit can carry.
        effect: Option<PendingLookupEffect>,
    },

    /// All tasks are blocked waiting for external futures to resolve.
    ///
    /// The caller must resolve the pending CallIds before calling `resume()`.
    /// This happens when await is called on an ExternalFuture that hasn't
    /// been resolved yet, and there are no other ready tasks to switch to.
    ResolveFutures(Vec<CallId>),

    /// Execution paused for an unresolved name lookup.
    ///
    /// When the VM encounters an `Undefined` value in a global slot, it yields
    /// to the host to resolve the name.
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

impl<C: ContainsHeap> DropWithContext<C> for FrameExit {
    fn drop_with(self, heap: &mut C) {
        match self {
            Self::Return(value) => value.drop_with(heap),
            Self::ExternalCall { args, .. } | Self::MethodCall { args, .. } => {
                args.drop_with(heap);
            }
            Self::OsCall {
                function_call, effect, ..
            } => {
                function_call.drop_with(heap);
                // Never reached the host, so no `resume` will consume it.
                release_pending_effect(effect, heap);
            }
            Self::AttrLookup { effect, .. } => effect.drop_with(heap),
            Self::ResolveFutures(_) | Self::NameLookup { .. } => {}
        }
    }
}

/// A single function activation record.
///
/// Each frame represents one level in the call stack and owns its own
/// instruction pointer. This design avoids sync bugs on call/return.
#[derive(Debug)]
pub struct CallFrame<'code> {
    /// Bytecode being executed.
    code: &'code Code,

    /// `code.bytecode()`, hoisted into the frame so the dispatch loop reaches
    /// the instruction stream with one load instead of chasing
    /// `frame -> Code -> Vec` on every opcode and operand fetch.
    bytecode: &'code [u8],

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

    /// Base of this frame's entries in the VM-wide `exception_stack`.
    /// Recorded region depths are relative to this index, keeping caller
    /// exceptions intact when abandoned handlers are unwound.
    exception_stack_base: usize,

    /// Function ID (for tracebacks). None for module-level code.
    function_id: Option<FunctionId>,

    /// Caller's bytecode offset at the call site (for tracebacks). Stored raw
    /// and resolved to a `CodeRange` lazily on unwind (see `resolve_offset`) to
    /// skip the location-table scan unless the call raises. `None` at the root.
    call_offset: Option<u32>,

    /// When this frame returns (or exits with an exception) the VM should exit the run loop
    /// and return to the caller. Supports `evaluate_function`.
    should_return: bool,

    /// Whether this is a non-executing frame parked between active tasks.
    is_parked: bool,

    /// Whether this frame is a class `__init__` running for `Foo(...)`.
    ///
    /// When `true`, the `ReturnValue` handler discards the frame's return value
    /// (`__init__` returns `None`) and leaves the instance — pushed onto the
    /// caller's operand stack before this frame was created — as the result of the
    /// construction. Threaded through serialization (`SerializedFrame`) so a
    /// suspended initializer resumes correctly.
    is_initializer: bool,
}

impl<'code> CallFrame<'code> {
    /// Creates a new call frame for module-level code.
    ///
    /// Module frames have `locals_count = 0` because module-level variables
    /// are stored in the VM's `globals` vec, not in the stack.
    pub fn new_module(code: &'code Code, exception_stack_base: usize) -> Self {
        Self {
            code,
            bytecode: code.bytecode(),
            ip: 0,
            stack_base: 0,
            locals_count: 0,
            exception_stack_base,
            function_id: None,
            call_offset: None,
            should_return: false,
            is_parked: false,
            is_initializer: false,
        }
    }

    /// Creates a non-executing frame for a VM with no active Python task.
    fn new_parked(code: &'code Code) -> Self {
        let mut frame = Self::new_module(code, 0);
        frame.is_parked = true;
        frame
    }

    /// Creates a new call frame for a function call.
    ///
    /// The frame's layout on the VM stack is
    /// `[locals (locals_count) | operand stack ...]`. `stack_base` points at
    /// the start of the locals region; comprehension variables are pushed
    /// onto the operand stack at each comprehension's entry and popped at
    /// its exit, so they share the same address space as ordinary operand
    /// values (no separate per-frame region).
    pub fn new_function(
        code: &'code Code,
        stack_base: usize,
        locals_count: u16,
        exception_stack_base: usize,
        function_id: FunctionId,
        call_offset: Option<u32>,
    ) -> Self {
        Self {
            code,
            bytecode: code.bytecode(),
            ip: 0,
            stack_base,
            locals_count,
            exception_stack_base,
            function_id: Some(function_id),
            call_offset,
            should_return: false,
            is_parked: false,
            is_initializer: false,
        }
    }
}

impl CallFrame<'_> {
    /// Fetches `N` bytes from bytecode at the current IP, advancing IP by `N`.
    ///
    /// Performs a single bounds check covering all `N` bytes. All typed fetch
    /// helpers are built on top of this so each fetched operand — even
    /// multi-byte combinations like `u16 + u8 + u8` — costs exactly one
    /// bounds check.
    #[inline]
    fn fetch_array<const N: usize>(&mut self) -> [u8; N] {
        let Some(bytes) = self.bytecode.get(self.ip..).and_then(<[u8]>::first_chunk::<N>) else {
            unreachable!("instruction IP is out of bounds of the bytecode")
        };
        self.ip += N;
        *bytes
    }

    /// Fetches a `u8` operand at the current IP.
    #[inline]
    fn fetch_u8(&mut self) -> u8 {
        self.fetch_array::<1>()[0]
    }

    /// Fetches an `i8` operand at the current IP.
    #[inline]
    fn fetch_i8(&mut self) -> i8 {
        self.fetch_u8().cast_signed()
    }

    /// Fetches a little-endian `u16` operand at the current IP.
    #[inline]
    fn fetch_u16(&mut self) -> u16 {
        u16::from_le_bytes(self.fetch_array())
    }

    /// Fetches a little-endian `i16` operand at the current IP.
    #[inline]
    fn fetch_i16(&mut self) -> i16 {
        self.fetch_u16().cast_signed()
    }

    /// Fetches two consecutive `u8` operands in a single bounds check.
    ///
    /// Mirrors `CodeBuilder::emit_u8_u8` on the encode side.
    #[inline]
    fn fetch_u8_u8(&mut self) -> (u8, u8) {
        let [a, b] = self.fetch_array();
        (a, b)
    }

    /// Fetches a little-endian `u16` followed by a `u8`, in a single bounds check.
    ///
    /// Mirrors `CodeBuilder::emit_u16_u8` on the encode side.
    #[inline]
    fn fetch_u16_u8(&mut self) -> (u16, u8) {
        let [a, b, c] = self.fetch_array();
        (u16::from_le_bytes([a, b]), c)
    }

    /// Fetches two consecutive little-endian `u16`s, in a single bounds check.
    ///
    /// Mirrors the `Operand::U16U16` encoding (e.g. `LoadGlobalCallable`).
    #[inline]
    fn fetch_u16_u16(&mut self) -> (u16, u16) {
        let [a, b, c, d] = self.fetch_array();
        (u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d]))
    }

    /// Fetches a little-endian `u16` followed by two `u8`s, in a single bounds check.
    ///
    /// Mirrors `CodeBuilder::emit_u16_u8_u8` on the encode side.
    #[inline]
    fn fetch_u16_u8_u8(&mut self) -> (u16, u8, u8) {
        let [a, b, c, d] = self.fetch_array();
        (u16::from_le_bytes([a, b]), c, d)
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

    /// Base index into the VM-wide `exception_stack` for this frame.
    /// See `CallFrame.exception_stack_base`.
    exception_stack_base: usize,

    /// Caller's bytecode offset at the call site (for tracebacks). See
    /// `CallFrame.call_offset`.
    call_offset: Option<u32>,

    /// Whether this frame is a class `__init__` (see `CallFrame.is_initializer`).
    ///
    /// Unlike `should_return`, an initializer frame can legitimately be live
    /// across a suspend (an `__init__` that calls an external/OS function), so it
    /// must round-trip — otherwise the resumed frame would push `__init__`'s
    /// `None` instead of leaving the instance on the stack.
    #[serde(default)]
    is_initializer: bool,
}

impl CallFrame<'_> {
    /// Converts this frame to a serializable representation.
    fn serialize(&self) -> SerializedFrame {
        assert!(!self.is_parked, "cannot serialize a parked frame");
        assert!(
            !self.should_return,
            "cannot serialize frame marked for return - not yet supported"
        );
        SerializedFrame {
            function_id: self.function_id,
            ip: self.ip,
            stack_base: self.stack_base,
            locals_count: self.locals_count,
            exception_stack_base: self.exception_stack_base,
            call_offset: self.call_offset,
            is_initializer: self.is_initializer,
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

    /// In-flight resume effect for the paused OS call, if any. See
    /// [`VM::pending_os_effect`].
    #[serde(default)]
    pending_os_effect: Option<PendingOsEffect>,
    /// In-flight resume effect for the paused lazy attribute lookup, if any.
    /// See [`VM::pending_lookup_effect`].
    #[serde(default)]
    pending_lookup_effect: Option<PendingLookupEffect>,
}

impl VMSnapshot {
    /// Discards the in-flight execution state of a snapshot that will never be
    /// restored, releasing every heap reference it holds (operand and exception
    /// stacks, scheduler tasks, pending resume effects), and returns the globals
    /// so an abandoned REPL snippet keeps its namespace. Mirrors `VM::drop`.
    pub(crate) fn abandon(self, heap: &mut Heap) -> Vec<Value> {
        let Self {
            stack,
            globals,
            exception_stack,
            mut scheduler,
            pending_os_effect,
            pending_lookup_effect,
            ..
        } = self;
        HeapReader::with(heap, &mut (), |heap, ()| {
            release_pending_effect(pending_os_effect, heap);
            pending_lookup_effect.drop_with(heap);
            exception_stack.drop_with(heap);
            stack.drop_with(heap);
            scheduler.cleanup(heap);
        });
        globals
    }

    /// Number of tasks the scheduler held when this snapshot was taken.
    ///
    /// Test-only bridge for [`crate::ResolveFutures::__live_task_count_for_tests`];
    /// `scheduler` is private to this module.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn live_task_count(&self) -> usize {
        self.scheduler.task_count()
    }
}

// ============================================================================
// Virtual Machine
// ============================================================================

/// The bytecode virtual machine.
///
/// Executes compiled bytecode using a stack-based execution model.
/// The instruction pointer (IP) lives in each `CallFrame`, not here,
/// to avoid sync bugs on call/return.
pub struct VM<'h> {
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

    /// Frame currently executing, or a placeholder while no task is active.
    current_frame: CallFrame<'h>,

    /// Caller frames suspended below `current_frame`.
    suspended_frames: Vec<CallFrame<'h>>,

    /// Heap for reference-counted objects.
    pub(crate) heap: &'h mut HeapReader<'h>,

    /// Interned strings/bytes.
    pub(crate) interns: &'h Interns,

    /// Print output writer, borrowed so callers retain access to collected output.
    pub(crate) print_writer: PrintWriter<'h>,

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
    module_code: Option<&'h Code>,

    /// Bytecode IP of the most recent `LoadGlobalCallable` that
    /// pushed an `ExtFunction` for an undefined name.
    ///
    /// Used to restore the frame IP when standard execution converts an `ExternalCall`
    /// back to a `NameError`, so the traceback points to the name reference rather than
    /// the call expression.
    ext_function_load_ip: Option<usize>,

    /// Per-run string cache for `json.loads()`.
    ///
    /// Deduplicates heap allocations for repeated strings (especially dict keys)
    /// across multiple `json.loads()` calls within a single execution. Lazily
    /// initialized on first use, cleaned up when the VM is dropped.
    pub(crate) json_string_cache: JsonStringCache,

    /// File state update to apply when the next OS-call result resumes.
    ///
    /// `Some(effect)` between the yield to the host and the matching
    /// `resume()`; `None` otherwise. Cleared on resume after applying the
    /// file state effect, or on exception cleanup before the host-raised
    /// error is rethrown into Monty code.
    ///
    /// At most one OS call can be in flight at a time for a given task — the
    /// VM is single-threaded and OS calls are strictly request/response — so a
    /// single `Option` is sufficient even with async tasks (which interleave
    /// between OS calls, not within one).
    pub(crate) pending_os_effect: Option<PendingOsEffect>,

    /// How the paused lazy attribute lookup's answer is consumed on `resume`
    /// (`hasattr()` / `getattr()` default), armed like
    /// [`pending_os_effect`](Self::pending_os_effect) once the lookup reaches
    /// the host; `None` for `obj.attr` and when nothing is in flight.
    pub(crate) pending_lookup_effect: Option<PendingLookupEffect>,

    /// Current recursion depth — charged by function-call frames and by nested
    /// data-structure traversals (`repr`/`eq`/`cmp`/`hash`, json, ...).
    ///
    /// See [`recursion`](self::recursion) for the guard/token primitives that
    /// maintain it. Not serialized: it is reconstructed from the active frame
    /// count on `restore` and rebalanced per-task across async switches.
    recursion_depth: usize,

    /// Reusable scratch buffer for building a sync call's locals, avoiding a
    /// `malloc`/`free` per call. Only held transiently within
    /// `call_sync_function`, so one shared buffer is safe under recursion.
    namespace_scratch: Vec<Value>,
    /// Remaining native Rust call-stack re-entry budget, counted down from
    /// [`recursion::MAX_RUN_REENTRY_DEPTH`] around the two places the
    /// interpreter recurses on its own stack instead of switching
    /// `current_frame`: `evaluate_function`'s nested call into [`Self::run`],
    /// and `call_heap_callable`'s re-dispatch through a `functools.partial`.
    ///
    /// Not serialized, because neither site is still charged at a snapshot
    /// boundary: a nested `run()` never reaches one (its non-`Return` exits
    /// become `NotImplementedError` in `evaluate_function`), and a partial
    /// wrapping an external function releases its level as the suspending
    /// `CallResult` is returned. `debug_assert!`-checked in [`Self::snapshot`].
    run_reentry_depth: u8,

    /// Per-run cache of compiled patterns for module-level `re.*` calls. Not
    /// snapshotted (a pure performance cache), so default-initialized on restore.
    pub(crate) re_pattern_cache: RePatternCache,

    /// UTF-8 byte cap for each operand repr in introspected assert messages.
    /// Supplied by the executor on construction, so it is not snapshotted.
    pub(crate) assert_repr_max_bytes: u32,
}

impl<'h> VM<'h> {
    /// Creates a new VM with the given runtime context.
    pub fn new(
        globals: Vec<Value>,
        code: &'h Code,
        heap: &'h mut HeapReader<'h>,
        interns: &'h Interns,
        print_writer: PrintWriter<'h>,
        assert_repr_max_bytes: u32,
    ) -> Self {
        Self::new_with_frame(
            globals,
            CallFrame::new_module(code, 0),
            heap,
            interns,
            print_writer,
            assert_repr_max_bytes,
        )
    }

    /// Creates a VM from its initial frame and runtime context.
    fn new_with_frame(
        globals: Vec<Value>,
        current_frame: CallFrame<'h>,
        heap: &'h mut HeapReader<'h>,
        interns: &'h Interns,
        print_writer: PrintWriter<'h>,
        assert_repr_max_bytes: u32,
    ) -> Self {
        Self {
            stack: Vec::with_capacity(64),
            globals,
            current_frame,
            suspended_frames: Vec::with_capacity(16),
            heap,
            interns,
            print_writer,
            exception_stack: Vec::new(),
            instruction_ip: 0,
            scheduler: Scheduler::new(),
            ext_function_load_ip: None, // Set by LoadGlobalCallable
            module_code: None,
            json_string_cache: JsonStringCache::default(),
            pending_os_effect: None,
            pending_lookup_effect: None,
            recursion_depth: 0,
            namespace_scratch: Vec::new(),
            run_reentry_depth: recursion::MAX_RUN_REENTRY_DEPTH,
            re_pattern_cache: RePatternCache::default(),
            assert_repr_max_bytes,
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
    /// * `assert_repr_max_bytes` - Operand-repr byte cap from the executor
    pub fn restore(
        snapshot: VMSnapshot,
        module_code: &'h Code,
        heap: &'h mut HeapReader<'h>,
        interns: &'h Interns,
        print_writer: PrintWriter<'h>,
        assert_repr_max_bytes: u32,
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
                    bytecode: code.bytecode(),
                    ip: sf.ip,
                    stack_base: sf.stack_base,
                    locals_count: sf.locals_count,
                    exception_stack_base: sf.exception_stack_base,
                    function_id: sf.function_id,
                    call_offset: sf.call_offset,
                    should_return: false,
                    is_parked: false,
                    is_initializer: sf.is_initializer,
                }
            })
            .collect();

        // Restore recursion depth to match the number of suspended callers.
        // The root frame does not contribute to recursion depth.
        let current_frame_depth = frames.len().saturating_sub(1);
        let mut frames = frames;
        let current_frame = frames.pop().unwrap_or_else(|| CallFrame::new_parked(module_code));

        Self {
            stack: snapshot.stack,
            globals: snapshot.globals,
            current_frame,
            suspended_frames: frames,
            heap,
            interns,
            print_writer,
            exception_stack: snapshot.exception_stack,
            instruction_ip: snapshot.instruction_ip,
            scheduler: snapshot.scheduler,
            module_code: Some(module_code),
            ext_function_load_ip: None,
            json_string_cache: JsonStringCache::default(),
            pending_os_effect: snapshot.pending_os_effect,
            pending_lookup_effect: snapshot.pending_lookup_effect,
            recursion_depth: current_frame_depth,
            namespace_scratch: Vec::new(),
            // Always default value at a restore boundary — see the `run_reentry_depth` field doc.
            run_reentry_depth: recursion::MAX_RUN_REENTRY_DEPTH,
            re_pattern_cache: RePatternCache::default(),
            assert_repr_max_bytes,
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
    pub fn snapshot(mut self) -> VMSnapshot {
        // Always fully released (== MAX) here — see the field doc. Asserted to
        // catch a future `run()` call site that can suspend mid-re-entry.
        debug_assert_eq!(
            self.run_reentry_depth,
            recursion::MAX_RUN_REENTRY_DEPTH,
            "VM snapshotted while inside a nested native re-entry"
        );
        debug_assert!(
            !self.current_frame.is_parked || self.suspended_frames.is_empty(),
            "parked frame has suspended callers"
        );

        // Drop cached JSON strings before consuming the VM — they are not
        // included in the snapshot and their refcounts must be decremented.
        self.json_string_cache.drop_all(self.heap);

        VMSnapshot {
            // Move values directly — no clone, no refcount increment needed
            // (the VM owned them, now the snapshot owns them)
            stack: mem::take(&mut self.stack),
            globals: mem::take(&mut self.globals),
            frames: if self.current_frame.is_parked {
                Vec::new()
            } else {
                self.suspended_frames
                    .iter()
                    .chain([&self.current_frame])
                    .map(CallFrame::serialize)
                    .collect()
            },
            exception_stack: mem::take(&mut self.exception_stack),
            instruction_ip: self.instruction_ip,
            scheduler: mem::take(&mut self.scheduler),
            pending_os_effect: self.pending_os_effect.take(),
            pending_lookup_effect: self.pending_lookup_effect.take(),
        }
    }

    /// Runs the module frame installed when the VM was constructed.
    pub fn run_module(&mut self) -> Result<FrameExit, RunError> {
        // Store module code for restoring main task frames during task switching.
        self.module_code = Some(self.current_frame.code);
        self.run_external()
    }

    /// Returns the `stack_base` of the current (topmost) call frame.
    ///
    /// Used by `NameLookup` resolution to determine which stack region to cache
    /// resolved values into when the lookup originated from a function scope.
    pub fn current_stack_base(&self) -> usize {
        self.current_frame.stack_base
    }

    /// Takes ownership of the globals vector, replacing it with an empty vec.
    ///
    /// Used by the REPL to reclaim globals after VM execution completes.
    /// Must be called before the VM is dropped, since `Drop` will clean up
    /// any remaining globals with `drop_with`.
    pub fn take_globals(&mut self) -> Vec<Value> {
        mem::take(&mut self.globals)
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

    /// Runs the VM from a host boundary, bracketing the loop with the
    /// tracker's execution-clock hooks so `max_duration` measures cumulative
    /// *execution* time only — the clock stops whenever this returns
    /// (completion, error, or suspension at an external call).
    ///
    /// Every host turn must enter the loop through exactly one
    /// `run_external` call, and it must NEVER nest: VM-internal re-entry
    /// (task switches, `evaluate_function`) uses the raw private [`Self::run`]
    /// instead, whose time is already inside the enclosing window.
    pub(crate) fn run_external(&mut self) -> Result<FrameExit, RunError> {
        self.heap.tracker.on_execution_start();
        let result = self.run();
        self.heap.tracker.on_execution_stop();
        self.finish_host_turn(result)
    }

    /// Epilogue for every host-boundary execution window (here and
    /// `MontyRepl::call_function`): re-checks both resource limits so an
    /// overshoot that arose after the run loop's last amortized check — or
    /// was swallowed by a truncating caller (repr's `...[timeout]`) — cannot
    /// escape as a successful result. Consumes a discarded success so its
    /// heap refcounts are released rather than leaked.
    pub(crate) fn finish_host_turn<T: DropWithContext<Self>>(
        &mut self,
        result: Result<T, RunError>,
    ) -> Result<T, RunError> {
        // A turn shorter than the dispatch-checkpoint interval never probes
        // GC inside the run loop, so a stream of tiny feeds could otherwise
        // accumulate eligible cyclic garbage indefinitely — and the memory
        // check below could trip on memory a collection would reclaim. The
        // collection is charged to the execution clock like dispatch-loop GC,
        // so the limit check sees post-GC elapsed time as well as memory.
        // Skipped after a resource error, where refcounts are unreliable and
        // trial deletion could free live entries; ordinary Python exceptions
        // unwind through the drop machinery, so they still collect.
        if !matches!(result, Err(RunError::UncatchableExc(_))) && self.heap.should_gc() {
            self.heap.tracker.on_execution_start();
            self.run_gc();
            self.heap.tracker.on_execution_stop();
        }
        // Checked for erroring turns too: session state survives Python
        // exceptions, so allocate-then-raise feeds must not evade the limits.
        // The uncatchable resource error out-ranks the turn's own error.
        match self.heap.tracker.check_memory_time() {
            Ok(()) => result,
            Err(e) => {
                if let Ok(value) = result {
                    value.drop_with(self);
                }
                Err(e.into())
            }
        }
    }

    /// Periodic dispatch-loop work, outlined (`#[inline(never)]`) so the hot
    /// loop stays small: the amortized memory + time check — where a timeout
    /// swallowed by a truncating caller re-detects (elapsed time is
    /// monotonic), backstopped by the `run_external` exit check — the
    /// GC-scheduling probe, and the print writer's flush poll.
    #[inline(never)]
    fn dispatch_checkpoint(&mut self, check_limits: bool, poll_print: bool) -> Result<(), RunError> {
        if check_limits {
            self.heap.tracker.check_memory_time()?;
        }
        if self.heap.should_gc() {
            self.run_gc();
        }
        // A buffering writer only flushes when written to, so code that prints
        // and then computes in silence would hold that output until its next
        // print. This is what bounds that wait.
        if poll_print {
            self.print_writer.poll_flush()?;
        }
        Ok(())
    }

    /// Main execution loop.
    ///
    /// Fetches opcodes from the current frame's bytecode and executes them.
    /// Returns when execution completes, an error occurs, or an external
    /// call is needed.
    ///
    /// Private: host boundaries go through [`Self::run_external`] (directly
    /// or via `run_module`/`resume`/`resume_with_exception`/
    /// `resume_with_resolved_futures`) so the execution clock is accounted;
    /// only VM-internal re-entry calls this raw loop.
    ///
    /// The current frame is stored separately from its suspended callers, so
    /// dispatch updates its authoritative instruction pointer directly.
    fn run(&mut self) -> Result<FrameExit, RunError> {
        /// How often (in instructions) the dispatch loop runs its periodic
        /// work: the full `check_memory_time` and the GC-scheduling probe.
        /// The checkpoint reads the clock when limits are armed, so this sets
        /// the entire cost of limit enforcement — ~40% on tight loops at 10,
        /// ~2% at u8::MAX (see the `_limits` benchmarks) — while detection
        /// latency stays sub-µs. Native ops poll internally and the host-turn
        /// epilogue re-checks, so only this dispatch cadence rides on it.
        /// The countdown is per-`run()`, so `evaluate_function` re-entry
        /// restarts it — a native loop calling a shorter callback reaches no
        /// checkpoint at all and must poll the tracker itself.
        const CHECK_INTERVAL: u8 = u8::MAX;

        // Limits cannot change mid-run (`set_max_duration` needs `&mut` at the
        // host boundary), so with none configured the whole checkpoint reduces
        // to this one hoisted, well-predicted branch per instruction.
        let check_limits = self.heap.tracker.has_memory_time_limit();
        // Likewise hoisted: only a `Callback` writer can buffer, so every other
        // variant skips the poll — and the clock read behind it — entirely.
        let poll_print = self.print_writer.wants_poll();

        let mut countdown = CHECK_INTERVAL;

        loop {
            // One decrement-and-branch per instruction; the periodic work
            // runs every `CHECK_INTERVAL`-th, outlined to keep the hot loop
            // small. GC triggering is allocation-count-based (intervals in
            // the hundreds), so probing it a few instructions late is
            // immaterial.
            if let Some(c) = countdown.checked_sub(1) {
                countdown = c;
            } else {
                countdown = CHECK_INTERVAL;
                self.dispatch_checkpoint(check_limits, poll_print)?;
            }

            // Track instruction IP for exception table lookup
            self.instruction_ip = self.current_frame.ip;

            // Fetch the opcode and advance the authoritative frame IP.
            let opcode = {
                let byte = self.current_frame.bytecode[self.current_frame.ip];
                self.current_frame.ip += 1;
                Opcode::from_repr(byte).expect("invalid opcode in bytecode")
            };

            match opcode {
                // ============================================================
                // Stack Operations
                // ============================================================
                Opcode::Pop => {
                    let value = self.pop();
                    value.drop_with(self);
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
                    let idx = self.current_frame.fetch_u16();
                    let value = self.current_frame.code.constants().get(idx);
                    // Handle InternLongInt specially - convert to heap-allocated LongInt
                    if let Value::InternLongInt(long_int_id) = value {
                        let bi = self.interns.get_long_int(*long_int_id).clone();
                        let long_value = LongInt::new(bi).into_value(self.heap);
                        self.push(long_value);
                    } else {
                        self.push(value.clone_with_heap(self));
                    }
                }
                Opcode::LoadNone => self.push(Value::None),
                Opcode::LoadTrue => self.push(Value::Bool(true)),
                Opcode::LoadFalse => self.push(Value::Bool(false)),
                Opcode::BuildCell => {
                    let cell_id = self.heap.allocate(HeapData::Cell(CellValue(Value::Undefined)));
                    self.push(Value::Ref(cell_id));
                }
                Opcode::LoadSmallInt => {
                    let n = self.current_frame.fetch_i8();
                    self.push(Value::Int(i64::from(n)));
                }
                // Variables - Specialized Local Loads (no operand)
                Opcode::LoadLocal0 => try_catch!(self, self.load_local(0)),
                Opcode::LoadLocal1 => try_catch!(self, self.load_local(1)),
                Opcode::LoadLocal2 => try_catch!(self, self.load_local(2)),
                Opcode::LoadLocal3 => try_catch!(self, self.load_local(3)),
                // Variables - General Local Operations
                Opcode::LoadLocal => {
                    let slot = u16::from(self.current_frame.fetch_u8());
                    try_catch!(self, self.load_local(slot));
                }
                Opcode::LoadLocalW => {
                    let slot = self.current_frame.fetch_u16();
                    try_catch!(self, self.load_local(slot));
                }
                Opcode::StoreLocal => {
                    let slot = u16::from(self.current_frame.fetch_u8());
                    self.store_local(slot);
                }
                Opcode::StoreLocalW => {
                    let slot = self.current_frame.fetch_u16();
                    self.store_local(slot);
                }
                Opcode::LiftToTop => {
                    let n = self.current_frame.fetch_u8();
                    // Move the item at TOS - n to TOS, shifting items in
                    // between down by one. Single `rotate_left(1)` on the
                    // affected slice does exactly that.
                    let len = self.stack.len();
                    let src_idx = len - 1 - n as usize;
                    self.stack[src_idx..].rotate_left(1);
                }
                Opcode::RaiseUnboundLocal => {
                    let name_idx = self.current_frame.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    catch!(self, self.unbound_local_error(0, Some(name_id)));
                }
                Opcode::DeleteLocal => {
                    let slot = u16::from(self.current_frame.fetch_u8());
                    self.delete_local(slot);
                }
                Opcode::DeleteGlobal => {
                    let slot = self.current_frame.fetch_u16();
                    try_catch!(self, self.delete_global(slot));
                }
                // Variables - Global Operations
                Opcode::LoadGlobal => {
                    let slot = self.current_frame.fetch_u16();
                    handle_load_result!(self, self.load_global(slot));
                }
                Opcode::LoadGlobalCallable => {
                    let (slot, name_idx) = self.current_frame.fetch_u16_u16();
                    let name_id = StringId::from_index(name_idx);
                    self.load_global_callable(slot, name_id);
                }
                Opcode::StoreGlobal => {
                    let slot = self.current_frame.fetch_u16();
                    self.store_global(slot);
                }
                // Variables - Cell Operations (closures)
                Opcode::LoadCell => {
                    let slot = self.current_frame.fetch_u16();
                    try_catch!(self, self.load_cell(slot));
                }
                Opcode::StoreCell => {
                    let slot = self.current_frame.fetch_u16();
                    self.store_cell(slot);
                }
                Opcode::DeleteCell => {
                    let slot = self.current_frame.fetch_u16();
                    self.delete_cell(slot);
                }
                // Binary Operations - route through exception handling for tracebacks
                Opcode::BinaryAdd => try_catch!(self, self.binary_add()),
                Opcode::BinarySub => try_catch!(self, self.binary_sub()),
                Opcode::BinaryMul => try_catch!(self, self.binary_mult()),
                Opcode::BinaryDiv => try_catch!(self, self.binary_div()),
                Opcode::BinaryFloorDiv => try_catch!(self, self.binary_floordiv()),
                Opcode::BinaryMod => try_catch!(self, self.binary_mod()),
                Opcode::BinaryPow => try_catch!(self, self.binary_pow()),
                // Bitwise operations - only work on integers
                Opcode::BinaryAnd => try_catch!(self, self.binary_and()),
                Opcode::BinaryOr => try_catch!(self, self.binary_or()),
                Opcode::BinaryXor => try_catch!(self, self.binary_xor()),
                Opcode::BinaryLShift => {
                    try_catch!(self, self.binary_lshift());
                }
                Opcode::BinaryRShift => {
                    try_catch!(self, self.binary_rshift());
                }
                Opcode::BinaryMatMul => try_catch!(self, self.binary_matmul()),
                // Comparison Operations
                Opcode::CompareEq => try_catch!(self, self.compare_eq()),
                Opcode::CompareNe => try_catch!(self, self.compare_ne()),
                Opcode::CompareLt => try_catch!(self, self.compare_lt()),
                Opcode::CompareLe => try_catch!(self, self.compare_le()),
                Opcode::CompareGt => try_catch!(self, self.compare_gt()),
                Opcode::CompareGe => try_catch!(self, self.compare_ge()),
                Opcode::CompareIs => try_catch!(self, self.compare_is()),
                Opcode::CompareIsNot => try_catch!(self, self.compare_is_not()),
                Opcode::CompareIn => try_catch!(self, self.compare_in()),
                Opcode::CompareNotIn => try_catch!(self, self.compare_not_in()),
                // Unary Operations
                Opcode::UnaryNot => {
                    let value = self.pop();
                    let result = value.py_bool(self);
                    value.drop_with(self);
                    match result {
                        Ok(value) => self.push(Value::Bool(!value)),
                        Err(error) => catch!(self, error),
                    }
                }
                Opcode::UnaryNeg => try_catch!(self, self.unary_neg()),
                Opcode::UnaryPos => try_catch!(self, self.unary_pos()),
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
                                value.drop_with(self);
                                let inverted_value = LongInt::new(inverted).into_value(self.heap);
                                self.push(inverted_value);
                            } else {
                                let value_type = value.py_type_name(self);
                                value.drop_with(self);
                                catch!(self, ExcType::unary_type_error("~", &value_type));
                            }
                        }
                        _ => {
                            let value_type = value.py_type_name(self);
                            value.drop_with(self);
                            catch!(self, ExcType::unary_type_error("~", &value_type));
                        }
                    }
                }
                // In-place Operations - route through exception handling.
                // `+=`/`-=`/`&=`/`|=` first try a true in-place `Counter` op
                // (mutating the left operand); everything else — and the
                // non-Counter fallback — reuses the binary implementation, since
                // Monty's other types have no distinct in-place form.
                Opcode::InplaceAdd => try_catch!(self, self.inplace_add()),
                Opcode::InplaceSub => try_catch!(self, self.inplace_sub()),
                Opcode::InplaceMul => try_catch!(self, self.binary_mult()),
                Opcode::InplaceDiv => try_catch!(self, self.binary_div()),
                Opcode::InplaceFloorDiv => try_catch!(self, self.binary_floordiv()),
                Opcode::InplaceMod => try_catch!(self, self.binary_mod()),
                Opcode::InplacePow => try_catch!(self, self.binary_pow()),
                Opcode::InplaceAnd => {
                    try_catch!(self, self.inplace_and());
                }
                Opcode::InplaceOr => try_catch!(self, self.inplace_or()),
                Opcode::InplaceXor => {
                    try_catch!(self, self.binary_xor());
                }
                Opcode::InplaceLShift => {
                    try_catch!(self, self.binary_lshift());
                }
                Opcode::InplaceRShift => {
                    try_catch!(self, self.binary_rshift());
                }
                // Collection Building - route through exception handling
                Opcode::BuildList => {
                    let count = self.current_frame.fetch_u16() as usize;
                    self.build_list(count);
                }
                Opcode::BuildTuple => {
                    let count = self.current_frame.fetch_u16() as usize;
                    self.build_tuple(count);
                }
                Opcode::BuildDict => {
                    let count = self.current_frame.fetch_u16() as usize;
                    try_catch!(self, self.build_dict(count));
                }
                Opcode::BuildSet => {
                    let count = self.current_frame.fetch_u16() as usize;
                    try_catch!(self, self.build_set(count));
                }
                Opcode::FormatValue => {
                    let flags = self.current_frame.fetch_u8();
                    try_catch!(self, self.format_value(flags));
                }
                Opcode::BuildFString => {
                    let count = self.current_frame.fetch_u16() as usize;
                    try_catch!(self, self.build_fstring(count));
                }
                Opcode::BuildSlice => {
                    try_catch!(self, self.build_slice());
                }
                Opcode::ListExtend => {
                    try_catch!(self, self.list_extend());
                }
                Opcode::ListToTuple => {
                    try_catch!(self, self.list_to_tuple());
                }
                Opcode::DictMerge => {
                    let func_name_id = self.current_frame.fetch_u16();
                    try_catch!(self, self.dict_merge(func_name_id));
                }
                Opcode::MethodDictMerge => {
                    let func_name_id = self.current_frame.fetch_u16();
                    try_catch!(self, self.method_dict_merge(func_name_id));
                }
                // PEP 448 literal building
                Opcode::DictUpdate => {
                    let depth = self.current_frame.fetch_u8() as usize;
                    try_catch!(self, self.dict_update(depth));
                }
                Opcode::SetExtend => {
                    let depth = self.current_frame.fetch_u8() as usize;
                    try_catch!(self, self.set_extend(depth));
                }
                // Comprehension Building - append/add/set items during iteration
                Opcode::ListAppend => {
                    let depth = self.current_frame.fetch_u8() as usize;
                    try_catch!(self, self.list_append(depth));
                }
                Opcode::SetAdd => {
                    let depth = self.current_frame.fetch_u8() as usize;
                    try_catch!(self, self.set_add(depth));
                }
                Opcode::DictSetItem => {
                    let depth = self.current_frame.fetch_u8() as usize;
                    try_catch!(self, self.dict_set_item(depth));
                }
                // Subscript & Attribute - route through exception handling
                Opcode::BinarySubscr => {
                    let index = self.pop();
                    let obj = self.pop();
                    let result = obj.py_getitem(&index, self);
                    obj.drop_with(self);
                    index.drop_with(self);
                    match result {
                        Ok(v) => self.push(v),
                        Err(e) => catch!(self, e),
                    }
                }
                Opcode::StoreSubscr => {
                    // Stack order: value, obj, index (TOS)
                    let index = self.pop();
                    let mut obj = self.pop();
                    let value = self.pop();
                    let result = obj.py_setitem(index, value, self);
                    obj.drop_with(self);
                    if let Err(e) = result {
                        catch!(self, e);
                    }
                }
                Opcode::LoadAttr => {
                    let name_idx = self.current_frame.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    handle_call_result!(self, self.load_attr(name_id));
                }
                Opcode::LoadAttrImport => {
                    let name_idx = self.current_frame.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    handle_call_result!(self, self.load_attr_import(name_id));
                }
                Opcode::StoreAttr => {
                    let name_idx = self.current_frame.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    try_catch!(self, self.store_attr(name_id));
                }
                // Control Flow - use self.current_frame.ip directly for jumps
                Opcode::Jump => {
                    let offset = self.current_frame.fetch_i16();
                    jump_relative!(self.current_frame.ip, offset);
                }
                Opcode::JumpIfTrue => {
                    let offset = self.current_frame.fetch_i16();
                    let cond = self.pop();
                    let result = cond.py_bool(self);
                    cond.drop_with(self);
                    match result {
                        Ok(true) => jump_relative!(self.current_frame.ip, offset),
                        Ok(false) => {}
                        Err(error) => catch!(self, error),
                    }
                }
                Opcode::JumpIfFalse => {
                    let offset = self.current_frame.fetch_i16();
                    let cond = self.pop();
                    let result = cond.py_bool(self);
                    cond.drop_with(self);
                    match result {
                        Ok(false) => jump_relative!(self.current_frame.ip, offset),
                        Ok(true) => {}
                        Err(error) => catch!(self, error),
                    }
                }
                Opcode::JumpIfTrueOrPop => {
                    let offset = self.current_frame.fetch_i16();
                    let value = self.pop();
                    match value.py_bool(self) {
                        Ok(true) => {
                            self.push(value);
                            jump_relative!(self.current_frame.ip, offset);
                        }
                        Ok(false) => value.drop_with(self),
                        Err(error) => {
                            value.drop_with(self);
                            catch!(self, error);
                        }
                    }
                }
                Opcode::JumpIfFalseOrPop => {
                    let offset = self.current_frame.fetch_i16();
                    let value = self.pop();
                    match value.py_bool(self) {
                        Ok(true) => value.drop_with(self),
                        Ok(false) => {
                            self.push(value);
                            jump_relative!(self.current_frame.ip, offset);
                        }
                        Err(error) => {
                            value.drop_with(self);
                            catch!(self, error);
                        }
                    }
                }
                // Iteration - route through exception handling
                Opcode::GetIter => {
                    let value = self.pop();
                    let iterator = value.py_iter(self);
                    value.drop_with(self);
                    match iterator {
                        Ok(iterator) => self.push(iterator),
                        Err(e) => catch!(self, e),
                    }
                }
                Opcode::ForIter => {
                    let offset = self.current_frame.fetch_i16();
                    // Iterator implementations return heap objects from `py_iter`.
                    let Value::Ref(heap_id) = *self.peek() else {
                        return Err(RunError::internal("ForIter: expected iterator ref on stack"));
                    };
                    let mut iter = self.heap.read(heap_id);

                    match iter.py_next(self) {
                        Ok(Some(value)) => self.push(value),
                        Ok(None) => {
                            // Drop the HeapRead before dec_ref to release the reader count
                            drop(iter);
                            // Iterator exhausted - pop it and jump to end
                            let iter = self.pop();
                            iter.drop_with(self);
                            jump_relative!(self.current_frame.ip, offset);
                        }
                        Err(e) => {
                            // Drop the HeapRead before dec_ref to release the reader count
                            drop(iter);
                            // Error during iteration (e.g., dict size changed)
                            let iter = self.pop();
                            iter.drop_with(self);
                            catch!(self, e);
                        }
                    }
                }
                // Function Calls
                Opcode::CallFunction => {
                    let arg_count = self.current_frame.fetch_u8() as usize;
                    handle_call_result!(self, self.exec_call_function(arg_count));
                }
                Opcode::CallBuiltinFunction => {
                    let (builtin_id, arg_count) = self.current_frame.fetch_u8_u8();
                    let result = self.exec_call_builtin_function(builtin_id, arg_count as usize);
                    handle_call_result!(self, result);
                }
                Opcode::CallBuiltinType => {
                    let (type_id, arg_count) = self.current_frame.fetch_u8_u8();
                    let arg_count = arg_count as usize;

                    match self.exec_call_builtin_type(type_id, arg_count) {
                        Ok(result) => self.push(result),
                        Err(err) => catch!(self, err),
                    }
                }
                Opcode::CallFunctionKw => {
                    // Fetch operands: pos_count, kw_count, then kw_count name indices
                    let (pos_count, kw_count) = self.current_frame.fetch_u8_u8();
                    let (pos_count, kw_count) = (pos_count as usize, kw_count as usize);

                    // Read keyword name StringIds
                    let mut kwname_ids = Vec::with_capacity(kw_count);
                    for _ in 0..kw_count {
                        kwname_ids.push(StringId::from_index(self.current_frame.fetch_u16()));
                    }

                    handle_call_result!(self, self.exec_call_function_kw(pos_count, kwname_ids));
                }
                Opcode::CallAttr => {
                    // CallAttr: u16 name_id, u8 arg_count
                    // Stack: [obj, arg1, arg2, ..., argN] -> [result]
                    let (name_idx, arg_count) = self.current_frame.fetch_u16_u8();
                    let name_id = StringId::from_index(name_idx);
                    let arg_count = arg_count as usize;

                    handle_call_result!(self, self.exec_call_attr(name_id, arg_count));
                }
                Opcode::CallAttrKw => {
                    // CallAttrKw: u16 name_id, u8 pos_count, u8 kw_count, then kw_count u16 name indices
                    // Stack: [obj, pos_args..., kw_values...] -> [result]
                    let (name_idx, pos_count, kw_count) = self.current_frame.fetch_u16_u8_u8();
                    let name_id = StringId::from_index(name_idx);
                    let (pos_count, kw_count) = (pos_count as usize, kw_count as usize);

                    // Read keyword name StringIds
                    let mut kwname_ids = Vec::with_capacity(kw_count);
                    for _ in 0..kw_count {
                        kwname_ids.push(StringId::from_index(self.current_frame.fetch_u16()));
                    }

                    handle_call_result!(self, self.exec_call_attr_kw(name_id, pos_count, kwname_ids));
                }
                Opcode::CallFunctionExtended => {
                    let flags = self.current_frame.fetch_u8();
                    let has_kwargs = (flags & 0x01) != 0;

                    handle_call_result!(self, self.exec_call_function_extended(has_kwargs));
                }
                Opcode::CallAttrExtended => {
                    let (name_idx, flags) = self.current_frame.fetch_u16_u8();
                    let name_id = StringId::from_index(name_idx);
                    let has_kwargs = (flags & 0x01) != 0;

                    handle_call_result!(self, self.exec_call_attr_extended(name_id, has_kwargs));
                }
                // Function Definition
                Opcode::MakeFunction => {
                    let (func_idx, defaults_count) = self.current_frame.fetch_u16_u8();
                    let func_id = FunctionId::from_index(func_idx);
                    let defaults_count = defaults_count as usize;

                    if defaults_count == 0 {
                        // No defaults - use inline Value::Function (no heap allocation)
                        self.push(Value::DefFunction(func_id));
                    } else {
                        // Pop default values from stack (drain maintains order: first pushed = first in vec)
                        let defaults = self.pop_n(defaults_count);

                        // Create FunctionDefaults on heap and push reference
                        let heap_id = self
                            .heap
                            .allocate(HeapData::FunctionDefaults(FunctionDefaults { func_id, defaults }));
                        self.push(Value::Ref(heap_id));
                    }
                }
                Opcode::MakeClosure => {
                    let (func_idx, defaults_count, cell_count) = self.current_frame.fetch_u16_u8_u8();
                    let func_id = FunctionId::from_index(func_idx);
                    let (defaults_count, cell_count) = (defaults_count as usize, cell_count as usize);

                    // Pop cells from stack (pushed after defaults, so on top)
                    // Cells are Value::Ref pointing to HeapData::Cell
                    // We use individual pops which reverses order, so we need to reverse back
                    let mut cells = Vec::with_capacity(cell_count);
                    for _ in 0..cell_count {
                        // mut needed for dec_ref_forget when memory-model-checks feature is enabled
                        #[cfg_attr(not(feature = "memory-model-checks"), expect(unused_mut))]
                        let mut cell_val = self.pop();
                        match &cell_val {
                            Value::Ref(heap_id) => {
                                // Keep the reference - the Closure will own the HeapId
                                cells.push(*heap_id);
                                // Mark the Value as dereferenced since Closure takes ownership
                                // of the reference count (we don't call drop_with because
                                // we're not decrementing the refcount, just transferring it)
                                #[cfg(feature = "memory-model-checks")]
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
                    }));
                    self.push(Value::Ref(heap_id));
                }
                // Exception Handling
                Opcode::Raise => {
                    let exc = self.pop();
                    let error = self.make_exception(&exc, true); // is_raise=true, hide caret
                    // Re-raise an instance as-is so `raise e` preserves `e`'s
                    // identity, like CPython. A bare type or non-exception has
                    // nothing to reuse and rebuilds from the error.
                    let raised = match &exc {
                        Value::Ref(id) if matches!(self.heap.get(*id), HeapData::Exception(_)) => Some(exc),
                        _ => {
                            exc.drop_with(self);
                            None
                        }
                    };
                    if let Some(result) = self.handle_exception_with_value(error, raised) {
                        return Err(result);
                    }
                    yield_if_parked!(self);
                }
                Opcode::Assert => {
                    match decode_assert_flags(self.current_frame.fetch_u8()).expect("invalid assert flags in bytecode")
                    {
                        Some(op) => try_catch!(self, self.assert_cmp(op)),
                        None => try_catch!(self, self.assert_test()),
                    }
                }
                Opcode::AssertFailed => {
                    let cmp_op =
                        decode_assert_flags(self.current_frame.fetch_u8()).expect("invalid assert flags in bytecode");
                    let error = self.assert_failed_msg(cmp_op);
                    catch!(self, error);
                }
                Opcode::Reraise => {
                    // Clone rather than pop: a locally caught bare raise must
                    // preserve its enclosing handler's active exception.
                    let raised = self.exception_stack.last().map(|exc| exc.clone_with_heap(self.heap));
                    let error = match &raised {
                        Some(exc) => self.make_exception(exc, true), // is_raise=true for reraise
                        // No active exception - create a RuntimeError
                        None => {
                            SimpleException::new_msg(ExcType::RuntimeError, "No active exception to reraise").into()
                        }
                    };
                    if let Some(result) = self.handle_exception_with_value(error, raised) {
                        return Err(result);
                    }
                    yield_if_parked!(self);
                }
                Opcode::ClearException => {
                    // Pop the current exception from the stack
                    // This restores the previous exception context (if any)
                    if let Some(exc) = self.exception_stack.pop() {
                        exc.drop_with(self);
                    }
                }
                Opcode::CheckExcMatch => {
                    // Stack: [exception, exc_type] -> [exception, bool]
                    let exc_type = self.pop();
                    let exception = self.peek();
                    let result = self.check_exc_match(exception, &exc_type);
                    exc_type.drop_with(self);
                    match result {
                        Ok(matched) => self.push(Value::Bool(matched)),
                        // An invalid `except` type (e.g. `except 123:` or a
                        // nested tuple) raises a `TypeError`. As in CPython this
                        // is an ordinary exception raised while evaluating the
                        // clause: it propagates out of the whole `try` and may be
                        // caught by an enclosing handler, so route it through the
                        // exception machinery rather than aborting the run.
                        Err(err) => catch!(self, err),
                    }
                }
                // Return
                Opcode::ReturnValue => {
                    let value = self.pop();
                    if self.suspended_frames.is_empty() {
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
                            Ok(AwaitResult::FramePushed) => {}
                            Ok(AwaitResult::Yield(pending)) => {
                                // All tasks blocked - return to host
                                return Ok(FrameExit::ResolveFutures(pending));
                            }
                            Err(e) => catch!(self, e),
                        }
                        continue;
                    }
                    // Read the initializer flag before popping the frame.
                    let is_init = self.current_frame().is_initializer;
                    // Pop current frame; `stop` requests returning to the host
                    // (e.g. `evaluate_function`).
                    let stop = self.pop_frame();
                    if is_init {
                        if !matches!(value, Value::None) {
                            // CPython raises at the `Foo(...)` call site: the
                            // initializer frame is already popped, so the traceback
                            // matches (no `__init__` frame).
                            let type_name = value.py_type_name(self);
                            value.drop_with(self);
                            let err = ExcType::type_error_init_return(type_name);
                            if stop {
                                // The initializer was driven by `evaluate_function`
                                // and its frame boundary is already popped —
                                // propagate directly rather than unwinding into
                                // frames that must not observe this error. The
                                // pending instance left on the operand stack is
                                // reclaimed by the eventual `handle_exception`
                                // stack drain (or final teardown).
                                return Err(err);
                            }
                            catch!(self, err);
                            continue;
                        }
                        // `__init__` returned None — discard it. The instance was
                        // pushed onto the caller's stack before this frame ran and
                        // is the real result of `Foo(...)`.
                        value.drop_with(self);
                        if stop {
                            let instance = self.pop();
                            return Ok(FrameExit::Return(instance));
                        }
                        // Instance already on the caller's stack — push nothing.
                    } else if stop {
                        // This frame indicated evaluation should stop - return to host with value
                        // e.g. `evaluate_function`
                        return Ok(FrameExit::Return(value));
                    } else {
                        self.push(value);
                    }
                }
                // Async/Await
                Opcode::Await => {
                    let result = self.exec_get_awaitable();
                    match result {
                        Ok(AwaitResult::ValueReady(value)) => {
                            self.push(value);
                        }
                        Ok(AwaitResult::FramePushed) => {}
                        Ok(AwaitResult::Yield(pending_calls)) => {
                            // All tasks are blocked - return control to host
                            return Ok(FrameExit::ResolveFutures(pending_calls));
                        }
                        Err(e) => {
                            catch!(self, e);
                        }
                    }
                }
                // Unpacking - route through exception handling
                Opcode::UnpackSequence => {
                    let count = self.current_frame.fetch_u8() as usize;
                    try_catch!(self, self.unpack_sequence(count));
                }
                Opcode::UnpackEx => {
                    let (before, after) = self.current_frame.fetch_u8_u8();
                    try_catch!(self, self.unpack_ex(before as usize, after as usize));
                }
                // Special
                Opcode::Nop => {
                    // No operation
                }
                // Module Operations
                Opcode::LoadModule => {
                    let module_id = self.current_frame.fetch_u8();
                    self.load_module(module_id);
                }
                Opcode::RaiseImportError => {
                    // Fetch the module name from the constant pool and raise ModuleNotFoundError
                    let const_idx = self.current_frame.fetch_u16();
                    let module_name = self.current_frame.code.constants().get(const_idx);
                    // The constant should be an InternString from compile_import/compile_import_from
                    let name_str = match module_name {
                        Value::InternString(id) => self.interns.get_str(*id),
                        _ => "<unknown>",
                    };
                    let error = ExcType::module_not_found_error(name_str);
                    catch!(self, error);
                }
                // Context Managers
                Opcode::BeforeWith => {
                    handle_call_result!(self, self.exec_before_with());
                }
                Opcode::WithExit => {
                    handle_call_result!(self, self.exec_with_exit());
                }
                Opcode::WithExceptStart => {
                    handle_call_result!(self, self.exec_with_except_start());
                }
            }
        }
    }

    /// Loads a built-in module and pushes it onto the stack.
    fn load_module(&mut self, module_id: u8) {
        let module = StandardLib::from_repr(module_id).expect("unknown module id");

        // Create the module on the heap using pre-interned strings
        let heap_id = module.create(self);
        self.push(Value::Ref(heap_id));
    }

    /// Resumes execution after an external call completes.
    ///
    /// Pushes the return value onto the stack and continues execution.
    ///
    /// If the paused OS call has a pending effect, the result is routed
    /// through the corresponding helper (file-state update or `os.listdir`
    /// name reduction) before it is pushed back to Python.
    pub fn resume(&mut self, obj: MontyObject) -> Result<FrameExit, RunError> {
        // `ListdirNames` reshapes the raw host object *before* heap
        // conversion — plain data in, plain data out, no refcounts involved.
        let obj = if matches!(self.pending_os_effect, Some(PendingOsEffect::ListdirNames)) {
            self.pending_os_effect = None;
            match listdir_names(obj) {
                Ok(obj) => obj,
                Err(err) => return self.resume_with_exception(err),
            }
        } else {
            obj
        };
        // Surface resource-exhaustion failures from `to_value` (e.g. a host
        // string whose `heap.allocate` trips `max_memory`) as the same
        // `RunError::Resource` that pure-Monty allocations produce, so the
        // user sees `MemoryError` instead of `RuntimeError: invalid return
        // type`. Other input errors stay as `RuntimeError`.
        let value = obj.to_value(self).map_err(|e| match e {
            InvalidInputError::Resource(err) => RunError::from(err),
            other @ InvalidInputError::InvalidType(_) => {
                SimpleException::new(ExcType::RuntimeError, Some(format!("invalid return type: {other}"))).into()
            }
        })?;
        if let Some(effect) = self.pending_os_effect.take() {
            let result = match effect {
                PendingOsEffect::BufferStore { file_id } => apply_buffer_store(file_id, value, self),
                PendingOsEffect::WritePosition { file_id, .. } => apply_write_position(file_id, value, self),
                // Cleared above, before conversion.
                PendingOsEffect::ListdirNames => unreachable!("ListdirNames is handled before heap conversion"),
            };
            match result {
                Ok(value) => {
                    self.push(value);
                    self.run_external()
                }
                Err(err) => self.resume_with_exception(err),
            }
        } else {
            self.push(value);
            self.run_external()
        }
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
    ///
    /// Also clears any pending file effect so user code that catches a
    /// host-side OS exception can retry without stale in-flight state.
    pub fn resume_with_exception(&mut self, error: RunError) -> Result<FrameExit, RunError> {
        if let Some(effect) = self.pending_os_effect.take() {
            match effect {
                PendingOsEffect::BufferStore { file_id } => {
                    if let HeapReadOutput::OpenFile(mut file) = self.heap.read(file_id) {
                        file.get_mut(self.heap).clear_pending_read();
                        drop(file);
                    }
                    self.heap.dec_ref(file_id);
                }
                PendingOsEffect::WritePosition {
                    file_id,
                    previous_position,
                    previous_length,
                } => {
                    if let HeapReadOutput::OpenFile(mut file) = self.heap.read(file_id) {
                        file.get_mut(self.heap)
                            .rollback_write_position(previous_position, previous_length);
                        drop(file);
                    }
                    self.heap.dec_ref(file_id);
                }
                // Holds no state or heap references — nothing to roll back.
                PendingOsEffect::ListdirNames => {}
            }
        }
        // Use the normal exception handling mechanism
        // handle_exception returns None if caught, Some(error) if not caught
        if let Some(uncaught_error) = self.handle_exception(error) {
            return Err(uncaught_error);
        }
        yield_if_parked!(self);
        // Exception was caught, continue execution
        self.run_external()
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

    /// Returns the frame currently executing, or the parked placeholder.
    #[inline]
    pub(crate) fn current_frame(&self) -> &CallFrame<'h> {
        &self.current_frame
    }

    /// Returns mutable access to the current frame.
    #[inline]
    pub(super) fn current_frame_mut(&mut self) -> &mut CallFrame<'h> {
        &mut self.current_frame
    }

    /// Pushes the given frame onto the call stack.
    ///
    /// Returns an error if the recursion depth limit is exceeded by pushing this frame.
    pub(super) fn push_frame(&mut self, frame: CallFrame<'h>) -> RunResult<()> {
        if !self.current_frame.is_parked
            && let Err(e) = self.incr_recursion()
        {
            self.cleanup_frame_state(&frame);
            return Err(e.into());
        }
        let caller = mem::replace(&mut self.current_frame, frame);
        self.suspended_frames.push(caller);
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
        let caller = self.suspended_frames.pop().expect("cannot pop the root frame");
        let frame = mem::replace(&mut self.current_frame, caller);
        self.cleanup_frame_state(&frame);
        // Sync instruction_ip to the restored caller so exception table lookups
        // target the correct frame after returning from a nested run() call.
        self.instruction_ip = self.current_frame.ip;
        if !self.current_frame.is_parked {
            self.decr_recursion();
        }
        frame.should_return
    }

    fn cleanup_frame_state(&mut self, frame: &CallFrame<'_>) {
        // Clean up frame's stack region (locals + operand stack, which now
        // includes any in-flight comprehension variables — the operand-stack
        // drain naturally covers them).
        self.stack
            .drain(frame.stack_base..)
            .for_each(|value| value.drop_with(&mut *self.heap));
    }

    /// Cleans up all frames and stack values for the current task.
    ///
    /// Used when a task completes or fails and we need to switch to another task.
    /// Drains the stack with proper `drop_with`, then leaves a non-executing
    /// parked frame until another task is loaded.
    pub(super) fn cleanup_current_task(&mut self) {
        self.stack.drain(..).drop_with(self.heap);
        self.suspended_frames.clear();
        let code = self.module_code.unwrap_or(self.current_frame.code);
        self.current_frame = CallFrame::new_parked(code);
    }

    /// Runs the trial-deletion cycle collector.
    ///
    /// Roots are not enumerated by the VM: every value held in the stack,
    /// globals, exception stack, scheduler tasks, JSON-string cache, etc.
    /// already keeps its referent alive via refcount, so the collector treats
    /// any non-zero refcount as proof of liveness. See
    /// [`Heap::collect_cycles`].
    ///
    /// Returns the number of unreachable heap entries freed during the sweep.
    fn run_gc(&mut self) -> usize {
        self.heap.collect_cycles()
    }

    /// Forces a GC cycle and returns the freed count.
    ///
    /// This is only compiled for tests so integration tests can reproduce GC
    /// bugs deterministically.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn __force_gc_for_tests(&mut self) -> usize {
        self.run_gc()
    }

    /// Releases everything the scheduler still holds, as dropping the VM
    /// would.
    ///
    /// A run can end with tasks still live — a task detached from a failed
    /// gather outlives it, and the module can return before that task next
    /// gets a turn. Their references are real, so the leak check in
    /// `run_ref_counts_with_tracker` has to run *after* this rather than
    /// count them as unreachable.
    #[cfg(feature = "ref-count-return")]
    pub(crate) fn __finalize_tasks_for_tests(&mut self) {
        self.cleanup_current_task();
        self.scheduler.cleanup(self.heap);
    }

    /// Hands the surviving tasks' pending calls back to the host, for
    /// [`yield_if_parked`] when dispatch has no frame left to run.
    ///
    /// Those tasks are parked on external calls, so there is normally
    /// something to hand over. With nothing pending there is no way forward
    /// either: resuming would fail the same way one round-trip later, blaming
    /// the scheduler rather than the discarded task that emptied it.
    fn yield_parked(&self) -> Result<FrameExit, RunError> {
        let pending_call_ids = self.scheduler.pending_call_ids();
        if pending_call_ids.is_empty() {
            Err(RunError::internal(
                "asyncio scheduler stalled: exception discarded with no task to run and no pending external calls",
            ))
        } else {
            Ok(FrameExit::ResolveFutures(pending_call_ids))
        }
    }

    /// Returns the source position for the instruction currently executing.
    pub(super) fn current_position(&self) -> CodeRange {
        self.current_frame
            .code
            .location_for_offset(self.instruction_ip)
            .map(LocationEntry::range)
            .unwrap_or_default()
    }

    /// Captures the caller's current bytecode offset for a call site, or `None`
    /// while the VM is parked with no active Python task.
    ///
    /// The cheap counterpart to [`current_position`](Self::current_position):
    /// no location-table scan, so it is affordable on every call. Out-of-range
    /// offsets (an invariant violation) degrade to `None` rather than panic.
    pub(super) fn current_offset(&self) -> Option<u32> {
        if self.current_frame.is_parked {
            None
        } else {
            u32::try_from(self.instruction_ip).ok()
        }
    }

    /// Resolves a raw caller offset (`CallFrame::call_offset`) to a source
    /// [`CodeRange`] against the current frame's code, during traceback unwind
    /// once the failing frame has been popped so the current frame is the caller.
    pub(super) fn resolve_offset(&self, offset: u32) -> CodeRange {
        self.current_frame
            .code
            .location_for_offset(offset as usize)
            .map(LocationEntry::range)
            .unwrap_or_default()
    }

    // ========================================================================
    // Variable Operations
    // ========================================================================

    /// Loads a local variable and pushes it onto the stack.
    ///
    /// Raises `UnboundLocalError` if the slot holds `Undefined` — every reachable
    /// `LoadLocal*` slot is registered as assigned by the compiler, so an undefined
    /// value can only mean access-before-assignment.
    fn load_local(&mut self, slot: u16) -> RunResult<()> {
        let index = self.current_frame.stack_base + slot as usize;
        if matches!(self.stack[index], Value::Undefined) {
            let name = self.current_frame.code.local_name(slot);
            Err(self.unbound_local_error(slot, name))
        } else {
            let value = self.stack[index].clone_with_heap(self.heap);
            self.push(value);
            Ok(())
        }
    }

    /// Loads a global variable in call context, pushing an external function for undefined names.
    ///
    /// Unlike `load_global`, this never yields `NameLookup`. When the variable is undefined,
    /// it allocates an external function so that the subsequent `CallFunction` opcode can
    /// yield `FunctionCall` instead. Before doing so it tries the builtin fallback
    /// (see [`builtin_for_name`]) so `f()` style calls into a builtin still work when
    /// the name happens to have a module slot allocated (e.g. because the module also
    /// `def`-binds the same name elsewhere) but that slot is currently `Undefined`.
    fn load_global_callable(&mut self, slot: u16, name_id: StringId) {
        let value = self.globals[slot as usize].clone_with_heap(self);

        if matches!(value, Value::Undefined) {
            if let Some(builtin) = self.builtin_for_name(name_id) {
                self.push(builtin);
                return;
            }
            // A reserved module dunder (e.g. `__name__`) in call position resolves
            // to its fixed value; the subsequent call then fails with the usual
            // "object is not callable" error, matching CPython.
            if let Some(value) = self.module_dunder(name_id) {
                self.push(value);
                return;
            }
            // Save the load instruction's IP so NameError tracebacks point to the name
            self.ext_function_load_ip = Some(self.instruction_ip);
            let function = self.heap.get_ext_function(self.interns.get_str(name_id));
            self.push(function);
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

    /// Returns the builtin value for a name, if the name happens to match a builtin.
    ///
    /// Function-level global reads fall back to builtins at runtime when the module
    /// slot is `Undefined`, mirroring CPython's `globals → builtins` lookup order.
    /// Function-scope name resolution never substitutes `Expr::Builtin` at parse time
    /// (see `prepare::resolve_name_or_builtin`), so this fallback is also what makes
    /// late-binding patterns work:
    ///
    /// ```python
    /// def f(): return sum(...)   # uses builtin sum
    /// f()
    /// def sum(*args): return 42  # later module-level shadow
    /// f()                        # picks up the user-defined sum
    /// ```
    ///
    /// The first call hits `globals[sum_slot] = Undefined` and falls back here to
    /// the builtin; once `def sum` runs, the slot holds the user function and the
    /// fallback isn't taken anymore.
    fn builtin_for_name(&self, name_id: StringId) -> Option<Value> {
        Builtins::value_from_name(self.interns.get_str(name_id))
    }

    /// Returns the fixed value for a module-level dunder, or `None` if `name_id`
    /// does not name one of [`RESERVED_MODULE_DUNDERS`](crate::bytecode::RESERVED_MODULE_DUNDERS).
    ///
    /// Backs the read side of those dunders: `__name__` is always `'__main__'`
    /// (Monty only ever runs a top-level module) and `__debug__` is `True`
    /// (asserts always run). `__doc__`/`__spec__`/`__package__` default to
    /// `None` and `__annotations__` to a fresh empty dict — module-level
    /// annotations are not stored (see `limitations/typing.md`), so it is
    /// always empty. `__loader__` is deliberately *not* exposed: CPython only
    /// ever puts a loader object there (never `None`), so rather than diverge
    /// on the type we let it raise `NameError` like other unexposed dunders
    /// (`__file__`, `__cached__`, …).
    fn module_dunder(&self, name_id: StringId) -> Option<Value> {
        let value = match self.interns.get_str(name_id) {
            "__name__" => Value::InternString(StaticStrings::DunderMain.into()),
            "__debug__" => Value::Bool(true),
            "__annotations__" => Value::Ref(self.heap.allocate(HeapData::Dict(Dict::new()))),
            "__doc__" | "__spec__" | "__package__" => Value::None,
            _ => return None,
        };
        Some(value)
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
    fn store_local(&mut self, slot: u16) {
        let value = self.pop();
        let index = self.current_frame.stack_base + slot as usize;
        let old_value = mem::replace(&mut self.stack[index], value);
        old_value.drop_with(self);
    }

    /// Deletes a local variable (sets it to Undefined).
    fn delete_local(&mut self, slot: u16) {
        let index = self.current_frame.stack_base + slot as usize;
        let old_value = mem::replace(&mut self.stack[index], Value::Undefined);
        old_value.drop_with(self);
    }

    /// Loads a global variable and pushes it onto the stack.
    ///
    /// When the variable is undefined, falls back to builtin resolution (see
    /// [`builtin_for_name`]) before yielding `NameLookup` so the host can supply
    /// an external binding.
    fn load_global(&mut self, slot: u16) -> Result<Option<FrameExit>, RunError> {
        let value = self.globals[slot as usize].clone_with_heap(self);

        // Check for undefined value — raise appropriate error or yield to host
        if matches!(value, Value::Undefined) {
            let name = self.global_name(slot);

            let Some(name_id) = name else {
                // No name available — raise NameError directly
                return Err(self.name_error(slot, None));
            };
            if let Some(builtin) = self.builtin_for_name(name_id) {
                self.push(builtin);
                return Ok(None);
            }
            if let Some(value) = self.module_dunder(name_id) {
                self.push(value);
                return Ok(None);
            }
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

    /// Returns the interned name of a module-level global at `slot`, if known.
    ///
    /// Returns `None` if no module code is attached (test harness use of
    /// `VM::new` without `run_module`) or if the slot is past the recorded
    /// name table.
    fn global_name(&self, slot: u16) -> Option<StringId> {
        self.module_code.and_then(|c| c.local_name(slot))
    }

    /// Pops the top of stack and stores it in a global variable.
    ///
    /// Reassigning a reserved module dunder (see [`RESERVED_MODULE_DUNDERS`]) is
    /// rejected at compile time (see `Compiler::compile_store`), so no name
    /// check is needed here.
    fn store_global(&mut self, slot: u16) {
        let value = self.pop();
        let old_value = mem::replace(&mut self.globals[slot as usize], value);
        old_value.drop_with(self);
    }

    /// Deletes a global variable (sets it to `Undefined`).
    ///
    /// Raises `NameError` if the slot is already `Undefined`.
    fn delete_global(&mut self, slot: u16) -> RunResult<()> {
        // TODO: the `Undefined` branch is currently unreachable from Python source,
        // needs support for the `del` statement.
        if matches!(self.globals[slot as usize], Value::Undefined) {
            let name = self.global_name(slot);
            return Err(self.name_error(slot, name));
        }
        let old_value = mem::replace(&mut self.globals[slot as usize], Value::Undefined);
        old_value.drop_with(self);
        Ok(())
    }

    /// Loads from a closure cell and pushes onto the stack.
    ///
    /// The cell `HeapId` is read from the frame's local variable slot on the stack
    /// (cells are stored as `Value::Ref(cell_id)` at known positions in the locals region).
    /// Returns a `NameError` if the cell value is undefined (free variable not bound).
    fn load_cell(&mut self, slot: u16) -> RunResult<()> {
        let cell_id = self.cell_id_from_local(slot);
        let value = match self.heap.get(cell_id) {
            HeapData::Cell(c) => c.0.clone_with_heap(self),
            _ => panic!("LoadCell: entry is not a Cell"),
        };

        // An undefined value raises the error CPython picks by cell kind: the
        // free-variable NameError only for a cell *captured* from an enclosing
        // function; an unbound cell this frame owns (a local captured by
        // nested functions) is an ordinary UnboundLocalError, like any local.
        if matches!(value, Value::Undefined) {
            value.drop_with(self);
            let name = self.current_frame.code.local_name(slot);
            Err(if self.is_free_var_slot(slot) {
                self.free_var_error(name)
            } else {
                self.unbound_local_error(slot, name)
            })
        } else {
            self.push(value);
            Ok(())
        }
    }

    /// Extracts the cell `HeapId` from a local variable slot on the stack.
    ///
    /// Cell variables are stored as `Value::Ref(cell_id)` in the frame's locals region.
    fn cell_id_from_local(&self, slot: u16) -> HeapId {
        match &self.stack[self.current_frame.stack_base + slot as usize] {
            Value::Ref(cell_id) => *cell_id,
            other => panic!("LoadCell/StoreCell: expected cell reference in local slot {slot}, found {other:?}"),
        }
    }

    /// Whether `slot` holds a cell captured from an enclosing function (a
    /// free variable), as opposed to a cell this frame owns. Module frames
    /// (`function_id: None`) own all their cells — the only module-level
    /// cells are inlined-comprehension captures.
    fn is_free_var_slot(&self, slot: u16) -> bool {
        self.current_frame().function_id.is_some_and(|id| {
            self.interns
                .get_function(id)
                .free_var_slots
                .iter()
                .any(|s| s.as_u16() == slot)
        })
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
    fn store_cell(&mut self, slot: u16) {
        let value = self.pop();
        // The guard will clean up the new value if we panic, or the old value if we swap
        let this = self;
        defer_drop_mut!(value, this);

        let cell_id = this.cell_id_from_local(slot);
        let HeapReadOutput::Cell(mut cell) = this.heap.read(cell_id) else {
            panic!("StoreCell: entry is not a Cell")
        };
        mem::swap(&mut cell.get_mut(this.heap).0, value);
    }

    /// Unbinds a closure cell: replaces its contents with `Undefined`, so a
    /// later [`Self::load_cell`] raises the free-variable `NameError` —
    /// CPython's `DELETE_DEREF` cleanup of a captured `except ... as` target.
    /// The only emitter stores `None` first, so the cell is never already
    /// unbound here (no error path, unlike [`Self::delete_global`]).
    fn delete_cell(&mut self, slot: u16) {
        let value = Value::Undefined;
        // the guard drops the cell's previous contents after the swap
        let this = self;
        defer_drop_mut!(value, this);

        let cell_id = this.cell_id_from_local(slot);
        let HeapReadOutput::Cell(mut cell) = this.heap.read(cell_id) else {
            panic!("DeleteCell: entry is not a Cell")
        };
        mem::swap(&mut cell.get_mut(this.heap).0, value);
    }
}

// `heap` is not a public field on VM, so this implementation needs to go here rather than in `heap.rs`
impl ContainsHeap for VM<'_> {
    fn heap(&self) -> &Heap {
        self.heap
    }
    fn heap_mut(&mut self) -> &mut Heap {
        self.heap
    }
}

/// Ensures proper reference-counting cleanup when the VM goes out of scope.
///
/// Drains exception stack, operand stack, globals, scheduler state, and JSON
/// string cache — all of which may hold heap references that need their
/// ref-counts decremented. Fields that were already emptied (e.g. by
/// `take_globals`) are harmlessly drained as empty.
impl Drop for VM<'_> {
    fn drop(&mut self) {
        release_pending_effect(self.pending_os_effect.take(), self.heap);
        self.pending_lookup_effect.take().drop_with(self.heap);
        self.exception_stack.drain(..).drop_with(self.heap);
        self.cleanup_current_task();
        self.scheduler.cleanup(self.heap);
        self.globals.drain(..).drop_with(self.heap);
        self.json_string_cache.drop_all(self.heap);
    }
}
