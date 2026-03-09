//! Stateful REPL execution support for Monty.
//!
//! This module implements incremental snippet execution where each new snippet
//! is compiled and executed against persistent heap/namespace state without
//! replaying previously executed snippets.

use std::mem;

use ahash::AHashMap;
use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{InterpolatedStringErrorType, LexicalErrorType, ParseErrorType, parse_module};

use crate::{
    ExcType, MontyException,
    asyncio::CallId,
    bytecode::{Code, Compiler, FrameExit, VM, VMSnapshot},
    exception_private::{RunError, RunResult},
    heap::{DropWithHeap, Heap},
    intern::{InternerBuilder, Interns},
    io::PrintWriter,
    namespace::NamespaceId,
    object::MontyObject,
    os::OsFunction,
    parse::parse_with_interner,
    prepare::prepare_with_existing_names,
    resource::ResourceTracker,
    run_progress::{ExtFunctionResult, NameLookupResult},
    value::Value,
};

/// Stateful REPL session that executes snippets incrementally without replay.
///
/// `MontyRepl` preserves heap and global variable state between snippets.
/// Each `feed()` compiles and executes only the new snippet against the current
/// state, avoiding the cost and semantic risks of replaying prior code.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct MontyRepl<T: ResourceTracker> {
    /// Script name used for runtime error messages and REPL identification.
    ///
    /// Incremental `feed()` / `start()` snippets intentionally use internal script names
    /// like `<python-input-0>` to match CPython's interactive traceback style.
    script_name: String,
    /// Counter for generated `<python-input-N>` snippet filenames.
    next_input_id: u64,
    /// Stable mapping of global variable names to namespace slot IDs.
    global_name_map: AHashMap<String, NamespaceId>,
    /// Persistent intern table across snippets so intern/function IDs remain valid.
    interns: Interns,
    /// Persistent heap across snippets.
    heap: Heap<T>,
    /// Persistent global variables across snippets.
    ///
    /// Indexed by slot number from the compiler's global-name map.
    globals: Vec<Value>,
}

impl<T: ResourceTracker> MontyRepl<T> {
    /// Creates an empty REPL session with no code parsed or executed.
    ///
    /// All code execution is driven through `feed_run()` or `feed_start()`. This separates
    /// construction from execution, matching the pattern used by `MontyRun::new()`.
    #[must_use]
    pub fn new(script_name: &str, resource_tracker: T) -> Self {
        let heap = Heap::new(0, resource_tracker);

        Self {
            script_name: script_name.to_owned(),
            next_input_id: 0,
            global_name_map: AHashMap::new(),
            interns: Interns::new(InternerBuilder::default(), Vec::new()),
            heap,
            globals: Vec::new(),
        }
    }

    /// Starts executing a new snippet and returns suspendable REPL progress.
    ///
    /// This is the REPL equivalent of `MontyRun::start`: execution may complete,
    /// suspend at external calls / OS calls / unresolved futures, or raise a Python
    /// exception. Resume with the returned state object and eventually recover the
    /// updated REPL from `ReplProgress::into_complete`.
    ///
    /// Unlike `MontyRepl::feed`, this method consumes `self` so runtime state can be
    /// safely moved into snapshot objects for serialization and cross-process resume.
    ///
    /// On a Python-level runtime exception the REPL is **not** destroyed: it is
    /// returned inside `ReplStartError` so the caller can continue feeding
    /// subsequent snippets against the same heap and namespace state.
    ///
    /// # Errors
    /// Returns `Err(Box<ReplStartError>)` for syntax, compile-time, or runtime
    /// failures — the REPL session is always preserved inside the error.
    pub fn feed_start(
        self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        print: PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let mut this = self;
        if code.is_empty() {
            return Ok(ReplProgress::Complete {
                repl: this,
                value: MontyObject::None,
            });
        }

        let (input_names, input_values): (Vec<_>, Vec<_>) = inputs.into_iter().unzip();

        let input_script_name = this.next_input_script_name();
        let executor = match ReplExecutor::new_repl_snippet(
            code.to_owned(),
            &input_script_name,
            this.global_name_map.clone(),
            &this.interns,
            input_names,
        ) {
            Ok(exec) => exec,
            Err(error) => return Err(Box::new(ReplStartError { repl: this, error })),
        };

        this.ensure_globals_size(executor.namespace_size);
        if let Err(error) = this.inject_inputs(input_values, &executor) {
            return Err(Box::new(ReplStartError { repl: this, error }));
        }

        // Move globals into the VM for execution.
        let globals = mem::take(&mut this.globals);
        let mut vm = VM::new(globals, &mut this.heap, &executor.interns, print);
        let vm_result = vm.run_module(&executor.module_code);

        if matches!(
            &vm_result,
            Ok(FrameExit::ExternalCall { .. }
                | FrameExit::OsCall { .. }
                | FrameExit::MethodCall { .. }
                | FrameExit::ResolveFutures(_)
                | FrameExit::NameLookup { .. })
        ) {
            // Snapshot path: globals move into the VMSnapshot.
            let vm_state = vm.snapshot();
            handle_repl_vm_result(vm_result, Some(vm_state), executor, this)
        } else {
            // Completion/error path: reclaim globals before cleanup.
            this.globals = vm.take_globals();
            vm.cleanup();
            handle_repl_vm_result(vm_result, None, executor, this)
        }
    }

    /// Feeds and executes a new snippet against the current REPL state to completion.
    ///
    /// This compiles only `code` using the existing global slot map, extends the
    /// global namespace if new names are introduced, and executes the snippet once.
    /// Previously executed snippets are never replayed. If execution raises after
    /// partially mutating globals, those mutations remain visible in later feeds,
    /// matching Python REPL semantics.
    ///
    /// # Errors
    /// Returns `MontyException` for syntax/compile/runtime failures.
    pub fn feed_run(
        &mut self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        if code.is_empty() {
            return Ok(MontyObject::None);
        }

        let (input_names, input_values): (Vec<_>, Vec<_>) = inputs.into_iter().unzip();

        let input_script_name = self.next_input_script_name();
        let executor = ReplExecutor::new_repl_snippet(
            code.to_owned(),
            &input_script_name,
            self.global_name_map.clone(),
            &self.interns,
            input_names,
        )?;

        let ReplExecutor {
            namespace_size,
            name_map,
            module_code,
            interns,
            code,
            input_names,
        } = executor;

        self.ensure_globals_size(namespace_size);

        // Inject input values into their pre-assigned namespace slots.
        for (name, obj) in input_names.iter().zip(input_values) {
            let slot = name_map[name];
            let value = obj
                .to_value(&mut self.heap, &interns)
                .map_err(|e| MontyException::runtime_error(format!("invalid input type: {e}")))?;
            let old = mem::replace(&mut self.globals[slot.index()], value);
            old.drop_with_heap(&mut self.heap);
        }

        // Move globals into the VM for execution, reclaim after completion.
        let globals = mem::take(&mut self.globals);
        let mut vm = VM::new(globals, &mut self.heap, &interns, print);
        let mut frame_exit_result = vm.run_module(&module_code);

        // Handle NameLookup exits by raising NameError through the VM so that
        // traceback information is properly captured. In the non-iterative REPL path,
        // there's no host to resolve names, so all NameLookup exits become NameErrors.
        while let Ok(FrameExit::NameLookup { name_id, .. }) = &frame_exit_result {
            let name = interns.get_str(*name_id);
            let err = ExcType::name_error(name);
            frame_exit_result = vm.resume_with_exception(err.into());
        }

        // Reclaim globals before cleanup.
        self.globals = vm.take_globals();
        vm.cleanup();

        // Commit compiler metadata even on runtime errors.
        // Snippets can mutate globals before raising, and those values may contain
        // FunctionId/StringId values that must be interpreted with the updated tables.
        self.global_name_map = name_map;
        self.interns = interns;

        frame_exit_to_object(frame_exit_result, &mut self.heap, &self.interns)
            .map_err(|e| e.into_python_exception(&self.interns, &code))
    }

    /// Grows the globals vector to at least `size` slots.
    ///
    /// Newly introduced slots are initialized to `Undefined` to keep slot alignment
    /// with the compiler's global-name map.
    fn ensure_globals_size(&mut self, size: usize) {
        if self.globals.len() < size {
            self.globals.resize_with(size, || Value::Undefined);
        }
    }

    /// Converts `MontyObject` inputs to `Value`s and places them in their assigned
    /// namespace slots before execution begins.
    ///
    /// Input names were pre-registered in the name map by `new_repl_snippet`, so their
    /// slots are already allocated. This just converts and stores the values.
    fn inject_inputs(&mut self, inputs: Vec<MontyObject>, executor: &ReplExecutor) -> Result<(), MontyException> {
        for (name, obj) in executor.input_names.iter().zip(inputs) {
            let slot = executor.name_map[name];
            let value = obj
                .to_value(&mut self.heap, &executor.interns)
                .map_err(|e| MontyException::runtime_error(format!("invalid input type: {e}")))?;
            let old = mem::replace(&mut self.globals[slot.index()], value);
            old.drop_with_heap(&mut self.heap);
        }
        Ok(())
    }

    /// Returns the generated filename for the next interactive snippet.
    ///
    /// CPython labels interactive snippets as `<python-input-N>` and increments
    /// N for each feed attempt. Matching this improves traceback ergonomics and
    /// makes REPL errors easier to correlate with user input history.
    fn next_input_script_name(&mut self) -> String {
        let input_id = self.next_input_id;
        self.next_input_id += 1;
        format!("<python-input-{input_id}>")
    }
}

impl<T: ResourceTracker + serde::Serialize> MontyRepl<T> {
    /// Serializes the REPL session state to bytes.
    ///
    /// This includes heap + globals + global slot mapping, allowing snapshot/restore
    /// of interactive state between process runs.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }
}

impl<T: ResourceTracker + serde::de::DeserializeOwned> MontyRepl<T> {
    /// Restores a REPL session from bytes produced by `MontyRepl::dump`.
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

impl<T: ResourceTracker> Drop for MontyRepl<T> {
    fn drop(&mut self) {
        self.globals.drain(..).drop_with_heap(&mut self.heap);
    }
}

// ---------------------------------------------------------------------------
// ReplProgress and per-variant structs
// ---------------------------------------------------------------------------

/// Result of a single suspendable REPL snippet execution.
///
/// This mirrors `RunProgress` but returns the updated `MontyRepl` on completion
/// so callers can continue feeding additional snippets without replaying prior code.
/// Each variant (except `Complete`) wraps a dedicated struct with only the relevant
/// resume methods.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub enum ReplProgress<T: ResourceTracker> {
    /// Execution paused at an external function call or dataclass method call.
    FunctionCall(ReplFunctionCall<T>),
    /// Execution paused for an OS-level operation.
    OsCall(ReplOsCall<T>),
    /// All async tasks are blocked waiting for external futures to resolve.
    ResolveFutures(ReplResolveFutures<T>),
    /// Execution paused for an unresolved name lookup.
    NameLookup(ReplNameLookup<T>),
    /// Snippet execution completed with the updated REPL and result value.
    Complete {
        /// Updated REPL session state to continue feeding snippets.
        repl: MontyRepl<T>,
        /// Final result produced by the snippet.
        value: MontyObject,
    },
}

/// Error returned when a REPL snippet raises a Python exception during `start()` or `resume()`.
///
/// Unlike syntax/compile errors which consume the REPL, runtime errors preserve
/// the full session state so the caller can inspect the error and continue feeding
/// subsequent snippets. Any global mutations that occurred before the exception
/// remain visible in the returned `repl`.
#[derive(Debug)]
pub struct ReplStartError<T: ResourceTracker> {
    /// REPL session state after the failed snippet — ready for further use.
    pub repl: MontyRepl<T>,
    /// The Python exception that was raised.
    pub error: MontyException,
}

impl<T: ResourceTracker> ReplProgress<T> {
    /// Consumes the progress and returns the `ReplFunctionCall` struct.
    #[must_use]
    pub fn into_function_call(self) -> Option<ReplFunctionCall<T>> {
        match self {
            Self::FunctionCall(call) => Some(call),
            _ => None,
        }
    }

    /// Consumes the progress and returns the `ReplResolveFutures` struct.
    #[must_use]
    pub fn into_resolve_futures(self) -> Option<ReplResolveFutures<T>> {
        match self {
            Self::ResolveFutures(state) => Some(state),
            _ => None,
        }
    }

    /// Consumes the progress and returns the `ReplNameLookup` struct.
    #[must_use]
    pub fn into_name_lookup(self) -> Option<ReplNameLookup<T>> {
        match self {
            Self::NameLookup(lookup) => Some(lookup),
            _ => None,
        }
    }

    /// Consumes the progress and returns the completed REPL and value.
    #[must_use]
    pub fn into_complete(self) -> Option<(MontyRepl<T>, MontyObject)> {
        match self {
            Self::Complete { repl, value } => Some((repl, value)),
            _ => None,
        }
    }

    /// Extracts the REPL session from any progress variant, discarding
    /// the in-flight execution state.
    ///
    /// Use this to recover the REPL when you need to abandon the current
    /// snippet (e.g. because `feed_run` doesn't support async futures).
    /// The REPL state reflects any mutations that occurred before the
    /// snapshot was taken.
    #[must_use]
    pub fn into_repl(self) -> MontyRepl<T> {
        match self {
            Self::FunctionCall(call) => call.into_repl(),
            Self::OsCall(call) => call.into_repl(),
            Self::ResolveFutures(state) => state.into_repl(),
            Self::NameLookup(lookup) => lookup.into_repl(),
            Self::Complete { repl, .. } => repl,
        }
    }
}

impl<T: ResourceTracker + serde::Serialize> ReplProgress<T> {
    /// Serializes the REPL execution progress to a binary format.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }
}

impl<T: ResourceTracker + serde::de::DeserializeOwned> ReplProgress<T> {
    /// Deserializes REPL execution progress from a binary format.
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

// ---------------------------------------------------------------------------
// ReplFunctionCall
// ---------------------------------------------------------------------------

/// REPL execution paused at an external function call or dataclass method call.
///
/// Resume with `resume(result, print)` to provide the return value and continue,
/// or `resume_pending(print)` to push an `ExternalFuture` for async resolution.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplFunctionCall<T: ResourceTracker> {
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
    /// Internal REPL execution snapshot.
    snapshot: ReplSnapshot<T>,
}

impl<T: ResourceTracker> ReplFunctionCall<T> {
    /// Extracts the REPL session, discarding the in-flight execution state.
    ///
    /// Restores globals from the VM snapshot so the REPL remains usable.
    #[must_use]
    pub fn into_repl(self) -> MontyRepl<T> {
        self.snapshot.into_repl()
    }

    /// Resumes snippet execution with an external result.
    pub fn resume(
        self,
        result: impl Into<ExtFunctionResult>,
        print: PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        self.snapshot.run(result, print)
    }

    /// Resumes execution by pushing an `ExternalFuture` for async resolution.
    ///
    /// Uses `self.call_id` internally — no need to pass it again.
    pub fn resume_pending(self, print: PrintWriter<'_>) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        self.snapshot.run(ExtFunctionResult::Future(self.call_id), print)
    }
}

// ---------------------------------------------------------------------------
// ReplOsCall
// ---------------------------------------------------------------------------

/// REPL execution paused for an OS-level operation.
///
/// Resume with `resume(result, print)` to provide the OS call result and continue.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplOsCall<T: ResourceTracker> {
    /// The OS function to execute.
    pub function: OsFunction,
    /// The positional arguments for the OS function.
    pub args: Vec<MontyObject>,
    /// The keyword arguments passed to the function (key, value pairs).
    pub kwargs: Vec<(MontyObject, MontyObject)>,
    /// Unique identifier for this call (used for async correlation).
    pub call_id: u32,
    /// Internal REPL execution snapshot.
    snapshot: ReplSnapshot<T>,
}

impl<T: ResourceTracker> ReplOsCall<T> {
    /// Extracts the REPL session, discarding the in-flight execution state.
    ///
    /// Restores globals from the VM snapshot so the REPL remains usable.
    #[must_use]
    pub fn into_repl(self) -> MontyRepl<T> {
        self.snapshot.into_repl()
    }

    /// Resumes snippet execution with the OS call result.
    pub fn resume(
        self,
        result: impl Into<ExtFunctionResult>,
        print: PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        self.snapshot.run(result, print)
    }
}

// ---------------------------------------------------------------------------
// ReplNameLookup
// ---------------------------------------------------------------------------

/// REPL execution paused for an unresolved name lookup.
///
/// The host should check if the name corresponds to a known external function or
/// value. Call `resume(result, print)` with the appropriate `NameLookupResult`.
/// The namespace slot and scope are managed internally.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplNameLookup<T: ResourceTracker> {
    /// The name being looked up.
    pub name: String,
    /// The namespace slot where the resolved value should be cached.
    namespace_slot: u16,
    /// Whether this is a global slot or a local/function slot.
    is_global: bool,
    /// Internal REPL execution snapshot.
    snapshot: ReplSnapshot<T>,
}

impl<T: ResourceTracker> ReplNameLookup<T> {
    /// Extracts the REPL session, discarding the in-flight execution state.
    ///
    /// Restores globals from the VM snapshot so the REPL remains usable.
    #[must_use]
    pub fn into_repl(self) -> MontyRepl<T> {
        self.snapshot.into_repl()
    }

    /// Resumes execution after name resolution.
    ///
    /// Caches the resolved value in the namespace slot before restoring the VM,
    /// then either pushes the value onto the stack or raises `NameError`.
    pub fn resume(
        self,
        result: NameLookupResult,
        print: PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let Self {
            name,
            namespace_slot,
            is_global,
            snapshot,
        } = self;

        let ReplSnapshot {
            mut repl,
            executor,
            mut vm_state,
        } = snapshot;

        // Resolve the name lookup result BEFORE restoring the VM, since the VM
        // borrows heap mutably and we need direct access for caching.
        let resolved_value = match result {
            NameLookupResult::Value(obj) => {
                let value = match obj.to_value(&mut repl.heap, &executor.interns) {
                    Ok(v) => v,
                    Err(e) => {
                        let error = MontyException::runtime_error(format!("invalid name lookup result: {e}"));
                        return Err(Box::new(ReplStartError { repl, error }));
                    }
                };

                // Cache in the appropriate slot (globals vector or VM stack).
                let slot = namespace_slot as usize;
                let target = if is_global {
                    &mut vm_state.globals[slot]
                } else {
                    let stack_base = vm_state.current_stack_base();
                    &mut vm_state.stack[stack_base + slot]
                };
                let old = mem::replace(target, value.clone_with_heap(&repl.heap));
                old.drop_with_heap(&mut repl.heap);

                Some(value)
            }
            NameLookupResult::Undefined => None,
        };

        // Now restore the VM
        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            &mut repl.heap,
            &executor.interns,
            print,
        );

        // Resume execution: either push the resolved value or raise NameError
        // through the VM so that traceback information is properly captured.
        let vm_result = if let Some(value) = resolved_value {
            vm.push(value);
            vm.run()
        } else {
            let err: RunError = ExcType::name_error(&name).into();
            vm.resume_with_exception(err)
        };

        // Handle snapshot vs completion: reclaim globals on the completion path.
        if matches!(
            &vm_result,
            Ok(FrameExit::ExternalCall { .. }
                | FrameExit::OsCall { .. }
                | FrameExit::MethodCall { .. }
                | FrameExit::ResolveFutures(_)
                | FrameExit::NameLookup { .. })
        ) {
            let vm_state = vm.snapshot();
            handle_repl_vm_result(vm_result, Some(vm_state), executor, repl)
        } else {
            repl.globals = vm.take_globals();
            vm.cleanup();
            handle_repl_vm_result(vm_result, None, executor, repl)
        }
    }
}

// ---------------------------------------------------------------------------
// ReplResolveFutures
// ---------------------------------------------------------------------------

/// REPL execution state blocked on unresolved external futures.
///
/// This is the REPL-aware counterpart to `ResolveFutures`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplResolveFutures<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: ReplExecutor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
    /// Pending call IDs expected by this snapshot.
    pending_call_ids: Vec<u32>,
}

impl<T: ResourceTracker> ReplResolveFutures<T> {
    /// Extracts the REPL session, discarding the in-flight execution state.
    #[must_use]
    pub fn into_repl(self) -> MontyRepl<T> {
        self.repl
    }

    /// Returns unresolved call IDs for this suspended state.
    #[must_use]
    pub fn pending_call_ids(&self) -> &[u32] {
        &self.pending_call_ids
    }

    /// Resumes snippet execution with zero or more resolved futures.
    ///
    /// Supports incremental resolution: callers can provide only a subset of
    /// pending call IDs and continue resolving over multiple resumes.
    ///
    /// All errors — including API misuse (unknown `call_id`) and Python-level
    /// runtime failures — are returned as `Err(Box<ReplStartError>)` so the REPL
    /// session is always preserved.
    pub fn resume(
        self,
        results: Vec<(u32, ExtFunctionResult)>,
        print: PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let Self {
            mut repl,
            executor,
            vm_state,
            pending_call_ids,
        } = self;

        let invalid_call_id = results
            .iter()
            .find(|(call_id, _)| !pending_call_ids.contains(call_id))
            .map(|(call_id, _)| *call_id);

        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            &mut repl.heap,
            &executor.interns,
            print,
        );

        if let Some(call_id) = invalid_call_id {
            repl.globals = vm.take_globals();
            vm.cleanup();
            let error = MontyException::runtime_error(format!(
                "unknown call_id {call_id}, expected one of: {pending_call_ids:?}"
            ));
            return Err(Box::new(ReplStartError { repl, error }));
        }

        for (call_id, ext_result) in results {
            match ext_result {
                ExtFunctionResult::Return(obj) => {
                    if let Err(e) = vm.resolve_future(call_id, obj) {
                        repl.globals = vm.take_globals();
                        vm.cleanup();
                        let error =
                            MontyException::runtime_error(format!("Invalid return type for call {call_id}: {e}"));
                        return Err(Box::new(ReplStartError { repl, error }));
                    }
                }
                ExtFunctionResult::Error(exc) => vm.fail_future(call_id, RunError::from(exc)),
                ExtFunctionResult::Future(_) => {}
                ExtFunctionResult::NotFound(function_name) => {
                    vm.fail_future(call_id, ExtFunctionResult::not_found_exc(&function_name));
                }
            }
        }

        if let Some(error) = vm.take_failed_task_error() {
            repl.globals = vm.take_globals();
            vm.cleanup();
            let error = error.into_python_exception(&executor.interns, &executor.code);
            return Err(Box::new(ReplStartError { repl, error }));
        }

        let main_task_ready = vm.prepare_current_task_after_resolve();

        let loaded_task = match vm.load_ready_task_if_needed() {
            Ok(loaded) => loaded,
            Err(e) => {
                repl.globals = vm.take_globals();
                vm.cleanup();
                let error = e.into_python_exception(&executor.interns, &executor.code);
                return Err(Box::new(ReplStartError { repl, error }));
            }
        };

        if !main_task_ready && !loaded_task {
            let pending_call_ids = vm.get_pending_call_ids();
            if !pending_call_ids.is_empty() {
                let vm_state = vm.snapshot();
                let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
                return Ok(ReplProgress::ResolveFutures(Self {
                    repl,
                    executor,
                    vm_state,
                    pending_call_ids,
                }));
            }
        }

        let vm_result = vm.run();

        // Handle snapshot vs completion: reclaim globals on the completion path.
        if matches!(
            &vm_result,
            Ok(FrameExit::ExternalCall { .. }
                | FrameExit::OsCall { .. }
                | FrameExit::MethodCall { .. }
                | FrameExit::ResolveFutures(_)
                | FrameExit::NameLookup { .. })
        ) {
            let vm_state = vm.snapshot();
            handle_repl_vm_result(vm_result, Some(vm_state), executor, repl)
        } else {
            repl.globals = vm.take_globals();
            vm.cleanup();
            handle_repl_vm_result(vm_result, None, executor, repl)
        }
    }
}

// ---------------------------------------------------------------------------
// ReplContinuationMode — public utility for interactive input collection
// ---------------------------------------------------------------------------

/// Parse-derived continuation state for interactive REPL input collection.
///
/// `monty-cli` uses this to decide whether to execute the buffered snippet
/// immediately, keep collecting continuation lines, or require a terminating
/// blank line for block statements (`if:`, `def:`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplContinuationMode {
    /// The current snippet is syntactically complete and can run now.
    Complete,
    /// The snippet is incomplete and needs more continuation lines.
    IncompleteImplicit,
    /// The snippet opened an indented block and should wait for a trailing blank
    /// line before execution, matching CPython interactive behavior.
    IncompleteBlock,
}

/// Detects whether REPL source is complete or needs more input.
///
/// This mirrors CPython's broad interactive behavior:
/// - Incomplete bracketed / parenthesized / triple-quoted constructs continue.
/// - Clause headers (`if:`, `def:`, etc.) require an indented body and then a
///   terminating blank line before execution.
/// - All other parse outcomes are treated as complete (either valid code or a
///   syntax error that should be shown immediately).
#[must_use]
pub fn detect_repl_continuation_mode(source: &str) -> ReplContinuationMode {
    let Err(error) = parse_module(source) else {
        return ReplContinuationMode::Complete;
    };

    match error.error {
        ParseErrorType::OtherError(msg) => {
            if msg.starts_with("Expected an indented block after ") {
                ReplContinuationMode::IncompleteBlock
            } else {
                ReplContinuationMode::Complete
            }
        }
        ParseErrorType::Lexical(LexicalErrorType::Eof)
        | ParseErrorType::ExpectedToken {
            found: TokenKind::EndOfFile,
            ..
        }
        | ParseErrorType::FStringError(InterpolatedStringErrorType::UnterminatedTripleQuotedString)
        | ParseErrorType::TStringError(InterpolatedStringErrorType::UnterminatedTripleQuotedString) => {
            ReplContinuationMode::IncompleteImplicit
        }
        _ => ReplContinuationMode::Complete,
    }
}

// ---------------------------------------------------------------------------
// ReplExecutor — internal compilation helper
// ---------------------------------------------------------------------------

/// Compiled snippet representation used only by REPL execution.
///
/// This intentionally mirrors the data shape needed by VM execution in
/// `run.rs` but lives in the REPL module so REPL evolution does not require
/// changing `run.rs`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ReplExecutor {
    /// Number of slots needed in the global namespace.
    namespace_size: usize,
    /// Maps variable names to their indices in the namespace.
    ///
    /// Stable slot assignment is required across snippets so previously created
    /// objects continue to resolve names correctly.
    name_map: AHashMap<String, NamespaceId>,
    /// Compiled bytecode for the snippet.
    module_code: Code,
    /// Interned strings and compiled functions for this snippet.
    interns: Interns,
    /// Source code used for traceback/error rendering.
    code: String,
    /// Input variable names that were injected for this snippet.
    ///
    /// Stored so that `inject_inputs` can look up their namespace slots
    /// after compilation assigns them.
    input_names: Vec<String>,
}

impl ReplExecutor {
    /// Compiles one REPL snippet against existing session metadata.
    ///
    /// This differs from normal compilation in three ways required for true
    /// no-replay execution:
    /// - Seeds parsing from `existing_interns` so old `StringId` values stay stable.
    /// - Seeds compilation with existing functions so old `FunctionId` values remain valid.
    /// - Reuses `existing_name_map` and appends new global names only.
    ///
    /// `input_names` are pre-registered in the name map before preparation so they
    /// receive stable namespace slots that `inject_inputs` can use to store values.
    fn new_repl_snippet(
        code: String,
        script_name: &str,
        mut existing_name_map: AHashMap<String, NamespaceId>,
        existing_interns: &Interns,
        input_names: Vec<String>,
    ) -> Result<Self, MontyException> {
        // Pre-register input names so they get stable slots before preparation.
        for name in &input_names {
            let next_slot = existing_name_map.len();
            existing_name_map
                .entry(name.clone())
                .or_insert_with(|| NamespaceId::new(next_slot));
        }

        let seeded_interner = InternerBuilder::from_interns(existing_interns, &code);
        let parse_result = parse_with_interner(&code, script_name, seeded_interner)
            .map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare_with_existing_names(parse_result, existing_name_map)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        let existing_functions = existing_interns.functions_clone();
        let mut interns = Interns::new(prepared.interner, Vec::new());
        let namespace_size_u16 = u16::try_from(prepared.namespace_size).expect("module namespace size exceeds u16");
        let compile_result =
            Compiler::compile_module_with_functions(&prepared.nodes, &interns, namespace_size_u16, existing_functions)
                .map_err(|e| e.into_python_exc(script_name, &code))?;
        interns.set_functions(compile_result.functions);

        Ok(Self {
            namespace_size: prepared.namespace_size,
            name_map: prepared.name_map,
            module_code: compile_result.code,
            interns,
            code,
            input_names,
        })
    }
}

// ---------------------------------------------------------------------------
// ReplSnapshot — internal execution state for suspend/resume
// ---------------------------------------------------------------------------

/// REPL execution state that can be resumed after an external call.
///
/// This is the REPL-aware counterpart to `Snapshot`. It is `pub(crate)` —
/// callers interact with the per-variant structs (`ReplFunctionCall`, etc.).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub(crate) struct ReplSnapshot<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: ReplExecutor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
}

impl<T: ResourceTracker> ReplSnapshot<T> {
    /// Extracts the REPL session, restoring globals from the VM snapshot.
    ///
    /// When a snapshot is taken, `mem::take` moves globals into the VM.
    /// This method puts them back so the REPL can be used for further snippets.
    fn into_repl(self) -> MontyRepl<T> {
        let Self { mut repl, vm_state, .. } = self;
        repl.globals = vm_state.globals;
        repl
    }

    /// Continues snippet execution with an external result.
    fn run(
        self,
        result: impl Into<ExtFunctionResult>,
        print: PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let Self {
            mut repl,
            executor,
            vm_state,
        } = self;

        let ext_result = result.into();

        let mut vm = VM::restore(
            vm_state,
            &executor.module_code,
            &mut repl.heap,
            &executor.interns,
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

        // Handle snapshot vs completion: reclaim globals on the completion path.
        if matches!(
            &vm_result,
            Ok(FrameExit::ExternalCall { .. }
                | FrameExit::OsCall { .. }
                | FrameExit::MethodCall { .. }
                | FrameExit::ResolveFutures(_)
                | FrameExit::NameLookup { .. })
        ) {
            let vm_state = vm.snapshot();
            handle_repl_vm_result(vm_result, Some(vm_state), executor, repl)
        } else {
            repl.globals = vm.take_globals();
            vm.cleanup();
            handle_repl_vm_result(vm_result, None, executor, repl)
        }
    }
}

// ---------------------------------------------------------------------------
// Private helper functions
// ---------------------------------------------------------------------------

/// Converts module/frame exit results into plain `MontyObject` outputs.
///
/// REPL initialization executes like normal module execution, which must reject
/// suspendable outcomes when called through non-iterative APIs.
fn frame_exit_to_object(
    frame_exit_result: RunResult<FrameExit>,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<MontyObject> {
    match frame_exit_result? {
        FrameExit::Return(return_value) => Ok(MontyObject::new(return_value, heap, interns)),
        FrameExit::ExternalCall {
            function_name, args, ..
        } => {
            args.drop_with_heap(heap);
            let function_name = function_name.as_str(interns);
            Err(ExcType::not_implemented(format!(
                "External function '{function_name}' not implemented with standard execution"
            ))
            .into())
        }
        FrameExit::OsCall { function, args, .. } => {
            args.drop_with_heap(heap);
            Err(ExcType::not_implemented(format!(
                "OS function '{function}' not implemented with standard execution"
            ))
            .into())
        }
        FrameExit::MethodCall { method_name, args, .. } => {
            args.drop_with_heap(heap);
            let name = method_name.as_str(interns);
            Err(
                ExcType::not_implemented(format!("Method call '{name}' not implemented with standard execution"))
                    .into(),
            )
        }
        FrameExit::ResolveFutures(_) => {
            Err(ExcType::not_implemented("async futures not supported by standard execution.").into())
        }
        FrameExit::NameLookup { name_id, .. } => {
            let name = interns.get_str(name_id);
            Err(ExcType::name_error(name).into())
        }
    }
}

/// Handles a `FrameExit` result and converts it to REPL progress.
///
/// This mirrors `handle_vm_result` but preserves REPL heap/globals on
/// completion by returning `ReplProgress::Complete { repl, value }`.
/// On runtime errors, the REPL is preserved inside a `ReplStartError`.
fn handle_repl_vm_result<T: ResourceTracker>(
    result: RunResult<FrameExit>,
    vm_state: Option<VMSnapshot>,
    executor: ReplExecutor,
    mut repl: MontyRepl<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    macro_rules! new_repl_snapshot {
        () => {
            ReplSnapshot {
                repl,
                executor,
                vm_state: vm_state.expect("snapshot should exist"),
            }
        };
    }

    match result {
        Ok(FrameExit::Return(value)) => {
            let output = MontyObject::new(value, &mut repl.heap, &executor.interns);
            let ReplExecutor { name_map, interns, .. } = executor;
            repl.global_name_map = name_map;
            repl.interns = interns;
            Ok(ReplProgress::Complete { repl, value: output })
        }
        Ok(FrameExit::ExternalCall {
            function_name,
            args,
            call_id,
            ..
        }) => {
            let function_name = function_name.into_string(&executor.interns);
            let (args_py, kwargs_py) = args.into_py_objects(&mut repl.heap, &executor.interns);

            Ok(ReplProgress::FunctionCall(ReplFunctionCall {
                function_name,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                method_call: false,
                snapshot: new_repl_snapshot!(),
            }))
        }
        Ok(FrameExit::OsCall {
            function,
            args,
            call_id,
        }) => {
            let (args_py, kwargs_py) = args.into_py_objects(&mut repl.heap, &executor.interns);

            Ok(ReplProgress::OsCall(ReplOsCall {
                function,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                snapshot: new_repl_snapshot!(),
            }))
        }
        Ok(FrameExit::MethodCall {
            method_name,
            args,
            call_id,
        }) => {
            let function_name = method_name.into_string(&executor.interns);
            let (args_py, kwargs_py) = args.into_py_objects(&mut repl.heap, &executor.interns);

            Ok(ReplProgress::FunctionCall(ReplFunctionCall {
                function_name,
                args: args_py,
                kwargs: kwargs_py,
                call_id: call_id.raw(),
                method_call: true,
                snapshot: new_repl_snapshot!(),
            }))
        }
        Ok(FrameExit::ResolveFutures(pending_call_ids)) => {
            let pending_call_ids: Vec<u32> = pending_call_ids.iter().map(|id| id.raw()).collect();
            Ok(ReplProgress::ResolveFutures(ReplResolveFutures {
                repl,
                executor,
                vm_state: vm_state.expect("snapshot should exist for ResolveFutures"),
                pending_call_ids,
            }))
        }
        Ok(FrameExit::NameLookup {
            name_id,
            namespace_slot,
            is_global,
        }) => {
            let name = executor.interns.get_str(name_id).to_owned();
            Ok(ReplProgress::NameLookup(ReplNameLookup {
                name,
                namespace_slot,
                is_global,
                snapshot: new_repl_snapshot!(),
            }))
        }
        Err(err) => {
            let error = err.into_python_exception(&executor.interns, &executor.code);
            // Commit compiler metadata even on runtime errors, matching feed() behavior.
            // Snippets can create new variables or functions before raising, and those
            // values may reference FunctionId/StringId values from the new tables.
            let ReplExecutor { name_map, interns, .. } = executor;
            repl.global_name_map = name_map;
            repl.interns = interns;
            Err(Box::new(ReplStartError { repl, error }))
        }
    }
}
