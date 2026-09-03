//! This module defines the public types returned by [`MontyRun::start()`](crate::MontyRun::start)
//! and their resume methods. Each variant of [`RunProgress`] wraps a dedicated struct
//! (`FunctionCall`, `OsCall`, `NameLookup`, `ResolveFutures`) that carries only the
//! fields and resume methods relevant to that suspension point.
//!
//! The internal [`Snapshot`] type is `pub(crate)` — callers interact exclusively with
//! the per-variant structs.

use std::mem;

use monty_types::{
    ExcType, InvalidInputError, MontyException, MontyObject, MontyUuid, OsFunctionCall, PrintWriter, ResourceTracker,
};

use crate::{
    asyncio::CallId,
    bytecode::{FrameExit, PendingLookupEffect, VM, VMSnapshot},
    exception_private::{ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropWithContext, Heap, HeapReader},
    object_bridge::MontyObjectExt,
    os_dispatch::release_pending_effect,
    run::Executor,
    value::Value,
};

// ---------------------------------------------------------------------------
// RunProgress enum
// ---------------------------------------------------------------------------

/// Result of a single step of iterative execution.
///
/// Each variant wraps a dedicated struct that owns the execution state and
/// exposes only the resume methods relevant to that suspension reason.
///
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum RunProgress {
    /// Execution paused at an external function call, or a method call on a
    /// host object (`object_id` set).
    FunctionCall(FunctionCall),
    /// Execution paused for an OS-level operation (filesystem, network, etc.).
    OsCall(OsCall),
    /// All async tasks are blocked waiting for external futures to resolve.
    ResolveFutures(ResolveFutures),
    /// Execution paused for an unresolved name lookup.
    NameLookup(NameLookup),
    /// Execution completed with a final result.
    Complete(MontyObject),
}

impl RunProgress {
    /// Consumes the progress and returns the [`FunctionCall`] struct if this is a function call.
    #[must_use]
    pub fn into_function_call(self) -> Option<FunctionCall> {
        match self {
            Self::FunctionCall(call) => Some(call),
            _ => None,
        }
    }

    /// Consumes the progress and returns the [`OsCall`] struct if this is an OS call.
    #[must_use]
    pub fn into_os_call(self) -> Option<OsCall> {
        match self {
            Self::OsCall(call) => Some(call),
            _ => None,
        }
    }

    /// Consumes the progress and returns the final value if execution completed.
    #[must_use]
    pub fn into_complete(self) -> Option<MontyObject> {
        match self {
            Self::Complete(value) => Some(value),
            _ => None,
        }
    }

    /// Consumes the progress and returns the [`ResolveFutures`] struct.
    #[must_use]
    pub fn into_resolve_futures(self) -> Option<ResolveFutures> {
        match self {
            Self::ResolveFutures(state) => Some(state),
            _ => None,
        }
    }

    /// Consumes the progress and returns the [`NameLookup`] struct.
    #[must_use]
    pub fn into_name_lookup(self) -> Option<NameLookup> {
        match self {
            Self::NameLookup(lookup) => Some(lookup),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// FunctionCall
// ---------------------------------------------------------------------------

/// Execution paused at an external function call or dataclass method call.
///
/// The host can choose how to handle this:
/// - **Sync resolution**: Call [`resume`](Self::resume) to push the result and continue.
/// - **Async resolution**: Call [`resume_pending`](Self::resume_pending) to push an `ExternalFuture` and continue.
///
/// When using async resolution, the code continues and may `await` the future later.
/// If the future isn't resolved when awaited, execution yields with [`ResolveFutures`].
///
/// When `object_id` is set, this represents a method call on a host-backed
/// object (construction of a host class is a `__call__` method call): route
/// it to the host object with that uuid — the receiver is NOT in `args`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FunctionCall {
    /// The name of the function or method being called.
    pub function_name: String,
    /// The positional arguments passed to the function.
    pub args: Vec<MontyObject>,
    /// The keyword arguments passed to the function (key, value pairs).
    pub kwargs: Vec<(MontyObject, MontyObject)>,
    /// Unique identifier for this call (used for async correlation).
    pub call_id: u32,
    /// Uuid of the routed receiver — an instance, or a class type (a
    /// classmethod call, or construction spelled `__call__`); `None` for
    /// plain external function calls.
    pub object_id: Option<MontyUuid>,
    /// Internal execution snapshot.
    snapshot: Snapshot,
}

impl FunctionCall {
    /// Creates a new `FunctionCall` from its parts.
    fn new(
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        object_id: Option<MontyUuid>,
        snapshot: Snapshot,
    ) -> Self {
        Self {
            function_name,
            args,
            kwargs,
            call_id,
            object_id,
            snapshot,
        }
    }

    /// Returns a mutable reference to the resource tracker.
    ///
    /// This allows modifying resource limits between execution phases,
    /// e.g. setting a time limit before resuming after an external function call.
    pub fn tracker_mut(&mut self) -> &mut ResourceTracker {
        &mut self.snapshot.heap.tracker
    }

    /// Returns the resource tracker while execution is suspended.
    #[must_use]
    pub fn tracker(&self) -> &ResourceTracker {
        &self.snapshot.heap.tracker
    }

    /// Resumes execution with the return value or exception from the external function.
    ///
    /// Consumes self and returns the next execution progress.
    ///
    /// # Arguments
    /// * `result` — The return value, exception, or pending future marker.
    /// * `print` — Writer for `print()` output.
    pub fn resume(
        self,
        result: impl Into<ExtFunctionResult>,
        print: PrintWriter<'_>,
    ) -> Result<RunProgress, MontyException> {
        self.snapshot.run(result, print)
    }

    /// Resumes execution by pushing an `ExternalFuture` instead of a concrete value.
    ///
    /// This is the async resolution pattern: the host continues execution with a
    /// pending future. The code can then `await` this future later. If the code
    /// awaits the future before it's resolved, execution will yield with
    /// [`RunProgress::ResolveFutures`].
    ///
    /// Uses `self.call_id` internally — no need to pass it again.
    ///
    /// # Arguments
    /// * `print` — Writer for print output.
    pub fn resume_pending(self, print: PrintWriter<'_>) -> Result<RunProgress, MontyException> {
        self.snapshot.run(ExtFunctionResult::Future(self.call_id), print)
    }

    /// Aborts the feed with an uncatchable exception; see [`OsCall::abort`].
    pub fn abort(self, exc: MontyException, print: PrintWriter<'_>) -> Result<RunProgress, MontyException> {
        self.snapshot.abort(exc, print)
    }
}

// ---------------------------------------------------------------------------
// OsCall
// ---------------------------------------------------------------------------

/// Execution paused for an OS-level operation.
///
/// The host should execute the OS operation (filesystem, network, etc.) and
/// call `resume(return_value, print)` to provide the result and continue.
///
/// This enables sandboxed execution where the interpreter never directly performs I/O.
///
/// `function_call` is a tagged [`OsFunctionCall`] whose variants carry the
/// typed args directly. Host bindings that need a generic
/// `(positional, keyword)` `MontyObject` view can call [`OsFunctionCall::to_args`]
/// (the public projection method).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct OsCall {
    /// Typed OS-call dispatch value (variant + args).
    pub function_call: OsFunctionCall,
    /// Unique identifier for this call (used for async correlation).
    pub call_id: u32,
    /// Internal execution snapshot.
    snapshot: Snapshot,
}

impl OsCall {
    /// Creates a new `OsCall` from its parts.
    fn new(function_call: OsFunctionCall, call_id: u32, snapshot: Snapshot) -> Self {
        Self {
            function_call,
            call_id,
            snapshot,
        }
    }

    /// Resumes execution with the OS call result.
    ///
    /// # Arguments
    /// * `result` — The return value or exception from the OS operation.
    /// * `print` — Writer for `print()` output.
    pub fn resume(
        self,
        result: impl Into<ExtFunctionResult>,
        print: PrintWriter<'_>,
    ) -> Result<RunProgress, MontyException> {
        self.snapshot.run(result.into(), print)
    }

    /// Dispatches the call to `handler` and resumes execution with its result.
    ///
    /// `handler` receives the [`OsFunctionCall`] by value, so large
    /// `WriteText` / `WriteBytes` payloads move into the host without
    /// cloning. Prefer this over reading [`Self::function_call`] and calling
    /// [`Self::resume`] separately when the handler consumes the call.
    pub fn resume_with(
        self,
        print: PrintWriter<'_>,
        handler: impl FnOnce(OsFunctionCall) -> ExtFunctionResult,
    ) -> Result<RunProgress, MontyException> {
        let result = handler(self.function_call);
        self.snapshot.run(result, print)
    }

    /// Ends the feed by raising `exc` uncatchably at the suspended call.
    ///
    /// The exception builds a traceback but bypasses sandbox handlers. Pending
    /// file effects roll back. Always returns `Err`.
    pub fn abort(self, exc: MontyException, print: PrintWriter<'_>) -> Result<RunProgress, MontyException> {
        self.snapshot.abort(exc, print)
    }

    /// Returns the resource tracker while execution is suspended.
    #[must_use]
    pub fn tracker(&self) -> &ResourceTracker {
        &self.snapshot.heap.tracker
    }
}

// ---------------------------------------------------------------------------
// NameLookup
// ---------------------------------------------------------------------------

/// Where a resumed name-lookup value lands, and what an `Undefined` answer
/// raises.
///
/// `Namespace` is the classic global/local lookup (the value is cached in the
/// slot, `Undefined` raises `NameError`). `Instance` is a lazy attribute
/// lookup on a host class instance: the value becomes the result of the
/// attribute expression — never cached, so every access re-consults the host —
/// and `Undefined` raises `AttributeError` naming the real class.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum LookupScope {
    /// Plain global/local name — cache into the namespace slot.
    Namespace {
        /// The namespace slot where the resolved value should be cached.
        namespace_slot: u16,
        /// Whether this is a global slot or a local/function slot.
        is_global: bool,
    },
    /// Lazy attribute on a host-backed object (instance or class type).
    Attr {
        /// Host identity of the object whose attribute is read.
        object_id: MontyUuid,
        /// Class name captured at suspension for the AttributeError message.
        class_name: String,
        /// True for a class type receiver — selects CPython's
        /// `type object '...' has no attribute ...` message.
        type_object: bool,
    },
}

impl LookupScope {
    /// The receiver uuid when this is a lazy attribute lookup.
    pub(crate) fn object_id(&self) -> Option<MontyUuid> {
        match self {
            Self::Namespace { .. } => None,
            Self::Attr { object_id, .. } => Some(*object_id),
        }
    }
}

/// Execution paused for an unresolved name lookup, or — when
/// [`object_id`](Self::object_id) is set — a lazy attribute lookup on a
/// host-backed object.
///
/// The host should check if the name corresponds to a known external function,
/// value, or instance attribute. Call [`resume`](Self::resume) with
/// [`NameLookupResult::Value`] to continue, [`NameLookupResult::Undefined`]
/// to raise `NameError` (plain lookups) / `AttributeError` (instance lookups),
/// or [`NameLookupResult::Error`] to raise a host exception in the sandbox.
///
/// The namespace slot and scope are managed internally — the host only needs to
/// provide the name resolution result.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct NameLookup {
    /// The name being looked up.
    pub name: String,
    /// Where the resolved value lands (namespace slot or instance attribute).
    scope: LookupScope,
    /// Internal execution snapshot.
    snapshot: Snapshot,
}

impl NameLookup {
    /// Creates a new `NameLookup` from its parts.
    fn new(name: String, scope: LookupScope, snapshot: Snapshot) -> Self {
        Self { name, scope, snapshot }
    }

    /// Host identity of the receiver for a lazy attribute lookup; `None` for
    /// a plain global/local name lookup.
    #[must_use]
    pub fn object_id(&self) -> Option<MontyUuid> {
        self.scope.object_id()
    }

    /// Returns the resource tracker while execution is suspended.
    #[must_use]
    pub fn tracker(&self) -> &ResourceTracker {
        &self.snapshot.heap.tracker
    }

    /// Aborts the feed with an uncatchable exception; see [`OsCall::abort`].
    pub fn abort(self, exc: MontyException, print: PrintWriter<'_>) -> Result<RunProgress, MontyException> {
        self.snapshot.abort(exc, print)
    }

    /// Resumes execution after name resolution.
    ///
    /// For a plain lookup, caches the resolved value in the appropriate slot
    /// (globals or stack) before pushing it, and `Undefined` raises
    /// `NameError`. For an instance attribute lookup, the value is pushed as
    /// the attribute expression's result (never cached), and `Undefined`
    /// raises `AttributeError`. `Error` raises the host's exception in the
    /// sandbox, bypassing any `hasattr()` / `getattr()` default.
    ///
    /// # Arguments
    /// * `result` — The resolved value, [`Undefined`](NameLookupResult::Undefined), or a host exception.
    /// * `print` — Writer for print output.
    pub fn resume(
        self,
        result: impl Into<NameLookupResult>,
        print: PrintWriter<'_>,
    ) -> Result<RunProgress, MontyException> {
        let result = result.into();

        let Snapshot {
            mut heap,
            executor,
            vm_state: snapshot_vm_state,
        } = self.snapshot;
        let scope = self.scope;
        let name = self.name;

        let (converted, vm_state) =
            HeapReader::with(&mut heap, &mut (&executor, print), |reader, (executor, print)| {
                // Restore the VM first, then convert inside its lifetime
                let mut vm = VM::restore(
                    snapshot_vm_state,
                    &executor.module_code,
                    reader,
                    &executor.interns,
                    print.reborrow(),
                    executor.assert_repr_max_bytes,
                );

                // Resolve the name lookup result with the VM alive
                let answer = LookupAnswer::new(result, &mut vm);
                let effect = vm.pending_lookup_effect.take();
                let vm_result = resume_lookup(&mut vm, answer, effect, &scope, &name);

                // Three-phase: convert while VM alive, snapshot, build progress
                let converted = convert_frame_exit(vm_result, &mut vm);
                let vm_state = check_snapshot_from_converted(&converted, vm);
                (converted, vm_state)
            });
        build_run_progress(converted, vm_state, executor, heap)
    }
}

/// A host's [`NameLookupResult`] in interpreter terms, ready for
/// [`resume_lookup`].
pub(crate) enum LookupAnswer {
    /// The host served this value.
    Value(Value),
    /// The name / attribute does not exist.
    Undefined,
    /// Resolving it failed — a host exception, or a value the heap could not
    /// take — to be raised in the sandbox where the lookup suspended.
    Error(RunError),
}

impl LookupAnswer {
    /// Converts the host's answer while the VM is alive.
    ///
    /// A value the heap cannot take becomes an in-sandbox error with the
    /// mapping [`VM::resume`] applies to external call results: a resource
    /// limit raises `MemoryError`, an unconvertible object `RuntimeError`.
    pub(crate) fn new(result: NameLookupResult, vm: &mut VM<'_>) -> Self {
        match result {
            NameLookupResult::Value(obj) => match obj.to_value(vm) {
                Ok(value) => Self::Value(value),
                Err(InvalidInputError::Resource(err)) => Self::Error(err.into()),
                Err(other @ InvalidInputError::InvalidType(_)) => Self::Error(
                    SimpleException::new(
                        ExcType::RuntimeError,
                        Some(format!("invalid name lookup result: {other}")),
                    )
                    .into(),
                ),
            },
            NameLookupResult::Undefined => Self::Undefined,
            NameLookupResult::Error(exc) => Self::Error(exc.into()),
        }
    }
}

/// Resumes a suspended lookup with the host's answer and runs on.
///
/// `effect` is the `hasattr()` / `getattr()` default armed for the lookup, if
/// any. An error is raised as-is, dropping the effect — CPython only swallows
/// `AttributeError` there, and the host reported something else. A served
/// value or `Undefined` goes through the effect when one is armed; otherwise
/// the value is pushed (a namespace lookup also caches it in its slot), or
/// `Undefined` raises the `NameError` / `AttributeError` an unanswered
/// lookup gets.
pub(crate) fn resume_lookup(
    vm: &mut VM<'_>,
    answer: LookupAnswer,
    effect: Option<PendingLookupEffect>,
    scope: &LookupScope,
    name: &str,
) -> RunResult<FrameExit> {
    let value = match (answer, effect) {
        (LookupAnswer::Error(err), effect) => {
            effect.drop_with(vm);
            return vm.resume_with_exception(err);
        }
        (LookupAnswer::Value(value), Some(effect)) => effect.apply(Some(value), vm),
        (LookupAnswer::Undefined, Some(effect)) => effect.apply(None, vm),
        (LookupAnswer::Value(value), None) => {
            if let LookupScope::Namespace {
                namespace_slot,
                is_global,
            } = scope
            {
                // Cache the resolved value in the appropriate slot
                let slot_idx = *namespace_slot as usize;
                let cloned = value.clone_with_heap(vm);
                let slot = if *is_global {
                    &mut vm.globals[slot_idx]
                } else {
                    let stack_base = vm.current_stack_base();
                    &mut vm.stack[stack_base + slot_idx]
                };
                let old = mem::replace(slot, cloned);
                old.drop_with(vm);
            }
            value
        }
        (LookupAnswer::Undefined, None) => return vm.resume_with_exception(undefined_lookup_error(scope, name)),
    };
    vm.push(value);
    vm.run_external()
}

/// Answers every lookup exit no host will serve — the non-iterative paths —
/// as `Undefined`, running on until execution reaches some other exit.
///
/// An armed `hasattr()` / `getattr()` effect yields `False` / its default; a
/// bare name or attribute read raises `NameError` / `AttributeError` through
/// the VM so the traceback is captured. Any other exit passes through.
pub(crate) fn answer_unserved_lookups(mut result: RunResult<FrameExit>, vm: &mut VM<'_>) -> RunResult<FrameExit> {
    loop {
        let (scope, name, effect) = match result? {
            FrameExit::NameLookup {
                name_id,
                namespace_slot,
                is_global,
            } => {
                let scope = LookupScope::Namespace {
                    namespace_slot,
                    is_global,
                };
                (scope, vm.interns.get_str(name_id).to_owned(), None)
            }
            FrameExit::AttrLookup {
                name,
                class_name,
                object_id,
                type_object,
                effect,
            } => {
                let scope = LookupScope::Attr {
                    object_id,
                    class_name,
                    type_object,
                };
                (scope, name.into_string(vm.interns), effect)
            }
            other => return Ok(other),
        };
        result = resume_lookup(vm, LookupAnswer::Undefined, effect, &scope, &name);
    }
}

/// The exception an `Undefined` answer raises: `NameError` for a plain name
/// lookup, `AttributeError` naming the real class for an instance attribute.
fn undefined_lookup_error(scope: &LookupScope, name: &str) -> RunError {
    match scope {
        LookupScope::Namespace { .. } => ExcType::name_error(name).into(),
        LookupScope::Attr {
            class_name,
            type_object: false,
            ..
        } => ExcType::attribute_error(class_name, name),
        LookupScope::Attr {
            class_name,
            type_object: true,
            ..
        } => ExcType::attribute_error_type(class_name, name),
    }
}

// ---------------------------------------------------------------------------
// ResolveFutures
// ---------------------------------------------------------------------------

/// Execution state paused while waiting for external future results.
///
/// Supports incremental resolution — you can provide partial results and Monty
/// will continue running until all tasks are blocked again.
///
/// Use [`pending_call_ids`](Self::pending_call_ids) to see which calls are pending, then call
/// [`resume`](Self::resume) with some or all of the results.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ResolveFutures {
    /// The executor containing compiled code and interns.
    executor: Executor,
    /// The VM state containing stack, frames, globals, and exception state.
    vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    heap: Heap,
    /// The pending call_ids that this snapshot is waiting on.
    pending_call_ids: Vec<u32>,
}

impl ResolveFutures {
    /// Creates a new `ResolveFutures` from its parts.
    fn new(executor: Executor, vm_state: VMSnapshot, heap: Heap, pending_call_ids: Vec<u32>) -> Self {
        Self {
            executor,
            vm_state,
            heap,
            pending_call_ids,
        }
    }

    /// Returns unresolved call IDs for this suspended state.
    #[must_use]
    pub fn pending_call_ids(&self) -> &[u32] {
        &self.pending_call_ids
    }

    /// Returns the resource tracker while execution is suspended.
    #[must_use]
    pub fn tracker(&self) -> &ResourceTracker {
        &self.heap.tracker
    }

    /// Aborts with an uncatchable exception and abandons pending futures.
    pub fn abort(self, exc: MontyException, print: PrintWriter<'_>) -> Result<RunProgress, MontyException> {
        let Self {
            executor,
            vm_state,
            heap,
            ..
        } = self;
        abort_restored(executor, vm_state, heap, exc, print)
    }

    /// Forces a GC cycle against the exact root walk used by the live VM.
    ///
    /// This is test-only support for reproducing GC bugs while execution is
    /// suspended in a `ResolveFutures` snapshot. The method round-trips through
    /// `VM::restore()` and `VM::snapshot()` so the production scheduler/stack root
    /// logic is exercised rather than duplicated in the test.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    #[must_use]
    pub fn __force_gc_for_tests(self) -> Self {
        let Self {
            executor,
            vm_state,
            mut heap,
            pending_call_ids,
        } = self;

        let vm_state = HeapReader::with(&mut heap, &mut &executor, |reader, executor| {
            let mut vm = VM::restore(
                vm_state,
                &executor.module_code,
                reader,
                &executor.interns,
                PrintWriter::Stdout,
                executor.assert_repr_max_bytes,
            );
            vm.__force_gc_for_tests();
            vm.snapshot()
        });

        Self::new(executor, vm_state, heap, pending_call_ids)
    }

    /// Number of tasks still live while this snapshot is suspended.
    ///
    /// Test-only: lets a test assert that a gather child whose external call
    /// failed was dropped rather than left parked forever on a future that
    /// can no longer be resolved.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    #[must_use]
    pub fn __live_task_count_for_tests(&self) -> usize {
        self.vm_state.live_task_count()
    }

    /// Resumes execution with results for some or all pending futures.
    ///
    /// **Incremental resolution**: You don't need to provide all results at once.
    /// If you provide a partial list, Monty will:
    /// 1. Mark those futures as resolved
    /// 2. Unblock any tasks waiting on those futures
    /// 3. Continue running until all tasks are blocked again
    /// 4. Return [`ResolveFutures`] with the remaining pending calls
    ///
    /// # Arguments
    /// * `results` — List of `(call_id, result)` pairs. Can be a subset of pending calls.
    /// * `print` — Writer for print output.
    ///
    /// # Errors
    /// Returns [`MontyException`] if any `call_id` in `results` is not in the pending set.
    pub fn resume(
        self,
        results: Vec<(u32, ExtFunctionResult)>,
        print: PrintWriter<'_>,
    ) -> Result<RunProgress, MontyException> {
        let Self {
            executor,
            vm_state,
            mut heap,
            pending_call_ids,
        } = self;

        // Validate that all provided call_ids are in the pending set before restoring VM.
        let invalid_call_id = results
            .iter()
            .find(|(call_id, _)| !pending_call_ids.contains(call_id))
            .map(|(call_id, _)| *call_id);

        let (converted, vm_state) =
            HeapReader::with(&mut heap, &mut (&executor, print), |reader, (executor, print)| {
                // Restore the VM from the snapshot (must happen before any error return to clean up properly).
                let mut vm = VM::restore(
                    vm_state,
                    &executor.module_code,
                    reader,
                    &executor.interns,
                    print.reborrow(),
                    executor.assert_repr_max_bytes,
                );

                // Now check for invalid call_ids after VM is restored.
                if let Some(call_id) = invalid_call_id {
                    return Err(MontyException::runtime_error(format!(
                        "unknown call_id {call_id}, expected one of: {pending_call_ids:?}"
                    )));
                }

                let result = vm.resume_with_resolved_futures(results);

                // Three-phase: convert while VM alive, snapshot, build progress
                let converted = convert_frame_exit(result, &mut vm);
                let vm_state = check_snapshot_from_converted(&converted, vm);
                Ok((converted, vm_state))
            })?;
        build_run_progress(converted, vm_state, executor, heap)
    }
}

// ---------------------------------------------------------------------------
// Snapshot (pub(crate))
// ---------------------------------------------------------------------------

/// Internal execution state that can be resumed after suspension.
///
/// This is a `pub(crate)` implementation detail wrapped by the per-variant
/// structs (`FunctionCall`, `OsCall`, `NameLookup`). It is not exposed in the
/// public API.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Snapshot {
    /// The executor containing compiled code and interns.
    pub(crate) executor: Executor,
    /// The VM state containing stack, frames, globals, and exception state.
    pub(crate) vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    pub(crate) heap: Heap,
}

impl Snapshot {
    /// Continues execution with the return value or exception from the external call.
    pub(crate) fn run(
        self,
        result: impl Into<ExtFunctionResult>,
        print: PrintWriter<'_>,
    ) -> Result<RunProgress, MontyException> {
        let ext_result = result.into();

        let Self {
            executor,
            vm_state,
            mut heap,
        } = self;

        let (converted, vm_state) =
            HeapReader::with(&mut heap, &mut (&executor, print), |reader, (executor, print)| {
                let mut vm = VM::restore(
                    vm_state,
                    &executor.module_code,
                    reader,
                    &executor.interns,
                    print.reborrow(),
                    executor.assert_repr_max_bytes,
                );

                let vm_result = match ext_result {
                    ExtFunctionResult::Return(obj) => vm.resume(obj),
                    ExtFunctionResult::Error(exc) => vm.resume_with_exception(exc.into()),
                    ExtFunctionResult::Future(raw_call_id) => {
                        let call_id = CallId::new(raw_call_id);
                        vm.add_pending_call(call_id);
                        vm.run_external()
                    }
                    ExtFunctionResult::NotFound(function_name) => {
                        vm.resume_with_exception(ExtFunctionResult::not_found_exc(&function_name))
                    }
                };

                // Three-phase: convert while VM alive, snapshot, build progress
                let converted = convert_frame_exit(vm_result, &mut vm);
                let vm_state = check_snapshot_from_converted(&converted, vm);
                (converted, vm_state)
            });
        build_run_progress(converted, vm_state, executor, heap)
    }

    /// Raises `exc` uncatchably at the suspension point.
    pub(crate) fn abort(self, exc: MontyException, print: PrintWriter<'_>) -> Result<RunProgress, MontyException> {
        let Self {
            executor,
            vm_state,
            heap,
        } = self;
        abort_restored(executor, vm_state, heap, exc, print)
    }
}

/// Restores the VM and aborts uncatchably, rolling back any armed OS effect.
fn abort_restored(
    executor: Executor,
    vm_state: VMSnapshot,
    mut heap: Heap,
    exc: MontyException,
    print: PrintWriter<'_>,
) -> Result<RunProgress, MontyException> {
    let (converted, vm_state) = HeapReader::with(&mut heap, &mut (&executor, print), |reader, (executor, print)| {
        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            reader,
            &executor.interns,
            print.reborrow(),
            executor.assert_repr_max_bytes,
        );
        let vm_result = vm.abort(exc);
        let converted = convert_frame_exit(vm_result, &mut vm);
        let vm_state = check_snapshot_from_converted(&converted, vm);
        (converted, vm_state)
    });
    build_run_progress(converted, vm_state, executor, heap)
}

pub use monty_types::{ExtFunctionResult, NameLookupResult};

/// Crate-internal extension for [`ExtFunctionResult`] (which lives in
/// `monty-types`): builds the interpreter-side `RunError` for a missing
/// external function.
pub(crate) trait ExtFunctionResultExt {
    /// The `NameError` raised when the host reports the function isn't defined.
    fn not_found_exc(function_name: &str) -> RunError {
        let msg = format!("name '{function_name}' is not defined");
        MontyException::new(ExcType::NameError, Some(msg)).into()
    }
}

impl ExtFunctionResultExt for ExtFunctionResult {}

// ---------------------------------------------------------------------------
// handle_vm_result
// ---------------------------------------------------------------------------

/// Pre-converted frame exit data, produced while the VM is still alive.
///
/// This intermediate enum holds `MontyObject`s and `String`s instead of `Value`s
/// and `StringId`s. It exists to separate the conversion phase (needs `&mut VM`)
/// from the snapshot/progress construction phase (needs owned `Heap`).
pub(crate) enum ConvertedExit {
    /// Execution completed with a final result.
    Complete(MontyObject),
    /// External function call, or a host-routed method call (`object_id`
    /// set; construction of a host class is a `__call__` method call).
    FunctionCall {
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        object_id: Option<MontyUuid>,
    },
    /// OS-level operation.
    OsCall {
        function_call: OsFunctionCall,
        call_id: u32,
    },
    /// All async tasks are blocked waiting for external futures.
    ResolveFutures(Vec<u32>),
    /// Unresolved name lookup or lazy instance attribute lookup.
    NameLookup { name: String, scope: LookupScope },
    /// Runtime error.
    Error(RunError),
}

impl ConvertedExit {
    /// Returns true if this exit requires a VM snapshot for later resumption.
    pub(crate) fn needs_snapshot(&self) -> bool {
        !matches!(self, Self::Complete(_) | Self::Error(_))
    }
}

/// Converts a `FrameExit` into a `ConvertedExit` while the VM is still alive.
///
/// All `Value` → `MontyObject` and `StringId` → `String` conversions happen here,
/// while the VM (and its heap/interns) are still accessible.
pub(crate) fn convert_frame_exit(result: RunResult<FrameExit>, vm: &mut VM<'_>) -> ConvertedExit {
    // An effect still armed on arrival belongs to an OS call that was answered
    // without consuming it — a host may reply `ExtFunctionResult::Future`,
    // whose resume never takes it. It can never apply to whatever suspends
    // next, so release it here rather than let it reshape an unrelated result
    // (or leak its file pin when the next OS call overwrites the slot).
    // Arming for *this* exit happens below, after the slot is clear.
    release_pending_effect(vm.pending_os_effect.take(), vm.heap);
    vm.pending_lookup_effect.take().drop_with(vm.heap);
    match result {
        Ok(FrameExit::Return(value)) => ConvertedExit::Complete(MontyObject::new(value, vm)),
        Ok(FrameExit::ExternalCall {
            function_name,
            args,
            call_id,
            ..
        }) => {
            let name = function_name.into_string(vm.interns);
            let (args_py, kwargs_py) = args.into_py_objects(vm);
            ConvertedExit::FunctionCall {
                function_name: name,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                object_id: None,
            }
        }
        Ok(FrameExit::OsCall {
            function_call,
            call_id,
            effect,
        }) => {
            // The point of no return: the call is the host's, so a matching
            // `resume` is guaranteed. Every other destination drops it.
            vm.pending_os_effect = effect;
            ConvertedExit::OsCall {
                function_call,
                call_id: call_id.raw(),
            }
        }
        Ok(FrameExit::MethodCall {
            method_name,
            args,
            call_id,
            object_id,
        }) => {
            let name = method_name.into_string(vm.interns);
            let (args_py, kwargs_py) = args.into_py_objects(vm);
            ConvertedExit::FunctionCall {
                function_name: name,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                object_id: Some(object_id),
            }
        }
        Ok(FrameExit::ResolveFutures(pending_call_ids)) => {
            ConvertedExit::ResolveFutures(pending_call_ids.iter().map(|id| id.raw()).collect())
        }
        Ok(FrameExit::NameLookup {
            name_id,
            namespace_slot,
            is_global,
        }) => {
            let name = vm.interns.get_str(name_id).to_owned();
            ConvertedExit::NameLookup {
                name,
                scope: LookupScope::Namespace {
                    namespace_slot,
                    is_global,
                },
            }
        }
        Ok(FrameExit::AttrLookup {
            name,
            class_name,
            object_id,
            type_object,
            effect,
        }) => {
            // The lookup is the host's now, so a `resume` is guaranteed to
            // consume the effect (or the next `convert_frame_exit` releases it).
            vm.pending_lookup_effect = effect;
            ConvertedExit::NameLookup {
                name: name.into_string(vm.interns),
                scope: LookupScope::Attr {
                    object_id,
                    class_name,
                    type_object,
                },
            }
        }
        Err(err) => ConvertedExit::Error(err),
    }
}

/// Decides whether to snapshot or clean up the VM based on the converted exit.
///
/// Consumes the VM. Returns `Some(VMSnapshot)` for suspendable exits, `None` for
/// completion/error (in which case the VM's `Drop` impl handles cleanup).
pub(crate) fn check_snapshot_from_converted(converted: &ConvertedExit, vm: VM<'_>) -> Option<VMSnapshot> {
    if converted.needs_snapshot() {
        Some(vm.snapshot())
    } else {
        None
    }
}

/// Assembles a `RunProgress` from already-converted data and owned heap.
///
/// This runs after the VM has been dropped (releasing the heap borrow),
/// so the heap can be moved into `Snapshot` structs.
pub(crate) fn build_run_progress(
    converted: ConvertedExit,
    vm_state: Option<VMSnapshot>,
    executor: Executor,
    heap: Heap,
) -> Result<RunProgress, MontyException> {
    macro_rules! new_snapshot {
        () => {
            Snapshot {
                executor,
                vm_state: vm_state.expect("snapshot should exist"),
                heap,
            }
        };
    }

    match converted {
        ConvertedExit::Complete(obj) => Ok(RunProgress::Complete(obj)),
        ConvertedExit::FunctionCall {
            function_name,
            args,
            kwargs,
            call_id,
            object_id,
        } => Ok(RunProgress::FunctionCall(FunctionCall::new(
            function_name,
            args,
            kwargs,
            call_id,
            object_id,
            new_snapshot!(),
        ))),
        ConvertedExit::OsCall { function_call, call_id } => Ok(RunProgress::OsCall(OsCall::new(
            function_call,
            call_id,
            new_snapshot!(),
        ))),
        ConvertedExit::ResolveFutures(pending_call_ids) => Ok(RunProgress::ResolveFutures(ResolveFutures::new(
            executor,
            vm_state.expect("snapshot should exist for ResolveFutures"),
            heap,
            pending_call_ids,
        ))),
        ConvertedExit::NameLookup { name, scope } => {
            Ok(RunProgress::NameLookup(NameLookup::new(name, scope, new_snapshot!())))
        }
        ConvertedExit::Error(err) => {
            Err(err.into_python_exception(&executor.interns, |_| Some(executor.code.as_str())))
        }
    }
}
