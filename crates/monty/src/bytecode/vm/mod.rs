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
mod dispatch_threaded;
mod exceptions;
mod format;
mod module_cache;
mod module_finder;
mod namespace;
mod pattern;
mod recursion;
mod scheduler;

use std::{borrow::Cow, mem};

pub(crate) use attr::PendingLookupEffect;
pub(crate) use call::CallResult;
pub(crate) use collections::unpack_exact;
use exceptions::CaughtException;
pub(crate) use module_cache::ModuleCache;
pub(crate) use module_finder::PendingImport;
use monty_types::{InvalidInputError, MontyObject, MontyUuid, OsFunctionCall, PrintWriter, SourceRange};
pub(crate) use namespace::{FrameGlobals, FrameNamespace, MissingNames, function_namespace};
use recursion::frame_recursion_depth;
pub(crate) use recursion::{ContainsVM, RecursionToken, RunReentryGuard};
use scheduler::Scheduler;

use crate::{
    args::ArgValues,
    asyncio::{CallId, TaskId},
    builtins::Builtins,
    bytecode::{
        code::{Code, LocationEntry},
        op::{MATCH_KEYS_REST, Opcode, decode_assert_flags},
    },
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, Heap, HeapData, HeapId, HeapReadOutput, HeapReader},
    heap_data::{CellValue, Closure, FunctionDefaults},
    intern::{FunctionId, Interns, StaticStrings, StringId},
    modules::{
        StandardLib,
        json::{JsonStringCache, apply_json_load},
        random::apply_seed_random,
        re::RePatternCache,
        time::apply_clock_reading,
    },
    name_map::NameMap,
    object_bridge::MontyObjectExt,
    os_dispatch::{
        PendingEffect, PostConversionEffect, PreConversionEffect, Reshaped, check_raw_call_paths,
        release_pending_effect, resolve_call_paths, urandom_reply_error,
    },
    parse::CodeRange,
    py_value::PyValue,
    run::{Program, SessionTables, VmEnv},
    types::{
        Dict, PyTrait, SessionRandom,
        file::{apply_buffer_load, apply_buffer_store, apply_file_next, apply_open_name, apply_write_position},
        random::SEED_BYTES,
        str::allocate_string,
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

/// Yields when an unhandled task exception leaves `cleanup_current_task`'s
/// parked frame with no successor. Dispatch must not execute that frame.
/// [`VM::pending_futures_exit`] returns surviving tasks' pending calls.
macro_rules! yield_if_parked {
    ($self:expr) => {
        if $self.current_frame.is_parked {
            return $self.pending_futures_exit();
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
        /// [`VM::pending_effect`] only once the call reaches the host
        /// (`convert_frame_exit`); dropping the exit releases it instead.
        effect: Option<PendingEffect>,
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
    /// Compiled body, stable even when runtime compilation publishes more functions.
    code: &'code Code,

    /// Hoisted instruction slice, avoiding a `Code` dereference on every fetch.
    bytecode: &'code [u8],

    /// Instruction pointer within this frame's bytecode.
    ip: usize,

    /// Base index into the VM stack for this frame's locals region.
    ///
    /// The frame's locals occupy `stack[stack_base..stack_base + locals_count]`,
    /// and operands are pushed above that. `u32` keeps the frame small; the
    /// stack can never hold that many values.
    stack_base: u32,

    /// Number of local variable slots in this frame.
    ///
    /// Zero for module-level frames (globals are stored separately).
    /// For function frames, this equals `func.namespace_size`.
    locals_count: u16,

    /// Base of this frame's entries in the VM-wide `exception_stack`.
    /// Recorded region depths are relative to this index, keeping caller
    /// exceptions intact when abandoned handlers are unwound.
    exception_stack_base: u32,

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

    /// Where names that are not stack slots resolve; `None` for every
    /// ordinary frame. Owns its dict references: released by
    /// `cleanup_frame_state`, or taken by `serialize` for a snapshot.
    namespace: Option<Box<FrameNamespace>>,

    /// Whether this frame is a class `__init__` running for `Foo(...)`, or the
    /// body of a module imported from a file.
    ///
    /// When `true`, the `ReturnValue` handler discards the frame's return value
    /// (`None`) and leaves the instance or module — pushed onto the caller's
    /// operand stack before this frame was created — as the result of the
    /// construction or import. Threaded through serialization (`SerializedFrame`)
    /// so a suspended initializer resumes correctly.
    is_initializer: bool,

    /// Module name for `LoadAttrImport` errors, set by `LoadModule`/`LoadSubmodule`.
    /// Keeping it on the frame avoids another opcode operand; the compiler emits
    /// no intervening instructions that change it.
    import_from_module: Option<StringId>,
}

/// Narrows a VM stack index to the frame's `u32` field.
/// Panics instead of truncating an index beyond the frame representation.
#[inline]
pub(super) fn stack_index(index: usize) -> u32 {
    u32::try_from(index).expect("VM stack index exceeds u32")
}

/// The code a saved frame runs: its function's, or `module_code` for the
/// module-level frame, which has no function ID.
pub(super) fn frame_code<'code>(
    interns: &'code Interns,
    module_code: &'code Code,
    function_id: Option<FunctionId>,
) -> &'code Code {
    function_id.map_or(module_code, |id| &interns.get_function(id).code)
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
            exception_stack_base: stack_index(exception_stack_base),
            function_id: None,
            call_offset: None,
            should_return: false,
            is_parked: false,
            namespace: None,
            is_initializer: false,
            import_from_module: None,
        }
    }

    /// Creates a non-executing frame for a VM with no active Python task.
    fn new_parked(code: &'code Code) -> Self {
        let mut frame = Self::new_module(code, 0);
        frame.is_parked = true;
        frame
    }

    /// Parks this finished frame between tasks; its namespace must already be released.
    fn park(&mut self, module_code: &'code Code) {
        self.code = module_code;
        self.bytecode = module_code.bytecode();
        self.ip = 0;
        self.stack_base = 0;
        self.locals_count = 0;
        self.exception_stack_base = 0;
        self.function_id = None;
        self.call_offset = None;
        self.should_return = false;
        self.is_parked = true;
        self.is_initializer = false;
        debug_assert!(self.namespace.is_none(), "parked frame still owns a namespace");
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
        namespace: Option<Box<FrameNamespace>>,
    ) -> Self {
        Self {
            code,
            bytecode: code.bytecode(),
            ip: 0,
            stack_base: stack_index(stack_base),
            locals_count,
            exception_stack_base: stack_index(exception_stack_base),
            function_id: Some(function_id),
            call_offset,
            should_return: false,
            is_parked: false,
            namespace,
            is_initializer: false,
            import_from_module: None,
        }
    }
}

impl CallFrame<'_> {
    /// Fetches `N` operand bytes with a single bounds check and advances the frame IP.
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

    /// Fetches two little-endian `u16`s followed by a `u8`, in a single bounds check.
    ///
    /// Mirrors `CodeBuilder::emit_name_op` on the encode side.
    #[inline]
    fn fetch_u16_u16_u8(&mut self) -> (u16, u16, u8) {
        let [slot_lo, slot_hi, name_lo, name_hi, flags] = self.fetch_array();
        (
            u16::from_le_bytes([slot_lo, slot_hi]),
            u16::from_le_bytes([name_lo, name_hi]),
            flags,
        )
    }

    /// Start of this frame's locals region on the VM stack.
    #[inline]
    pub(super) fn stack_base(&self) -> usize {
        self.stack_base as usize
    }

    /// Start of this frame's entries in the VM-wide `exception_stack`.
    #[inline]
    pub(super) fn exception_stack_base(&self) -> usize {
        self.exception_stack_base as usize
    }
}

/// Serializable representation of a call frame.
///
/// Cannot store `&Code` (a reference) — instead stores `FunctionId` to look up
/// the pre-compiled Code object on resume. Module-level code uses `None`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SerializedFrame {
    /// Which function's code this frame executes (None = module-level).
    #[serde(rename = "F")]
    function_id: Option<FunctionId>,

    /// Instruction pointer within this frame's bytecode.
    #[serde(rename = "P")]
    ip: usize,

    /// Base index into the VM stack for this frame's locals region.
    #[serde(rename = "S")]
    stack_base: usize,

    /// Number of local variable slots (0 for module-level frames).
    #[serde(rename = "L")]
    locals_count: u16,

    /// Base index into the VM-wide `exception_stack` for this frame.
    /// See `CallFrame.exception_stack_base`.
    #[serde(rename = "E")]
    exception_stack_base: usize,

    /// Caller's bytecode offset at the call site (for tracebacks). See
    /// `CallFrame.call_offset`.
    #[serde(rename = "C")]
    call_offset: Option<u32>,

    /// Whether this frame is a class `__init__` or an imported module body (see `CallFrame.is_initializer`).
    ///
    /// Unlike `should_return`, an initializer frame can legitimately be live
    /// across a suspend (an `__init__` that calls an external/OS function), so it
    /// must round-trip — otherwise the resumed frame would push `__init__`'s
    /// `None` instead of leaving the instance on the stack.
    #[serde(rename = "I")]
    is_initializer: bool,

    /// Frame namespace, with ownership of its dict references (see
    /// `CallFrame.namespace`).
    #[serde(rename = "N")]
    namespace: Option<Box<FrameNamespace>>,

    /// The `from ... import` in progress, if the frame suspended inside one.
    #[serde(default, rename = "M")]
    import_from_module: Option<StringId>,
}

impl CallFrame<'_> {
    /// Converts this frame to a serializable representation, moving the
    /// namespace's owned references across so the live frame releases nothing.
    fn serialize(&mut self) -> SerializedFrame {
        assert!(!self.is_parked, "cannot serialize a parked frame");
        assert!(
            !self.should_return,
            "cannot serialize frame marked for return - not yet supported"
        );
        SerializedFrame {
            function_id: self.function_id,
            ip: self.ip,
            stack_base: self.stack_base(),
            locals_count: self.locals_count,
            exception_stack_base: self.exception_stack_base(),
            call_offset: self.call_offset,
            is_initializer: self.is_initializer,
            namespace: mem::take(&mut self.namespace),
            import_from_module: self.import_from_module,
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
    exception_stack: Vec<CaughtException>,

    /// IP of the instruction that caused the pause (for exception handling).
    instruction_ip: usize,

    /// Scheduler state (always present).
    ///
    /// Contains call ID counter, task state, pending calls, and resolved futures.
    scheduler: Scheduler,

    /// In-flight resume effect for the paused OS call, if any. See
    /// [`VM::pending_effect`].
    pending_effect: Option<PendingEffect>,
    /// In-flight resume effect for the paused lazy attribute lookup, if any.
    /// See [`VM::pending_lookup_effect`].
    pending_lookup_effect: Option<PendingLookupEffect>,

    /// Working directory at the pause, including any `os.chdir` so far.
    cwd: String,
    /// The session's `random` state at the pause.
    random: SessionRandom,
    /// The session's file modules at the pause.
    #[serde(default)]
    modules: ModuleCache,
}

/// What an abandoned snapshot hands back to its REPL session: the state that
/// outlives a snippet.
pub(crate) struct AbandonedState {
    pub globals: Vec<Value>,
    pub cwd: String,
    pub random: SessionRandom,
    pub modules: ModuleCache,
}

impl VMSnapshot {
    /// Discards the in-flight execution state of a snapshot that will never be
    /// restored, releasing every heap reference it holds (operand and exception
    /// stacks, scheduler tasks, pending resume effects), and returns the
    /// globals, working directory, `random` generator and file modules so an
    /// abandoned REPL snippet keeps its namespace, any `os.chdir` it made, any
    /// seed it set and any module it finished importing. Mirrors `VM::drop`.
    pub(crate) fn abandon(self, heap: &mut Heap) -> AbandonedState {
        let Self {
            stack,
            globals,
            frames,
            exception_stack,
            mut scheduler,
            pending_effect,
            pending_lookup_effect,
            cwd,
            random,
            mut modules,
            ..
        } = self;
        HeapReader::with(heap, &mut (), |heap, ()| {
            release_pending_effect(pending_effect, heap);
            pending_lookup_effect.drop_with(heap);
            exception_stack.drop_with(heap);
            stack.drop_with(heap);
            for frame in frames {
                frame.namespace.drop_with(heap);
            }
            scheduler.cleanup(heap);
            // a module body interrupted here must run again at the next import
            modules.remove_unfinished(heap);
        });
        AbandonedState {
            globals,
            cwd,
            random,
            modules,
        }
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

    /// Stable committed entries, which remain borrowable across runtime compilation.
    pub(crate) interns: &'h Interns,

    /// Module-level global names, slot by slot; extended alongside
    /// [`globals`](Self::globals) when runtime-compiled code binds a new name.
    pub(crate) global_names: &'h mut NameMap,

    /// Print output writer, borrowed so callers retain access to collected output.
    pub(crate) print_writer: PrintWriter<'h>,

    /// Stack of exceptions being handled for nested except blocks.
    ///
    /// Used by bare `raise` to re-raise the current exception.
    /// When entering an except handler, the exception is pushed onto this stack.
    /// When exiting via `ClearException`, the top is popped. This allows nested
    /// except handlers to restore the outer exception context.
    exception_stack: Vec<CaughtException>,

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
    module_code: &'h Code,

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
    pub(crate) pending_effect: Option<PendingEffect>,

    /// How the paused lazy attribute lookup's answer is consumed on `resume`
    /// (`hasattr()` / `getattr()` default), armed like
    /// [`pending_effect`](Self::pending_effect) once the lookup reaches
    /// the host; `None` for `obj.attr` and when nothing is in flight.
    pub(crate) pending_lookup_effect: Option<PendingLookupEffect>,

    /// Current recursion depth — charged by non-frozen call frames and by nested
    /// data-structure traversals (`repr`/`eq`/`cmp`/`hash`, json, ...).
    ///
    /// See [`recursion`](self::recursion) for the guard/token primitives that
    /// maintain it. Not serialized: it is reconstructed from the active frame
    /// count, excluding root and frozen frames, on `restore` and async switches.
    recursion_depth: usize,

    /// Reusable scratch buffer for building a sync call's locals, avoiding a
    /// `malloc`/`free` per call. Only held transiently within
    /// `call_sync_function`, so one shared buffer is safe under recursion.
    namespace_scratch: Vec<Value>,
    /// Remaining native stack budget for `evaluate_function`, partials and callable-instance dispatch.
    /// Not serialized: nested runs cannot suspend, and callable wrappers release their
    /// charge before returning a suspending `CallResult`. Checked by [`Self::snapshot`].
    run_reentry_depth: u8,

    /// Per-run cache of compiled patterns for module-level `re.*` calls. Not
    /// snapshotted (a pure performance cache), so default-initialized on restore.
    pub(crate) re_pattern_cache: RePatternCache,

    /// Module generator and state for initializing unseeded generators.
    /// Preserved across snapshots and REPL feeds, including `random.seed()` changes.
    pub(crate) random: SessionRandom,

    /// Modules imported from files, preserved across snapshots and REPL feeds.
    pub(crate) modules: ModuleCache,

    /// Working directory, `__file__` inputs and the assert-repr cap for this
    /// run. Rebuilt from the executor on restore, except the working
    /// directory, which travels in the snapshot because `os.chdir` may have
    /// moved it.
    pub(crate) env: VmEnv<'h>,
}

impl<'h> VM<'h> {
    /// Creates a new VM ready to run `program`'s module code.
    ///
    /// The global-name map is borrowed mutably; committed intern entries and
    /// `program` remain shared while runtime compilation appends new entries.
    pub fn new(
        globals: Vec<Value>,
        tables: &'h mut SessionTables,
        program: &'h Program,
        heap: &'h mut HeapReader<'h>,
        print_writer: PrintWriter<'h>,
    ) -> Self {
        let SessionTables { global_names, interns } = tables;
        Self {
            stack: Vec::with_capacity(64),
            globals,
            current_frame: CallFrame::new_module(&program.module_code, 0),
            suspended_frames: Vec::with_capacity(16),
            heap,
            interns,
            global_names,
            print_writer,
            exception_stack: Vec::new(),
            instruction_ip: 0,
            scheduler: Scheduler::new(),
            ext_function_load_ip: None, // Set by LoadGlobalCallable
            module_code: &program.module_code,
            json_string_cache: JsonStringCache::default(),
            pending_effect: None,
            pending_lookup_effect: None,
            recursion_depth: 0,
            namespace_scratch: Vec::new(),
            run_reentry_depth: recursion::MAX_RUN_REENTRY_DEPTH,
            re_pattern_cache: RePatternCache::default(),
            random: SessionRandom::default(),
            modules: ModuleCache::default(),
            env: program.vm_env(),
        }
    }

    /// Reconstructs a VM from a snapshot.
    ///
    /// The heap must already be deserialized. `FunctionId` values in frames
    /// are used to look up pre-compiled `Code` objects from the intern table;
    /// `program.module_code` serves frames with `function_id = None`. The
    /// snapshot's working directory overrides the program's.
    pub fn restore(
        snapshot: VMSnapshot,
        tables: &'h mut SessionTables,
        program: &'h Program,
        heap: &'h mut HeapReader<'h>,
        print_writer: PrintWriter<'h>,
    ) -> Self {
        let SessionTables { global_names, interns } = tables;
        // Reconstruct call frames from serialized form
        let frames: Vec<CallFrame<'_>> = snapshot
            .frames
            .into_iter()
            .map(|sf| {
                let code = frame_code(interns, &program.module_code, sf.function_id);
                CallFrame {
                    code,
                    bytecode: code.bytecode(),
                    ip: sf.ip,
                    stack_base: stack_index(sf.stack_base),
                    locals_count: sf.locals_count,
                    exception_stack_base: stack_index(sf.exception_stack_base),
                    function_id: sf.function_id,
                    call_offset: sf.call_offset,
                    should_return: false,
                    is_parked: false,
                    namespace: sf.namespace,
                    is_initializer: sf.is_initializer,
                    import_from_module: sf.import_from_module,
                }
            })
            .collect();

        let current_frame_depth = frame_recursion_depth(frames.iter().map(|f| f.function_id), interns);
        let mut frames = frames;
        let current_frame = frames
            .pop()
            .unwrap_or_else(|| CallFrame::new_parked(&program.module_code));

        Self {
            stack: snapshot.stack,
            globals: snapshot.globals,
            current_frame,
            suspended_frames: frames,
            heap,
            interns,
            global_names,
            print_writer,
            exception_stack: snapshot.exception_stack,
            instruction_ip: snapshot.instruction_ip,
            scheduler: snapshot.scheduler,
            module_code: &program.module_code,
            ext_function_load_ip: None,
            json_string_cache: JsonStringCache::default(),
            pending_effect: snapshot.pending_effect,
            pending_lookup_effect: snapshot.pending_lookup_effect,
            recursion_depth: current_frame_depth,
            namespace_scratch: Vec::new(),
            // Always default value at a restore boundary — see the `run_reentry_depth` field doc.
            run_reentry_depth: recursion::MAX_RUN_REENTRY_DEPTH,
            re_pattern_cache: RePatternCache::default(),
            random: snapshot.random,
            modules: snapshot.modules,
            env: {
                let mut env = program.vm_env();
                env.cwd = Cow::Owned(snapshot.cwd);
                env
            },
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
                    .iter_mut()
                    .chain([&mut self.current_frame])
                    .map(CallFrame::serialize)
                    .collect()
            },
            exception_stack: mem::take(&mut self.exception_stack),
            instruction_ip: self.instruction_ip,
            scheduler: mem::take(&mut self.scheduler),
            pending_effect: self.pending_effect.take(),
            pending_lookup_effect: self.pending_lookup_effect.take(),
            // Reset to the starting directory rather than `take` (an empty
            // string), so a later `take_changed_cwd` on this VM stays honest.
            cwd: mem::replace(&mut self.env.cwd, Cow::Borrowed(self.env.initial_cwd)).into_owned(),
            random: mem::take(&mut self.random),
            modules: mem::take(&mut self.modules),
        }
    }

    /// Returns the `stack_base` of the current (topmost) call frame.
    ///
    /// Used by `NameLookup` resolution to determine which stack region to cache
    /// resolved values into when the lookup originated from a function scope.
    pub fn current_stack_base(&self) -> usize {
        self.current_frame.stack_base()
    }

    /// Takes ownership of the globals vector, replacing it with an empty vec.
    ///
    /// Used by the REPL to reclaim globals after VM execution completes.
    /// Must be called before the VM is dropped, since `Drop` will clean up
    /// any remaining globals with `drop_with`.
    pub fn take_globals(&mut self) -> Vec<Value> {
        mem::take(&mut self.globals)
    }

    /// Takes the working directory if this run owns one: after an `os.chdir`,
    /// or always for a restored VM (its snapshot carried the directory). The
    /// REPL writes it back so a directory change persists into later feeds.
    pub fn take_changed_cwd(&mut self) -> Option<String> {
        match mem::replace(&mut self.env.cwd, Cow::Borrowed(self.env.initial_cwd)) {
            Cow::Owned(cwd) => Some(cwd),
            Cow::Borrowed(_) => None,
        }
    }

    /// Rejects NUL-byte and empty paths, then joins the rest to cwd before yielding an OS call.
    /// Rejected calls release their pending effect; existence predicates finish locally.
    fn prepare_os_call(
        &mut self,
        mut call: OsFunctionCall,
        effect: Option<PendingEffect>,
    ) -> RunResult<Option<FrameExit>> {
        if let Err(err) = check_raw_call_paths(&call) {
            release_pending_effect(effect, self.heap);
            if call.is_existence_check() {
                self.push(Value::Bool(false));
                Ok(None)
            } else {
                Err(err)
            }
        } else {
            resolve_call_paths(&mut call, &self.env.cwd);
            // The effect is armed only after this exit is accepted for dispatch.
            Ok(Some(FrameExit::OsCall {
                function_call: call,
                call_id: self.allocate_call_id(),
                effect,
            }))
        }
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
    /// tracker's execution-clock hooks so the time limits measure
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

    /// Runs an export made after [`Self::run_external`] has returned, with the
    /// execution clock running, so a user `__repr__` it calls counts towards
    /// the time limits. Like `run_external` it must not nest: an export inside
    /// the loop (`os.getenv`'s default) is already on the clock and calls
    /// `export` directly.
    pub(crate) fn export_on_clock<T>(&mut self, export: impl FnOnce(&mut Self) -> RunResult<T>) -> RunResult<T> {
        self.heap.tracker.on_execution_start();
        let result = export(self);
        self.heap.tracker.on_execution_stop();
        // A container repr truncates on timeout (`...[timeout]`) rather than failing.
        result.and_then(|value| match self.heap.tracker.check_time() {
            Ok(()) => Ok(value),
            Err(err) => Err(err.into()),
        })
    }

    /// Epilogue for every host-boundary execution window (here, and the
    /// uncaught branch of `resume_with_exception`, which ends a turn without
    /// entering the loop): re-checks both resource limits so an
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

    /// Handles rare call outcomes: turns a suspending
    /// `CallResult` into the `FrameExit` that yields to the host, or awaits an
    /// `AwaitValue`. `Ok(None)` means execution continues in the current frame.
    #[inline(never)]
    fn suspend_call_result(&mut self, result: CallResult) -> Result<Option<FrameExit>, RunError> {
        let exit = match result {
            CallResult::Value(value) => {
                self.push(value);
                return Ok(None);
            }
            CallResult::FramePushed => return Ok(None),
            CallResult::External(name, args) => FrameExit::ExternalCall {
                function_name: name,
                args,
                call_id: self.allocate_call_id(),
                name_load_ip: self.ext_function_load_ip.take(),
            },
            CallResult::OsCall(call) => return self.prepare_os_call(call, None),
            CallResult::OsCallWithEffect { call, effect } => return self.prepare_os_call(call, Some(effect)),
            CallResult::MethodCall { name, args, object_id } => FrameExit::MethodCall {
                method_name: name,
                args,
                call_id: self.allocate_call_id(),
                object_id,
            },
            CallResult::AttrLookup {
                name,
                class_name,
                object_id,
                type_object,
                effect,
            } => FrameExit::AttrLookup {
                name,
                class_name,
                object_id,
                type_object,
                effect,
            },
            CallResult::AwaitValue(value) => {
                // Push the value and implicitly await it (used by asyncio.run())
                self.push(value);
                return match self.exec_get_awaitable()? {
                    AwaitResult::ValueReady(value) => {
                        self.push(value);
                        Ok(None)
                    }
                    AwaitResult::FramePushed => Ok(None),
                    AwaitResult::Yield(pending_calls) => Ok(Some(FrameExit::ResolveFutures(pending_calls))),
                };
            }
        };
        Ok(Some(exit))
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
    /// or via `run_external`/`resume`/`resume_with_exception`/
    /// `resume_with_resolved_futures`) so the execution clock is accounted;
    /// only VM-internal re-entry calls this raw loop.
    ///
    /// The current frame is stored separately from its suspended callers, so
    /// handlers publish its instruction pointer before checkpoints or Python-visible work.
    fn run(&mut self) -> Result<FrameExit, RunError> {
        // Limits and print policy cannot change before returning to the host.
        let check_limits = self.heap.tracker.has_memory_time_limit();
        let poll_print = self.print_writer.wants_poll();
        self.dispatch_threaded(check_limits, poll_print)
    }

    /// Loads a top-level built-in or a cached file module, or suspends with
    /// [`OsFunctionCall::Import`]: the host's value becomes the module, and a
    /// host with no such module falls back to a `.py` file (see `module_finder`).
    /// `LoadSubmodule` resolves the remaining dotted components.
    fn load_module(&mut self, module_id: u16) -> CallResult {
        let name_id = StringId::from_index(module_id);
        match self.interns.static_string(name_id).and_then(StandardLib::from_static) {
            Some(module) => CallResult::Value(Value::Ref(module.create(self))),
            None => self.import_top_level(self.interns.get_str(name_id)),
        }
    }

    /// Resumes execution after an external call completes.
    ///
    /// Pushes the return value onto the stack and continues execution.
    ///
    /// If the paused OS call has a pending effect, the result is routed
    /// through the corresponding helper (file-state update or `os.listdir`
    /// name reduction) before it is pushed back to Python.
    pub fn resume(&mut self, obj: MontyObject) -> Result<FrameExit, RunError> {
        // Pre-conversion effects reshape the raw host object; a post-conversion
        // effect waits in the slot until the value exists to apply it to.
        let obj = match self.pending_effect.take() {
            Some(PendingEffect::Pre(effect)) => match effect.reshape(obj, self) {
                Ok(Reshaped::Value(obj)) => obj,
                Ok(Reshaped::Call { call, effect }) => return self.suspend_again(call, effect),
                Err(err) => return self.resume_with_exception(err),
            },
            // Builds its result straight from the raw reply, never converting it.
            Some(PendingEffect::Scan(effect)) => {
                return match effect.apply(Ok(obj), self) {
                    Ok(value) => {
                        self.push(value);
                        self.run_external()
                    }
                    Err(err) => self.resume_with_exception(err),
                };
            }
            post => {
                self.pending_effect = post;
                obj
            }
        };
        // The sleeps ignore the host's answer, so an unconvertible one (a
        // host object with no wire form, say) must not fail them.
        let obj = match self.pending_effect.take() {
            Some(PendingEffect::Post(PostConversionEffect::DiscardResult)) => {
                self.push(Value::None);
                return self.run_external();
            }
            Some(PendingEffect::Post(PostConversionEffect::SleepResult { result })) => {
                let settled = self.settled_awaitable(result);
                self.push(settled);
                return self.run_external();
            }
            effect => {
                self.pending_effect = effect;
                obj
            }
        };
        // Output-only entropy replies must raise the os.urandom contract error at the draw.
        let seeding = matches!(
            self.pending_effect,
            Some(PendingEffect::Post(PostConversionEffect::SeedRandom { .. }))
        );
        let reply_type = seeding.then(|| obj.as_ref().type_name().to_owned());
        // Surface resource-exhaustion failures from `to_value` (e.g. a host
        // string whose `heap.allocate` trips `max_memory`) as the same
        // `RunError::Resource` that pure-Monty allocations produce, so the
        // user sees `MemoryError` instead of `RuntimeError: invalid return
        // type`. Other input errors stay as `RuntimeError`.
        let value = match obj.to_value(self) {
            Ok(value) => value,
            Err(InvalidInputError::Resource(err)) => return Err(RunError::from(err)),
            Err(InvalidInputError::InvalidType(_)) if let Some(type_name) = reply_type => {
                let effect = self.pending_effect.take();
                release_pending_effect(effect, self.heap);
                return self.resume_with_exception(urandom_reply_error(Err(&type_name), SEED_BYTES));
            }
            Err(other @ InvalidInputError::InvalidType(_)) => {
                return Err(
                    SimpleException::new(ExcType::RuntimeError, Some(format!("invalid return type: {other}"))).into(),
                );
            }
        };
        let result = match self.pending_effect.take() {
            Some(PendingEffect::Post(PostConversionEffect::BufferStore { file_id })) => {
                apply_buffer_store(file_id, value, self).map(Some)
            }
            // Parsing runs outside `run_external`, so it needs its own execution-clock window.
            Some(PendingEffect::Post(PostConversionEffect::JsonLoad { file_id })) => {
                self.heap.tracker.on_execution_start();
                let parsed = apply_json_load(file_id, value, self);
                self.heap.tracker.on_execution_stop();
                parsed.map(Some)
            }
            // Answers nothing: the `ForIter` that asked for the load re-dispatches.
            Some(PendingEffect::Post(PostConversionEffect::BufferLoad { file_id })) => {
                apply_buffer_load(file_id, value, self).map(|()| None)
            }
            Some(PendingEffect::Post(PostConversionEffect::FileNext { file_id, default })) => {
                apply_file_next(file_id, default, value, self).map(Some)
            }
            Some(PendingEffect::Post(PostConversionEffect::WritePosition {
                file_id,
                text_length,
                result_none,
                ..
            })) => apply_write_position(file_id, text_length, result_none, value, self).map(Some),
            Some(PendingEffect::Post(PostConversionEffect::LoadThenCall {
                file_id,
                callable,
                args,
            })) => return self.resume_load_then_call(file_id, callable, args, value),
            Some(PendingEffect::Post(PostConversionEffect::OpenName { name, newline })) => {
                apply_open_name(name, newline, value, self).map(Some)
            }
            Some(PendingEffect::Post(PostConversionEffect::SeedRandom { target, retry })) => {
                apply_seed_random(target, retry, value, self).map(Some)
            }
            Some(PendingEffect::Post(PostConversionEffect::ClockReading { reading })) => {
                apply_clock_reading(reading, value, self).map(Some)
            }
            // The sleeps were answered above; any pre-conversion effect was consumed.
            Some(
                PendingEffect::Post(PostConversionEffect::DiscardResult | PostConversionEffect::SleepResult { .. })
                | PendingEffect::Pre(_)
                | PendingEffect::Scan(_)
                | PendingEffect::Import(_),
            )
            | None => Ok(Some(value)),
        };
        match result {
            Ok(value) => {
                if let Some(value) = value {
                    self.push(value);
                }
                self.run_external()
            }
            Err(err) => self.resume_with_exception(err),
        }
    }

    /// `LoadThenCall` resume: installs the host content, then replays the call
    /// that found its file unread. The call's own outcome decides what happens
    /// next, since the replay may itself suspend (a second unread file).
    fn resume_load_then_call(
        &mut self,
        file_id: HeapId,
        callable: Value,
        args: ArgValues,
        value: Value,
    ) -> Result<FrameExit, RunError> {
        if let Err(err) = apply_buffer_load(file_id, value, self) {
            callable.drop_with(self);
            args.drop_with(self);
            return self.resume_with_exception(err);
        }
        // The replay runs Python (a `fieldnames` iterator, a dialect object's attributes)
        // outside `run_external`, so it needs its own execution-clock window.
        self.heap.tracker.on_execution_start();
        let result = self.call_function(&callable, args);
        self.heap.tracker.on_execution_stop();
        callable.drop_with(self);
        match result {
            Ok(CallResult::Value(value)) => {
                self.push(value);
                self.run_external()
            }
            Ok(CallResult::FramePushed) => self.run_external(),
            Ok(CallResult::OsCall(call)) => self.suspend_again(call, None),
            Ok(CallResult::OsCallWithEffect { call, effect }) => self.suspend_again(call, Some(effect)),
            Ok(other) => {
                other.drop_with(self);
                Err(RunError::internal(
                    "LoadThenCall: replayed call produced an unexpected result",
                ))
            }
            Err(err) => self.resume_with_exception(err),
        }
    }

    /// Yields the next call of a multi-step operation from `resume`: the VM is
    /// still suspended at the same instruction, so the host sees another
    /// `OsCall` and the eventual value is pushed where the first would have been.
    /// A call answered locally (a NUL byte in a predicate) resumes Python at once.
    fn suspend_again(&mut self, call: OsFunctionCall, effect: Option<PendingEffect>) -> Result<FrameExit, RunError> {
        match self.prepare_os_call(call, effect) {
            Ok(Some(exit)) => Ok(exit),
            Ok(None) => self.run_external(),
            Err(err) => self.resume_with_exception(err),
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
        if let Some(effect) = self.pending_effect.take() {
            match effect {
                // Resume on attempting to unlink missing file with unlink(missing_ok=True)
                PendingEffect::Pre(PreConversionEffect::Unlink { missing_ok: true }) if matches!(&error, RunError::Exc(raise) if raise.exc.exc_type() == ExcType::FileNotFoundError) =>
                {
                    self.push(Value::None);
                    return self.run_external();
                }
                PendingEffect::Post(
                    PostConversionEffect::BufferStore { file_id } | PostConversionEffect::JsonLoad { file_id },
                ) => {
                    if let HeapReadOutput::OpenFile(file) = self.heap.read(file_id) {
                        file.get_mut(self.heap).clear_pending_read();
                        drop(file);
                    }
                    self.heap.dec_ref(file_id);
                }
                PendingEffect::Post(PostConversionEffect::WritePosition {
                    file_id,
                    previous_position,
                    previous_length,
                    ..
                }) => {
                    if let HeapReadOutput::OpenFile(file) = self.heap.read(file_id) {
                        file.get_mut(self.heap)
                            .rollback_write_position(previous_position, previous_length);
                        drop(file);
                    }
                    self.heap.dec_ref(file_id);
                }
                // Nothing was installed or seeded, so there is nothing to roll
                // back: dropping the pin and any stashed value is the whole undo.
                PendingEffect::Post(
                    effect @ (PostConversionEffect::BufferLoad { .. }
                    | PostConversionEffect::FileNext { .. }
                    | PostConversionEffect::LoadThenCall { .. }
                    | PostConversionEffect::SeedRandom { .. }),
                ) => effect.release(self.heap),
                PendingEffect::Post(PostConversionEffect::SleepResult { result }) => result.drop_with(self),
                // A walk or glob turns an `OSError` into its result, as CPython's
                // do for a failed `scandir`; anything else still raises.
                PendingEffect::Scan(effect) if effect.absorbs(&error) => {
                    return match effect.apply(Err(error), self) {
                        Ok(value) => {
                            self.push(value);
                            self.run_external()
                        }
                        Err(err) => self.resume_with_exception(err),
                    };
                }
                PendingEffect::Scan(effect) => effect.release(self.heap),
                PendingEffect::Import(_) => {}
                // Hold no state or heap references — nothing to roll back.
                PendingEffect::Pre(_)
                | PendingEffect::Post(
                    PostConversionEffect::OpenName { .. }
                    | PostConversionEffect::DiscardResult
                    | PostConversionEffect::ClockReading { .. },
                ) => {}
            }
        }
        // Use the normal exception handling mechanism
        // handle_exception returns None if caught, Some(error) if not caught
        if let Some(uncaught_error) = self.handle_exception(error) {
            // The turn ends here without entering the run loop, so this is its
            // only chance at the limit re-check every host turn ends with.
            return self.finish_host_turn(Err(uncaught_error));
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

    /// Pushes a frame, releasing its state if the recursion limit rejects it.
    pub(super) fn push_frame(&mut self, frame: CallFrame<'h>) -> RunResult<()> {
        if !self.current_frame.is_parked
            && let Err(e) = self.incr_recursion()
        {
            self.cleanup_frame_state(frame);
            return Err(e.into());
        }
        self.push_admitted_frame(frame);
        Ok(())
    }

    /// Installs a frame after any required recursion reservation.
    fn push_admitted_frame(&mut self, frame: CallFrame<'h>) {
        let caller = mem::replace(&mut self.current_frame, frame);
        self.suspended_frames.push(caller);
    }

    /// Pops the current frame from the call stack.
    ///
    /// Cleans up the frame's stack region and namespace (except for global namespace).
    /// Syncs `instruction_ip` to the parent frame's IP so that exception handling
    /// looks up handlers in the correct frame's exception table.
    ///
    /// Returns `true` if this frame indicated evaluation should stop when popped.
    pub(super) fn pop_frame(&mut self) -> bool {
        let is_frozen = self.current_frame_is_frozen();
        let caller = self.suspended_frames.pop().expect("cannot pop the root frame");
        let frame = mem::replace(&mut self.current_frame, caller);
        let should_return = frame.should_return;
        self.cleanup_frame_state(frame);
        // Sync instruction_ip to the restored caller so exception table lookups
        // target the correct frame after returning from a nested run() call.
        self.instruction_ip = self.current_frame.ip;
        if !self.current_frame.is_parked && !is_frozen {
            self.decr_recursion();
        }
        should_return
    }

    /// Releases what a finished frame owns: its stack region, open handlers and namespace.
    #[inline]
    fn cleanup_frame_state(&mut self, frame: CallFrame<'_>) {
        // Clean up frame's stack region (locals + operand stack, which now
        // includes any in-flight comprehension variables — the operand-stack
        // drain naturally covers them).
        self.stack
            .drain(frame.stack_base()..)
            .for_each(|value| value.drop_with(&mut *self.heap));
        // An exception unwinding out of a handler leaves that handler's entry
        // behind; a later raise in the caller must not see it as being handled.
        self.exception_stack
            .drain(frame.exception_stack_base()..)
            .drop_with(self.heap);
        // Almost every frame has no namespace; skip the release call for those.
        if let Some(namespace) = frame.namespace {
            namespace.drop_with(self.heap);
        }
    }

    /// Drops the current task's operand and exception stacks and discards its frames.
    /// Leaves a parked frame until another task is activated, or the VM is dropped.
    pub(super) fn cleanup_current_task(&mut self) {
        self.stack.drain(..).drop_with(self.heap);
        self.exception_stack.drain(..).drop_with(self.heap);
        for frame in self.suspended_frames.drain(..) {
            frame.namespace.drop_with(self.heap);
        }
        self.current_frame.namespace.take().drop_with(self.heap);
        self.current_frame.park(self.module_code);
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

    /// Returns surviving tasks' pending calls when every task is blocked
    /// or [`yield_if_parked`] detects that dispatch has no runnable frame.
    /// With no pending calls, report the stall now: resuming cannot progress
    /// and would obscure the discarded task that caused it.
    pub(super) fn pending_futures_exit(&self) -> Result<FrameExit, RunError> {
        let pending_call_ids = self.scheduler.pending_call_ids();
        if pending_call_ids.is_empty() {
            Err(RunError::internal(
                "asyncio scheduler stalled: no task to run and no pending external calls",
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

    /// Returns the position a suspension at the current instruction reports.
    ///
    /// `instruction_ip` still names the suspending opcode; its location entry
    /// gives the byte range, so this reads no source.
    pub(crate) fn suspension_position(&self) -> SourceRange {
        self.code_position(self.current_frame.code, self.instruction_ip)
    }

    /// Returns the source position of the `await` the main task is blocked on.
    ///
    /// The position reported when every task is blocked on host futures: the
    /// blocked main task may be the loaded context, or parked in the scheduler
    /// with its frames saved while a spawned task ran last.
    pub(crate) fn main_task_position(&self) -> SourceRange {
        if self.is_main_task() && !self.current_frame.is_parked {
            self.suspension_position()
        } else {
            // `save_task_context` pushes the executing frame last.
            self.scheduler
                .main_task()
                .and_then(|task| Some((task.frames.last()?, task.instruction_ip)))
                .map_or_else(SourceRange::unknown, |(frame, ip)| {
                    self.code_position(frame_code(self.interns, self.module_code, frame.function_id), ip)
                })
        }
    }

    /// The byte range of the instruction at `offset` in `code`, named by its source.
    fn code_position(&self, code: &Code, offset: usize) -> SourceRange {
        code.location_for_offset(offset)
            .map_or_else(SourceRange::unknown, |entry| {
                let range = entry.range();
                SourceRange::new(
                    self.interns.get_filename(range.filename),
                    range.start_byte,
                    range.end_byte,
                )
            })
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

    /// Combines local loads with subscripting while preserving generic fallback ownership.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "native subscription benchmarks benefit from dispatch-local code"
    )]
    fn subscript_locals(&mut self, object_slot: u16, key_slot: u16, flags: u8) -> RunResult<()> {
        let base = self.current_frame.stack_base();
        let object = &self.stack[base + usize::from(object_slot)];
        let constant = if flags == 2 {
            Value::Int(i64::from(key_slot.cast_signed()))
        } else {
            Value::InternString(StringId::from_index(key_slot))
        };
        let key = if flags != 0 {
            &constant
        } else {
            &self.stack[base + usize::from(key_slot)]
        };

        if let Some(value) = self.native_subscript_view(object, key) {
            if let Value::Int(number) = value {
                let number = *number;
                self.push(Value::Int(number));
            } else if let Value::Ref(id) = value {
                let id = *id;
                self.heap.inc_ref(id);
                self.push(Value::Ref(id));
            } else {
                let value = value.clone_with_heap(self.heap);
                self.push(value);
            }
            return Ok(());
        }

        self.subscript_locals_fallback(object_slot, key_slot, flags)
    }

    /// Owns both operands before generic dispatch can execute Python.
    #[inline(never)]
    fn subscript_locals_fallback(&mut self, object_slot: u16, key_slot: u16, flags: u8) -> RunResult<()> {
        let base = self.current_frame.stack_base();
        let object = &self.stack[base + usize::from(object_slot)];
        let constant = if flags == 2 {
            Value::Int(i64::from(key_slot.cast_signed()))
        } else {
            Value::InternString(StringId::from_index(key_slot))
        };
        let key = if flags != 0 {
            &constant
        } else {
            &self.stack[base + usize::from(key_slot)]
        };

        if matches!(object, Value::Undefined) {
            self.instruction_ip += 1;
            return Err(self.unbound_local_error(object_slot, self.current_frame.code.local_name(object_slot)));
        }
        if matches!(key, Value::Undefined) {
            self.instruction_ip += 2;
            return Err(self.unbound_local_error(key_slot, self.current_frame.code.local_name(key_slot)));
        }

        let object = object.clone_with_heap(self.heap);
        let key = key.clone_with_heap(self.heap);
        let result = object.py_getitem(&key, self);
        object.drop_with(self);
        key.drop_with(self);
        self.push(result?);
        Ok(())
    }

    /// Stores a native hit directly in the destination local; no guest code runs here.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "keep the direct local assignment in the dispatch loop"
    )]
    fn try_subscript_to_local(&mut self, object_slot: u8, key_slot: u8, destination: u8) -> bool {
        let base = self.current_frame.stack_base();
        let object = &self.stack[base + usize::from(object_slot)];
        let key = &self.stack[base + usize::from(key_slot)];
        let value = self.native_subscript_view(object, key);
        let Some(value) = value else {
            return false;
        };

        if let Value::Int(number) = value {
            let number = *number;
            let destination = &mut self.stack[base + usize::from(destination)];
            if let Value::Int(previous) = destination {
                *previous = number;
            } else {
                let old = mem::replace(destination, Value::Int(number));
                old.drop_with(self);
            }
            return true;
        }

        if let Value::Ref(id) = value {
            let id = *id;
            // The destination already owns this exact object, so ownership does not change.
            if let Value::Ref(previous) = &self.stack[base + usize::from(destination)]
                && *previous == id
            {
                return true;
            }
            self.heap.inc_ref(id);
            let old = mem::replace(&mut self.stack[base + usize::from(destination)], Value::Ref(id));
            old.drop_with(self);
            return true;
        }

        let value = value.clone_with_heap(self.heap);
        let old = mem::replace(&mut self.stack[base + usize::from(destination)], value);
        old.drop_with(self);
        true
    }

    /// Replaces native subscript operands in place; fallback retains both owned operands.
    #[inline(always)]
    #[expect(clippy::inline_always, reason = "keep native stack replacement in the dispatch loop")]
    fn try_native_stack_subscript(&mut self) -> bool {
        let len = self.stack.len();
        let object = &self.stack[len - 2];
        let key = &self.stack[len - 1];
        let value = self.native_subscript_view(object, key);
        let Some(value) = value else {
            return false;
        };
        if let Value::Int(number) = value {
            let number = *number;
            let key = self.pop();
            let object = mem::replace(&mut self.stack[len - 2], Value::Int(number));
            object.drop_with(self);
            key.drop_with(self);
            return true;
        }
        let value = value.clone_with_heap(self.heap);
        let key = self.pop();
        let object = mem::replace(&mut self.stack[len - 2], value);
        object.drop_with(self);
        key.drop_with(self);
        true
    }

    /// Keeps temporary integer/string keys out of the handler's continuation frame.
    #[inline(never)]
    fn native_const_subscript_view<'a>(&'a self, object: &Value, key: u16, string: bool) -> Option<&'a Value> {
        let key = if string {
            Value::InternString(StringId::from_index(key))
        } else {
            Value::Int(i64::from(key.cast_signed()))
        };
        self.native_subscript_view(object, &key)
    }

    /// Reads native containers under one immutable VM borrow; no guest code can run.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "outlining the borrowed view regressed list access benchmarks"
    )]
    fn native_subscript_view<'a>(&'a self, object: &Value, key: &Value) -> Option<&'a Value> {
        let Value::Ref(id) = object else {
            return None;
        };
        let items = match self.heap.get(*id) {
            HeapData::Dict(dict) => return dict.native_subscript(key, self),
            HeapData::List(list) => list.as_slice(),
            HeapData::Tuple(tuple) => tuple.as_slice(),
            _ => return None,
        };
        let index = match key {
            Value::Int(index) => *index,
            Value::Bool(index) => i64::from(*index),
            _ => return None,
        };
        let index = if index < 0 {
            usize::try_from(index + i64::try_from(items.len()).ok()?).ok()?
        } else {
            usize::try_from(index).ok()?
        };
        items.get(index)
    }

    /// Loads a local variable and pushes it onto the stack.
    ///
    /// Raises `UnboundLocalError` if the slot holds `Undefined` — every reachable
    /// `LoadLocal*` slot is registered as assigned by the compiler, so an undefined
    /// value can only mean access-before-assignment.
    fn load_local(&mut self, slot: u16) -> RunResult<()> {
        let index = self.current_frame.stack_base() + slot as usize;
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
    /// always empty. `__file__` is the script name's final component under the
    /// working directory the run started in. `__loader__` is deliberately
    /// *not* exposed: CPython only ever puts a loader object there (never
    /// `None`), so rather than diverge on the type we let it raise `NameError`
    /// like other unexposed dunders (`__cached__`, …).
    fn module_dunder(&self, name_id: StringId) -> Option<Value> {
        let value = match self.interns.get_str(name_id) {
            "__name__" => Value::InternString(self.interns.intern_static(StaticStrings::DunderMain)),
            "__debug__" => Value::Bool(true),
            "__file__" => allocate_string(self.env.file(), self.heap),
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
        let index = self.current_frame.stack_base() + slot as usize;
        let old_value = mem::replace(&mut self.stack[index], value);
        old_value.drop_with(self);
    }

    /// Deletes a local variable (sets it to Undefined).
    fn delete_local(&mut self, slot: u16) {
        let index = self.current_frame.stack_base() + slot as usize;
        let old_value = mem::replace(&mut self.stack[index], Value::Undefined);
        old_value.drop_with(self);
    }

    /// Loads a global variable and pushes it onto the stack.
    ///
    /// When the variable is undefined, falls back to builtin resolution (see
    /// [`builtin_for_name`]) before yielding `NameLookup` so the host can supply
    /// an external binding.
    #[inline]
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
    /// Read from the live name map rather than the module `Code`, so slots
    /// first allocated after the module was compiled are named too.
    fn global_name(&self, slot: u16) -> Option<StringId> {
        self.global_names.names().get(usize::from(slot)).copied()
    }

    /// Pops the top of stack and stores it in a global variable.
    ///
    /// Reassigning a reserved module dunder (see [`RESERVED_MODULE_DUNDERS`]) is
    /// rejected at compile time (see `Compiler::compile_store`), so no name
    /// check is needed here.
    #[inline]
    fn store_global(&mut self, slot: u16) {
        let value = self.pop();
        self.set_global_slot(slot, value);
    }

    /// Binds `value` at a global slot, releasing what the slot held.
    fn set_global_slot(&mut self, slot: u16, value: Value) {
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
        match &self.stack[self.current_frame.stack_base() + slot as usize] {
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
        let HeapReadOutput::Cell(cell) = this.heap.read(cell_id) else {
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
        let HeapReadOutput::Cell(cell) = this.heap.read(cell_id) else {
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
        release_pending_effect(self.pending_effect.take(), self.heap);
        self.pending_lookup_effect.take().drop_with(self.heap);
        self.cleanup_current_task();
        self.scheduler.cleanup(self.heap);
        self.globals.drain(..).drop_with(self.heap);
        mem::take(&mut self.modules).drop_with(self.heap);
        self.json_string_cache.drop_all(self.heap);
    }
}
