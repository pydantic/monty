//! This module defines the public types returned by [`MontyRun::start()`](crate::MontyRun::start)
//! and their resume methods. Each variant of [`RunProgress`] wraps a dedicated struct
//! (`FunctionCall`, `OsCall`, `NameLookup`, `ResolveFutures`) that carries only the
//! fields and resume methods relevant to that suspension point.
//!
//! The internal [`Snapshot`] type is `pub(crate)` — callers interact exclusively with
//! the per-variant structs.

use std::mem;

use crate::{
    ExcType, MontyException,
    asyncio::CallId,
    bytecode::{FrameExit, VM, VMSnapshot},
    exception_private::{RunError, RunResult},
    heap::Heap,
    io::PrintWriter,
    namespace::{GLOBAL_NS_IDX, NamespaceId, Namespaces},
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

impl<T: ResourceTracker + serde::de::DeserializeOwned> RunProgress<T> {
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
        print: &mut PrintWriter<'_>,
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
    pub fn resume_pending(self, print: &mut PrintWriter<'_>) -> Result<RunProgress<T>, MontyException> {
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
        print: &mut PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
        self.snapshot.run(result, print)
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
    /// Caches the resolved value in the namespace slot before restoring the VM,
    /// then either pushes the value onto the stack or raises `NameError`.
    ///
    /// # Arguments
    /// * `result` — The resolved value or `Undefined`.
    /// * `print` — Writer for print output.
    pub fn resume(
        mut self,
        result: impl Into<NameLookupResult>,
        print: &mut PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
        // Resolve the name lookup result BEFORE restoring the VM, since the VM
        // borrows heap/namespaces mutably and we need direct access for caching.
        let resolved_value = match result.into() {
            NameLookupResult::Value(obj) => {
                let value = obj
                    .to_value(&mut self.snapshot.heap, &self.snapshot.executor.interns)
                    .map_err(|e| MontyException::runtime_error(format!("invalid name lookup result: {e}")))?;

                // Cache the resolved value in the appropriate namespace slot.
                let ns_slot = NamespaceId::new(self.namespace_slot as usize);
                let ns_idx = if self.is_global {
                    GLOBAL_NS_IDX
                } else {
                    self.snapshot.vm_state.current_namespace_idx()
                };
                let namespace = self.snapshot.namespaces.get_mut(ns_idx);
                let old = mem::replace(namespace.get_mut(ns_slot), value.clone_with_heap(&self.snapshot.heap));
                old.drop_with_heap(&mut self.snapshot.heap);

                Some(value)
            }
            NameLookupResult::Undefined => None,
        };

        // Now restore the VM (borrows heap and namespaces)
        let mut vm = VM::restore(
            self.snapshot.vm_state,
            &self.snapshot.executor.module_code,
            &mut self.snapshot.heap,
            &mut self.snapshot.namespaces,
            &self.snapshot.executor.interns,
            print,
        );

        // Resume execution: either push the resolved value or raise NameError
        // through the VM so that traceback information is properly captured.
        let vm_result = if let Some(value) = resolved_value {
            vm.push(value);
            vm.run()
        } else {
            let err = ExcType::name_error(&self.name);
            vm.resume_with_exception(err.into())
        };
        let vm_state = vm.check_snapshot(&vm_result);
        handle_vm_result(
            vm_result,
            vm_state,
            self.snapshot.executor,
            self.snapshot.heap,
            self.snapshot.namespaces,
        )
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
    /// The VM state containing stack, frames, and exception state.
    vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    heap: Heap<T>,
    /// The namespaces containing all variable bindings.
    namespaces: Namespaces,
    /// The pending call_ids that this snapshot is waiting on.
    pending_call_ids: Vec<u32>,
}

impl<T: ResourceTracker> ResolveFutures<T> {
    /// Creates a new `ResolveFutures` from its parts.
    fn new(
        executor: Executor,
        vm_state: VMSnapshot,
        heap: Heap<T>,
        namespaces: Namespaces,
        pending_call_ids: Vec<u32>,
    ) -> Self {
        Self {
            executor,
            vm_state,
            heap,
            namespaces,
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
        print: &mut PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
        let Self {
            executor,
            vm_state,
            mut heap,
            mut namespaces,
            pending_call_ids,
        } = self;

        // Validate that all provided call_ids are in the pending set before restoring VM.
        let invalid_call_id = results
            .iter()
            .find(|(call_id, _)| !pending_call_ids.contains(call_id))
            .map(|(call_id, _)| *call_id);

        // Restore the VM from the snapshot (must happen before any error return to clean up properly).
        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            &mut heap,
            &mut namespaces,
            &executor.interns,
            print,
        );

        // Now check for invalid call_ids after VM is restored.
        if let Some(call_id) = invalid_call_id {
            vm.cleanup();
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);
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
            vm.cleanup();
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);
            return Err(error.into_python_exception(&executor.interns, &executor.code));
        }

        // Push resolved value for main task if it was blocked.
        let main_task_ready = vm.prepare_current_task_after_resolve();

        let loaded_task = match vm.load_ready_task_if_needed() {
            Ok(loaded) => loaded,
            Err(e) => {
                vm.cleanup();
                #[cfg(feature = "ref-count-panic")]
                namespaces.drop_global_with_heap(&mut heap);
                return Err(e.into_python_exception(&executor.interns, &executor.code));
            }
        };

        // If no task is ready and there are still pending calls, return ResolveFutures.
        if !main_task_ready && !loaded_task {
            let pending_call_ids = vm.get_pending_call_ids();
            if !pending_call_ids.is_empty() {
                let vm_state = vm.snapshot();
                let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
                return Ok(RunProgress::ResolveFutures(Self {
                    executor,
                    vm_state,
                    heap,
                    namespaces,
                    pending_call_ids,
                }));
            }
        }

        let result = vm.run();
        let vm_state = vm.check_snapshot(&result);
        handle_vm_result(result, vm_state, executor, heap, namespaces)
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
    /// The VM state containing stack, frames, and exception state.
    pub(crate) vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    pub(crate) heap: Heap<T>,
    /// The namespaces containing all variable bindings.
    pub(crate) namespaces: Namespaces,
}

impl<T: ResourceTracker> Snapshot<T> {
    /// Continues execution with the return value or exception from the external call.
    pub(crate) fn run(
        mut self,
        result: impl Into<ExtFunctionResult>,
        print: &mut PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
        let ext_result = result.into();

        let mut vm = VM::restore(
            self.vm_state,
            &self.executor.module_code,
            &mut self.heap,
            &mut self.namespaces,
            &self.executor.interns,
            print,
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

        let vm_state = vm.check_snapshot(&vm_result);
        handle_vm_result(vm_result, vm_state, self.executor, self.heap, self.namespaces)
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
// Executor (re-export from run.rs via pub(crate))
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// handle_vm_result
// ---------------------------------------------------------------------------

/// Converts a VM `FrameExit` result into the appropriate `RunProgress` variant.
///
/// This is used by both `Snapshot::run()` and `ResolveFutures::resume()` to
/// convert raw VM results into typed progress values.
#[cfg_attr(not(feature = "ref-count-panic"), expect(unused_mut))]
pub(crate) fn handle_vm_result<T: ResourceTracker>(
    result: RunResult<FrameExit>,
    vm_state: Option<VMSnapshot>,
    executor: Executor,
    mut heap: Heap<T>,
    mut namespaces: Namespaces,
) -> Result<RunProgress<T>, MontyException> {
    macro_rules! new_snapshot {
        () => {
            Snapshot {
                executor,
                vm_state: vm_state.expect("snapshot should exist"),
                heap,
                namespaces,
            }
        };
    }

    match result {
        Ok(FrameExit::Return(value)) => {
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);

            let obj = MontyObject::new(value, &mut heap, &executor.interns);
            Ok(RunProgress::Complete(obj))
        }
        Ok(FrameExit::ExternalCall {
            function_name,
            args,
            call_id,
            ..
        }) => {
            let function_name = function_name.into_string(&executor.interns);
            let (args_py, kwargs_py) = args.into_py_objects(&mut heap, &executor.interns);

            Ok(RunProgress::FunctionCall(FunctionCall::new(
                function_name,
                args_py,
                kwargs_py,
                call_id.raw(),
                false,
                new_snapshot!(),
            )))
        }
        Ok(FrameExit::OsCall {
            function,
            args,
            call_id,
        }) => {
            let (args_py, kwargs_py) = args.into_py_objects(&mut heap, &executor.interns);

            Ok(RunProgress::OsCall(OsCall::new(
                function,
                args_py,
                kwargs_py,
                call_id.raw(),
                new_snapshot!(),
            )))
        }
        Ok(FrameExit::MethodCall {
            method_name,
            args,
            call_id,
        }) => {
            let function_name = method_name.into_string(&executor.interns);
            let (args_py, kwargs_py) = args.into_py_objects(&mut heap, &executor.interns);

            Ok(RunProgress::FunctionCall(FunctionCall::new(
                function_name,
                args_py,
                kwargs_py,
                call_id.raw(),
                true,
                new_snapshot!(),
            )))
        }
        Ok(FrameExit::ResolveFutures(pending_call_ids)) => {
            let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
            Ok(RunProgress::ResolveFutures(ResolveFutures::new(
                executor,
                vm_state.expect("snapshot should exist for ResolveFutures"),
                heap,
                namespaces,
                pending_call_ids,
            )))
        }
        Ok(FrameExit::NameLookup {
            name_id,
            namespace_slot,
            is_global,
        }) => {
            let name = executor.interns.get_str(name_id).to_owned();
            Ok(RunProgress::NameLookup(NameLookup::new(
                name,
                namespace_slot,
                is_global,
                new_snapshot!(),
            )))
        }
        Err(err) => {
            #[cfg(feature = "ref-count-panic")]
            namespaces.drop_global_with_heap(&mut heap);

            Err(err.into_python_exception(&executor.interns, &executor.code))
        }
    }
}
