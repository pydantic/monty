//! This module defines the public types returned by [`MontyRun::start()`](crate::MontyRun::start)
//! and their resume methods. Each variant of [`RunProgress`] wraps a dedicated struct
//! (`FunctionCall`, `OsCall`, `NameLookup`, `ResolveFutures`) that carries only the
//! fields and resume methods relevant to that suspension point.
//!
//! The internal [`Snapshot`] type is `pub(crate)` — callers interact exclusively with
//! the per-variant structs.

use std::mem;

use serde::de::DeserializeOwned;

use crate::{
    ExcType, MontyException,
    asyncio::CallId,
    bytecode::{FrameExit, VM, VMSnapshot},
    exception_private::{RunError, RunResult},
    heap::{Heap, HeapReader},
    io::PrintWriter,
    object::MontyObject,
    os::OsFunction,
    resource::ResourceTracker,
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
/// # Type Parameters
/// * `T` — Resource tracker implementation (e.g. `NoLimitTracker` or `LimitedTracker`).
///
/// Serialization requires `T: Serialize + Deserialize`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub enum RunProgress<T: ResourceTracker> {
    /// Execution paused at an external function call or dataclass method call.
    FunctionCall(FunctionCall<T>),
    /// Execution paused for an OS-level operation (filesystem, network, etc.).
    OsCall(OsCall<T>),
    /// All async tasks are blocked waiting for external futures to resolve.
    ResolveFutures(ResolveFutures<T>),
    /// Execution paused for an unresolved name lookup.
    NameLookup(NameLookup<T>),
    /// Execution completed with a final result.
    Complete(MontyObject),
}

impl<T: ResourceTracker> RunProgress<T> {
    /// Consumes the progress and returns the `FunctionCall` struct if this is a function call.
    #[must_use]
    pub fn into_function_call(self) -> Option<FunctionCall<T>> {
        match self {
            Self::FunctionCall(call) => Some(call),
            _ => None,
        }
    }

    /// Consumes the progress and returns the `OsCall` struct if this is an OS call.
    #[must_use]
    pub fn into_os_call(self) -> Option<OsCall<T>> {
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

    /// Consumes the progress and returns the `ResolveFutures` struct.
    #[must_use]
    pub fn into_resolve_futures(self) -> Option<ResolveFutures<T>> {
        match self {
            Self::ResolveFutures(state) => Some(state),
            _ => None,
        }
    }

    /// Consumes the progress and returns the `NameLookup` struct.
    #[must_use]
    pub fn into_name_lookup(self) -> Option<NameLookup<T>> {
        match self {
            Self::NameLookup(lookup) => Some(lookup),
            _ => None,
        }
    }
}

impl<T: ResourceTracker + serde::Serialize> RunProgress<T> {
    /// Serializes the execution state to a binary format.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }
}

impl<T: ResourceTracker + DeserializeOwned> RunProgress<T> {
    /// Deserializes execution state from binary format.
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

// ---------------------------------------------------------------------------
// FunctionCall
// ---------------------------------------------------------------------------

/// Execution paused at an external function call or dataclass method call.
///
/// The host can choose how to handle this:
/// - **Sync resolution**: Call `resume(return_value, print)` to push the result and continue.
/// - **Async resolution**: Call `resume_pending(print)` to push an `ExternalFuture` and continue.
///
/// When using async resolution, the code continues and may `await` the future later.
/// If the future isn't resolved when awaited, execution yields with `ResolveFutures`.
///
/// When `method_call` is true, this represents a dataclass method call where the first
/// positional arg is the dataclass instance (`self`).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct FunctionCall<T: ResourceTracker> {
    /// The name of the function or method being called.
    pub function_name: String,
    /// The positional arguments passed to the function.
    pub args: Vec<MontyObject>,
    /// The keyword arguments passed to the function (key, value pairs).
    pub kwargs: Vec<(MontyObject, MontyObject)>,
    /// Unique identifier for this call (used for async correlation).
    pub call_id: u32,
    /// Whether this is a dataclass method call (first arg is `self`).
    pub method_call: bool,
    /// Internal execution snapshot.
    snapshot: Snapshot<T>,
}

impl<T: ResourceTracker> FunctionCall<T> {
    /// Creates a new `FunctionCall` from its parts.
    fn new(
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        method_call: bool,
        snapshot: Snapshot<T>,
    ) -> Self {
        Self {
            function_name,
            args,
            kwargs,
            call_id,
            method_call,
            snapshot,
        }
    }

    /// Returns a mutable reference to the resource tracker.
    ///
    /// This allows modifying resource limits between execution phases,
    /// e.g. setting a time limit before resuming after an external function call.
    pub fn tracker_mut(&mut self) -> &mut T {
        self.snapshot.heap.tracker_mut()
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
    ) -> Result<RunProgress<T>, MontyException> {
        self.snapshot.run(result, print)
    }

    /// Resumes execution by pushing an `ExternalFuture` instead of a concrete value.
    ///
    /// This is the async resolution pattern: the host continues execution with a
    /// pending future. The code can then `await` this future later. If the code
    /// awaits the future before it's resolved, execution will yield with
    /// `RunProgress::ResolveFutures`.
    ///
    /// Uses `self.call_id` internally — no need to pass it again.
    ///
    /// # Arguments
    /// * `print` — Writer for print output.
    pub fn resume_pending(self, print: PrintWriter<'_>) -> Result<RunProgress<T>, MontyException> {
        self.snapshot.run(ExtFunctionResult::Future(self.call_id), print)
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
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct OsCall<T: ResourceTracker> {
    /// The OS function to execute.
    pub function: OsFunction,
    /// The positional arguments for the OS function.
    pub args: Vec<MontyObject>,
    /// The keyword arguments passed to the function (key, value pairs).
    pub kwargs: Vec<(MontyObject, MontyObject)>,
    /// Unique identifier for this call (used for async correlation).
    pub call_id: u32,
    /// Internal execution snapshot.
    snapshot: Snapshot<T>,
}

impl<T: ResourceTracker> OsCall<T> {
    /// Creates a new `OsCall` from its parts.
    fn new(
        function: OsFunction,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        snapshot: Snapshot<T>,
    ) -> Self {
        Self {
            function,
            args,
            kwargs,
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
    ) -> Result<RunProgress<T>, MontyException> {
        self.snapshot.run(result.into(), print)
    }
}

// ---------------------------------------------------------------------------
// NameLookup
// ---------------------------------------------------------------------------

/// Execution paused for an unresolved name lookup.
///
/// The host should check if the name corresponds to a known external function or
/// value. Call `resume(result, print)` with `NameLookupResult::Value(obj)` to
/// cache it in the namespace and continue, or `NameLookupResult::Undefined` to
/// raise `NameError`.
///
/// The namespace slot and scope are managed internally — the host only needs to
/// provide the name resolution result.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct NameLookup<T: ResourceTracker> {
    /// The name being looked up.
    pub name: String,
    /// The namespace slot where the resolved value should be cached.
    namespace_slot: u16,
    /// Whether this is a global slot or a local/function slot.
    is_global: bool,
    /// Internal execution snapshot.
    snapshot: Snapshot<T>,
}

impl<T: ResourceTracker> NameLookup<T> {
    /// Creates a new `NameLookup` from its parts.
    fn new(name: String, namespace_slot: u16, is_global: bool, snapshot: Snapshot<T>) -> Self {
        Self {
            name,
            namespace_slot,
            is_global,
            snapshot,
        }
    }

    /// Resumes execution after name resolution.
    ///
    /// Caches the resolved value in the appropriate slot (globals or stack)
    /// before restoring the VM, then either pushes the value or raises `NameError`.
    ///
    /// # Arguments
    /// * `result` — The resolved value or `Undefined`.
    /// * `print` — Writer for print output.
    pub fn resume(
        mut self,
        result: impl Into<NameLookupResult>,
        mut print: PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
        let result = result.into();

        let (converted, vm_state) = HeapReader::with(&mut self.snapshot.heap, |heap| {
            // Restore the VM first, then convert inside its lifetime
            let mut vm = VM::restore(
                self.snapshot.vm_state,
                &self.snapshot.executor.module_code,
                heap,
                &self.snapshot.executor.interns,
                print.reborrow(),
            );

            // Resolve the name lookup result with the VM alive
            let vm_result = match result {
                NameLookupResult::Value(obj) => {
                    let value = obj
                        .to_value(&mut vm)
                        .map_err(|e| MontyException::runtime_error(format!("invalid name lookup result: {e}")))?;

                    // Cache the resolved value in the appropriate slot
                    let slot = self.namespace_slot as usize;
                    if self.is_global {
                        let cloned = value.clone_with_heap(&vm);
                        let old = mem::replace(&mut vm.globals[slot], cloned);
                        old.drop_with_heap(&mut vm);
                    } else {
                        let stack_base = vm.current_stack_base();
                        let cloned = value.clone_with_heap(&vm);
                        let old = mem::replace(&mut vm.stack[stack_base + slot], cloned);
                        old.drop_with_heap(&mut vm);
                    }

                    vm.push(value);
                    vm.run()
                }
                NameLookupResult::Undefined => {
                    let err = ExcType::name_error(&self.name);
                    vm.resume_with_exception(err.into())
                }
            };

            // Three-phase: convert while VM alive, snapshot, build progress
            let converted = convert_frame_exit(vm_result, &mut vm);
            let vm_state = check_snapshot_from_converted(&converted, vm);
            Ok((converted, vm_state))
        })?;
        build_run_progress(converted, vm_state, self.snapshot.executor, self.snapshot.heap)
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
/// Use `pending_call_ids()` to see which calls are pending, then call
/// `resume(results, print)` with some or all of the results.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ResolveFutures<T: ResourceTracker> {
    /// The executor containing compiled code and interns.
    executor: Executor,
    /// The VM state containing stack, frames, globals, and exception state.
    vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    heap: Heap<T>,
    /// The pending call_ids that this snapshot is waiting on.
    pending_call_ids: Vec<u32>,
}

impl<T: ResourceTracker> ResolveFutures<T> {
    /// Creates a new `ResolveFutures` from its parts.
    fn new(executor: Executor, vm_state: VMSnapshot, heap: Heap<T>, pending_call_ids: Vec<u32>) -> Self {
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

    /// Resumes execution with results for some or all pending futures.
    ///
    /// **Incremental resolution**: You don't need to provide all results at once.
    /// If you provide a partial list, Monty will:
    /// 1. Mark those futures as resolved
    /// 2. Unblock any tasks waiting on those futures
    /// 3. Continue running until all tasks are blocked again
    /// 4. Return `ResolveFutures` with the remaining pending calls
    ///
    /// # Arguments
    /// * `results` — List of `(call_id, result)` pairs. Can be a subset of pending calls.
    /// * `print` — Writer for print output.
    ///
    /// # Errors
    /// Returns `Err(MontyException)` if any `call_id` in `results` is not in the pending set.
    pub fn resume(
        self,
        results: Vec<(u32, ExtFunctionResult)>,
        mut print: PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
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

        let (converted, vm_state) = HeapReader::with(&mut heap, |heap| {
            // Restore the VM from the snapshot (must happen before any error return to clean up properly).
            let mut vm = VM::restore(
                vm_state,
                &executor.module_code,
                heap,
                &executor.interns,
                print.reborrow(),
            );

            // Now check for invalid call_ids after VM is restored.
            if let Some(call_id) = invalid_call_id {
                return Err(MontyException::runtime_error(format!(
                    "unknown call_id {call_id}, expected one of: {pending_call_ids:?}"
                )));
            }

            for (call_id, ext_result) in results {
                match ext_result {
                    ExtFunctionResult::Return(obj) => vm.resolve_future(call_id, obj).map_err(|e| {
                        MontyException::runtime_error(format!("Invalid return type for call {call_id}: {e}"))
                    })?,
                    ExtFunctionResult::Error(exc) => vm.fail_future(call_id, exc.into()),
                    ExtFunctionResult::Future(_) => {}
                    ExtFunctionResult::NotFound(function_name) => {
                        vm.fail_future(call_id, ExtFunctionResult::not_found_exc(&function_name));
                    }
                }
            }

            // Check if the current task has failed.
            if let Some(error) = vm.take_failed_task_error() {
                return Err(error.into_python_exception(&executor.interns, &executor.code));
            }

            // Push resolved value for main task if it was blocked.
            let main_task_ready = vm.prepare_current_task_after_resolve();

            let loaded_task = match vm.load_ready_task_if_needed() {
                Ok(loaded) => loaded,
                Err(e) => {
                    return Err(e.into_python_exception(&executor.interns, &executor.code));
                }
            };

            // If no task is ready and there are still pending calls, return ResolveFutures.
            if !main_task_ready && !loaded_task {
                let pending_call_ids = vm.get_pending_call_ids();
                if !pending_call_ids.is_empty() {
                    let vm_state = vm.snapshot();
                    let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
                    return Ok((ConvertedExit::ResolveFutures(pending_call_ids), Some(vm_state)));
                }
            }

            let result = vm.run();

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
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub(crate) struct Snapshot<T: ResourceTracker> {
    /// The executor containing compiled code and interns.
    pub(crate) executor: Executor,
    /// The VM state containing stack, frames, globals, and exception state.
    pub(crate) vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    pub(crate) heap: Heap<T>,
}

impl<T: ResourceTracker> Snapshot<T> {
    /// Continues execution with the return value or exception from the external call.
    pub(crate) fn run(
        mut self,
        result: impl Into<ExtFunctionResult>,
        mut print: PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
        let ext_result = result.into();

        let (converted, vm_state) = HeapReader::with(&mut self.heap, |heap| {
            let mut vm = VM::restore(
                self.vm_state,
                &self.executor.module_code,
                heap,
                &self.executor.interns,
                print.reborrow(),
            );

            let vm_result = match ext_result {
                ExtFunctionResult::Return(obj) => vm.resume(obj),
                ExtFunctionResult::Error(exc) => vm.resume_with_exception(exc.into()),
                ExtFunctionResult::Future(raw_call_id) => {
                    let call_id = CallId::new(raw_call_id);
                    vm.add_pending_call(call_id);
                    vm.push(Value::ExternalFuture(call_id));
                    vm.run()
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
        build_run_progress(converted, vm_state, self.executor, self.heap)
    }
}

/// Result of a name lookup from the host.
///
/// When the VM encounters an unresolved name, the host provides one of these:
/// - `Value(obj)`: The name resolves to this value (cached in the namespace for future access).
/// - `Undefined`: The name is truly undefined, causing `NameError`.
#[derive(Debug)]
pub enum NameLookupResult {
    /// The name resolves to this value.
    Value(MontyObject),
    /// The name is undefined — VM will raise `NameError`.
    Undefined,
}

impl From<MontyObject> for NameLookupResult {
    fn from(value: MontyObject) -> Self {
        Self::Value(value)
    }
}

/// Return value or exception from an external function.
#[derive(Debug)]
pub enum ExtFunctionResult {
    /// Continues execution with the return value from the external function.
    Return(MontyObject),
    /// Continues execution with the exception raised by the external function.
    Error(MontyException),
    /// Pending future — the external function is a coroutine.
    ///
    /// The `u32` is the `call_id` from the `FunctionCall` that created this
    /// snapshot. It is used to track the pending future so it can be resolved
    /// later via `ResolveFutures::resume()`.
    Future(u32),
    /// The function was not found, should result in a `NameError` exception.
    NotFound(String),
}

impl ExtFunctionResult {
    pub(crate) fn not_found_exc(function_name: &str) -> RunError {
        let msg = format!("name '{function_name}' is not defined");
        MontyException::new(ExcType::NameError, Some(msg)).into()
    }
}

impl From<MontyObject> for ExtFunctionResult {
    fn from(value: MontyObject) -> Self {
        Self::Return(value)
    }
}

impl From<MontyException> for ExtFunctionResult {
    fn from(exception: MontyException) -> Self {
        Self::Error(exception)
    }
}

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
    /// External function call or dataclass method call.
    FunctionCall {
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        method_call: bool,
    },
    /// OS-level operation.
    OsCall {
        function: OsFunction,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
    },
    /// All async tasks are blocked waiting for external futures.
    ResolveFutures(Vec<u32>),
    /// Unresolved name lookup.
    NameLookup {
        name: String,
        namespace_slot: u16,
        is_global: bool,
    },
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
pub(crate) fn convert_frame_exit(
    result: RunResult<FrameExit>,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> ConvertedExit {
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
                method_call: false,
            }
        }
        Ok(FrameExit::OsCall {
            function,
            args,
            call_id,
        }) => {
            let (args_py, kwargs_py) = args.into_py_objects(vm);
            ConvertedExit::OsCall {
                function,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
            }
        }
        Ok(FrameExit::MethodCall {
            method_name,
            args,
            call_id,
        }) => {
            let name = method_name.into_string(vm.interns);
            let (args_py, kwargs_py) = args.into_py_objects(vm);
            ConvertedExit::FunctionCall {
                function_name: name,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                method_call: true,
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
                namespace_slot,
                is_global,
            }
        }
        Err(err) => ConvertedExit::Error(err),
    }
}

/// Decides whether to snapshot or clean up the VM based on the converted exit.
///
/// Consumes the VM. Returns `Some(VMSnapshot)` for suspendable exits, `None` for
/// completion/error (in which case the VM's `Drop` impl handles cleanup).
pub(crate) fn check_snapshot_from_converted(
    converted: &ConvertedExit,
    vm: VM<'_, '_, impl ResourceTracker>,
) -> Option<VMSnapshot> {
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
pub(crate) fn build_run_progress<T: ResourceTracker>(
    converted: ConvertedExit,
    vm_state: Option<VMSnapshot>,
    executor: Executor,
    heap: Heap<T>,
) -> Result<RunProgress<T>, MontyException> {
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
            method_call,
        } => Ok(RunProgress::FunctionCall(FunctionCall::new(
            function_name,
            args,
            kwargs,
            call_id,
            method_call,
            new_snapshot!(),
        ))),
        ConvertedExit::OsCall {
            function,
            args,
            kwargs,
            call_id,
        } => Ok(RunProgress::OsCall(OsCall::new(
            function,
            args,
            kwargs,
            call_id,
            new_snapshot!(),
        ))),
        ConvertedExit::ResolveFutures(pending_call_ids) => Ok(RunProgress::ResolveFutures(ResolveFutures::new(
            executor,
            vm_state.expect("snapshot should exist for ResolveFutures"),
            heap,
            pending_call_ids,
        ))),
        ConvertedExit::NameLookup {
            name,
            namespace_slot,
            is_global,
        } => Ok(RunProgress::NameLookup(NameLookup::new(
            name,
            namespace_slot,
            is_global,
            new_snapshot!(),
        ))),
        ConvertedExit::Error(err) => Err(err.into_python_exception(&executor.interns, &executor.code)),
    }
}
