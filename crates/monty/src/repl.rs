//! Stateful REPL execution support for Monty.
//!
//! This module implements incremental snippet execution where each new snippet
//! is compiled and executed against persistent heap/namespace state without
//! replaying previously executed snippets.

use std::mem;

use ahash::AHashMap;
use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{InterpolatedStringErrorType, LexicalErrorType, ParseErrorType, parse_module};
use serde::de::DeserializeOwned;

use crate::{
    ExcType, MontyException,
    asyncio::CallId,
    bytecode::{VM, VMSnapshot},
    exception_private::RunError,
    heap::{DropWithHeap, Heap, HeapReader},
    intern::{InternerBuilder, Interns},
    io::PrintWriter,
    namespace::NamespaceId,
    object::MontyObject,
    os::OsFunction,
    resource::ResourceTracker,
    run::Executor,
    run_progress::{ConvertedExit, ExtFunctionResult, NameLookupResult, convert_frame_exit},
    value::Value,
};

/// Stateful REPL session that executes snippets incrementally without replay.
///
/// `MontyRepl` preserves heap and global variable state between snippets.
/// Each `feed()` compiles and executes only the new snippet against the current
/// state, avoiding the cost and semantic risks of replaying prior code.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: DeserializeOwned"))]
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
    /// Source text of every snippet that has been fed, keyed by its
    /// generated script name (`<python-input-N>`).
    ///
    /// Required because a traceback raised in snippet N can include frames
    /// from functions defined in snippet M < N. Those frames carry
    /// `CodeRange` byte offsets that index into snippet M's source, so the
    /// diagnostic pass must be able to look that source up by filename —
    /// the current snippet's `Executor.code` is not sufficient.
    #[serde(default)]
    sources: AHashMap<String, String>,
    /// Persistent heap across snippets.
    heap: Heap<T>,
    /// Persistent global variable values across snippets.
    ///
    /// Indexed by `NamespaceId` slots from `global_name_map`. Between snippet
    /// executions these are the only VM values that persist — stack and frames
    /// are transient.
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
            sources: AHashMap::new(),
            heap,
            globals: Vec::new(),
        }
    }

    /// Returns the resource tracker that will be used for the next snippet.
    ///
    /// This is primarily intended for host integrations that need to attach
    /// per-execution state, such as cancellation markers, to an existing REPL.
    pub fn tracker(&self) -> &T {
        self.heap.tracker()
    }

    /// Returns mutable access to the resource tracker for the next snippet.
    ///
    /// REPL hosts use this to install ephemeral execution controls, such as
    /// async cancellation flags, before calling `feed_start()`.
    pub fn tracker_mut(&mut self) -> &mut T {
        self.heap.tracker_mut()
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
        mut print: PrintWriter<'_>,
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
        // Preserve this snippet's source (see `feed_run` for rationale).
        this.sources.insert(input_script_name.clone(), code.to_owned());
        let executor = match Executor::new_repl_snippet(
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

        match HeapReader::with(&mut this.heap, |heap| {
            let mut vm = VM::new(mem::take(&mut this.globals), heap, &executor.interns, print.reborrow());

            // Inject inputs with VM alive
            if let Err(error) = inject_inputs_into_vm(&executor, input_values, &mut vm) {
                this.globals = vm.take_globals();
                return Err(error);
            }

            let vm_result = vm.run_module(&executor.module_code);

            // Convert while VM alive, then snapshot or reclaim globals
            let converted = convert_frame_exit(vm_result, &mut vm);
            let vm_state = if converted.needs_snapshot() {
                Some(vm.snapshot())
            } else {
                this.globals = vm.take_globals();
                None
            };
            Ok((converted, vm_state))
        }) {
            Ok((converted, vm_state)) => build_repl_progress(converted, vm_state, executor, this),
            Err(error) => Err(Box::new(ReplStartError { repl: this, error })),
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
        mut print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        if code.is_empty() {
            return Ok(MontyObject::None);
        }

        let (input_names, input_values): (Vec<_>, Vec<_>) = inputs.into_iter().unzip();

        let input_script_name = self.next_input_script_name();
        // Preserve this snippet's source before anything can fail, so later
        // tracebacks with frames from this snippet can still resolve line/
        // column/preview information — `Executor.code` only survives until
        // the next feed.
        self.sources.insert(input_script_name.clone(), code.to_owned());
        let executor = Executor::new_repl_snippet(
            code.to_owned(),
            &input_script_name,
            self.global_name_map.clone(),
            &self.interns,
            input_names,
        )?;

        self.ensure_globals_size(executor.namespace_size);

        let result = HeapReader::with(&mut self.heap, |heap| {
            let mut vm = VM::new(mem::take(&mut self.globals), heap, &executor.interns, print.reborrow());

            if let Err(e) = inject_inputs_into_vm(&executor, input_values, &mut vm) {
                self.globals = vm.take_globals();
                return Err(e);
            }

            let result = executor.run_to_completion(&mut vm);

            // Reclaim globals before cleanup.
            self.globals = vm.take_globals();
            Ok(result)
        })?;

        // Commit compiler metadata even on runtime errors.
        // Snippets can mutate globals before raising, and those values may contain
        // FunctionId/StringId values that must be interpreted with the updated tables.
        let Executor { name_map, interns, .. } = executor;
        self.global_name_map = name_map;
        self.interns = interns;

        // Resolve every traceback frame against the source of the snippet that
        // produced it — frames from earlier snippets live in `self.sources`.
        result.map_err(|e| e.into_python_exception(&self.interns, |fname| self.sources.get(fname).map(String::as_str)))
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

impl<T: ResourceTracker + DeserializeOwned> MontyRepl<T> {
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
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: DeserializeOwned"))]
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

impl<T: ResourceTracker + DeserializeOwned> ReplProgress<T> {
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
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: DeserializeOwned"))]
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
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: DeserializeOwned"))]
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
        self.snapshot.run(result.into(), print)
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
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: DeserializeOwned"))]
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
        mut print: PrintWriter<'_>,
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
            vm_state,
        } = snapshot;

        match HeapReader::with(&mut repl.heap, |heap| {
            // Restore the VM first, then convert inside its lifetime
            let mut vm = VM::restore(
                vm_state,
                &executor.module_code,
                heap,
                &executor.interns,
                print.reborrow(),
            );

            // Resolve the name lookup result with the VM alive
            let vm_result = match result {
                NameLookupResult::Value(obj) => {
                    let value = match obj.to_value(&mut vm) {
                        Ok(v) => v,
                        Err(e) => {
                            repl.globals = vm.take_globals();
                            return Err(MontyException::runtime_error(format!(
                                "invalid name lookup result: {e}"
                            )));
                        }
                    };

                    // Cache the resolved value in the appropriate slot
                    let slot = namespace_slot as usize;
                    if is_global {
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
                    let err: RunError = ExcType::name_error(&name).into();
                    vm.resume_with_exception(err)
                }
            };

            // Convert while VM alive, then snapshot or reclaim globals
            let converted = convert_frame_exit(vm_result, &mut vm);
            let vm_state = if converted.needs_snapshot() {
                Some(vm.snapshot())
            } else {
                repl.globals = vm.take_globals();
                None
            };
            Ok((converted, vm_state))
        }) {
            Ok((converted, vm_state)) => build_repl_progress(converted, vm_state, executor, repl),
            Err(error) => Err(Box::new(ReplStartError { repl, error })),
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
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: DeserializeOwned"))]
pub struct ReplResolveFutures<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: Executor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
    /// Pending call IDs expected by this snapshot.
    pending_call_ids: Vec<u32>,
}

impl<T: ResourceTracker> ReplResolveFutures<T> {
    /// Extracts the REPL session, restoring globals from the suspended VM state.
    ///
    /// As with the other REPL snapshot types, globals live inside the VM
    /// snapshot while execution is suspended. Recovering the REPL for a
    /// cancelled or abandoned async snippet must put those globals back so
    /// previously defined REPL bindings remain available.
    #[must_use]
    pub fn into_repl(self) -> MontyRepl<T> {
        let Self { mut repl, vm_state, .. } = self;
        repl.globals = vm_state.globals;
        repl
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
        mut print: PrintWriter<'_>,
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

        match HeapReader::with(&mut repl.heap, |heap| {
            let mut vm = VM::restore(
                vm_state,
                &executor.module_code,
                heap,
                &executor.interns,
                print.reborrow(),
            );

            if let Some(call_id) = invalid_call_id {
                repl.globals = vm.take_globals();
                return Err(MontyException::runtime_error(format!(
                    "unknown call_id {call_id}, expected one of: {pending_call_ids:?}"
                )));
            }

            let vm_result = vm.resume_with_resolved_futures(results);

            // Convert while VM alive, then snapshot or reclaim globals
            let converted = convert_frame_exit(vm_result, &mut vm);
            let vm_state = if converted.needs_snapshot() {
                Some(vm.snapshot())
            } else {
                repl.globals = vm.take_globals();
                None
            };
            Ok((converted, vm_state))
        }) {
            Ok((converted, vm_state)) => build_repl_progress(converted, vm_state, executor, repl),
            Err(error) => Err(Box::new(ReplStartError { repl, error })),
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
// ReplSnapshot — internal execution state for suspend/resume
// ---------------------------------------------------------------------------

/// REPL execution state that can be resumed after an external call.
///
/// This is the REPL-aware counterpart to `Snapshot`. It is `pub(crate)` —
/// callers interact with the per-variant structs (`ReplFunctionCall`, etc.).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: DeserializeOwned"))]
pub(crate) struct ReplSnapshot<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: Executor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
}

impl<T: ResourceTracker> ReplSnapshot<T> {
    /// Extracts the REPL session, restoring globals from the VM snapshot.
    ///
    /// When a snapshot is taken, globals live inside the `VMSnapshot`.
    /// This method creates an empty snapshot from just the globals so the REPL
    /// can be used for further snippets.
    fn into_repl(self) -> MontyRepl<T> {
        let Self { mut repl, vm_state, .. } = self;
        repl.globals = vm_state.globals;
        repl
    }

    /// Continues snippet execution with an external result.
    fn run(
        self,
        result: impl Into<ExtFunctionResult>,
        mut print: PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let Self {
            mut repl,
            executor,
            vm_state,
        } = self;

        let ext_result = result.into();

        let (converted, vm_state) = HeapReader::with(&mut repl.heap, |heap| {
            let mut vm = VM::restore(
                vm_state,
                &executor.module_code,
                heap,
                &executor.interns,
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

            // Convert while VM alive, then snapshot or reclaim globals
            let converted = convert_frame_exit(vm_result, &mut vm);
            let vm_state = if converted.needs_snapshot() {
                Some(vm.snapshot())
            } else {
                repl.globals = vm.take_globals();
                None
            };
            (converted, vm_state)
        });
        build_repl_progress(converted, vm_state, executor, repl)
    }
}

// ---------------------------------------------------------------------------
// Private helper functions
// ---------------------------------------------------------------------------

/// Injects input values into the VM's global namespace slots.
///
/// Converts each `MontyObject` to a `Value` while the VM is alive, then stores
/// it in the global slot that the compiler assigned for the corresponding input name.
fn inject_inputs_into_vm(
    executor: &Executor,
    input_values: Vec<MontyObject>,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> Result<(), MontyException> {
    for (name, obj) in executor.input_names.iter().zip(input_values) {
        let slot = executor
            .name_map
            .get(name)
            .expect("input name should have a namespace slot")
            .index();
        let value = obj
            .to_value(vm)
            .map_err(|e| MontyException::runtime_error(format!("invalid input type: {e}")))?;
        let old = mem::replace(&mut vm.globals[slot], value);
        old.drop_with_heap(vm);
    }
    Ok(())
}

/// Assembles a `ReplProgress` from already-converted data.
///
/// This is the REPL equivalent of `build_run_progress`. On completion/error,
/// compiler metadata is committed to the REPL so subsequent snippets see
/// updated intern tables and name maps.
fn build_repl_progress<T: ResourceTracker>(
    converted: ConvertedExit,
    vm_state: Option<VMSnapshot>,
    executor: Executor,
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

    match converted {
        ConvertedExit::Complete(obj) => {
            let Executor { name_map, interns, .. } = executor;
            repl.global_name_map = name_map;
            repl.interns = interns;
            Ok(ReplProgress::Complete { repl, value: obj })
        }
        ConvertedExit::FunctionCall {
            function_name,
            args,
            kwargs,
            call_id,
            method_call,
        } => Ok(ReplProgress::FunctionCall(ReplFunctionCall {
            function_name,
            args,
            kwargs,
            call_id,
            method_call,
            snapshot: new_repl_snapshot!(),
        })),
        ConvertedExit::OsCall {
            function,
            args,
            kwargs,
            call_id,
        } => Ok(ReplProgress::OsCall(ReplOsCall {
            function,
            args,
            kwargs,
            call_id,
            snapshot: new_repl_snapshot!(),
        })),
        ConvertedExit::ResolveFutures(pending_call_ids) => Ok(ReplProgress::ResolveFutures(ReplResolveFutures {
            repl,
            executor,
            vm_state: vm_state.expect("snapshot should exist for ResolveFutures"),
            pending_call_ids,
        })),
        ConvertedExit::NameLookup {
            name,
            namespace_slot,
            is_global,
        } => Ok(ReplProgress::NameLookup(ReplNameLookup {
            name,
            namespace_slot,
            is_global,
            snapshot: new_repl_snapshot!(),
        })),
        ConvertedExit::Error(err) => {
            // Resolve traceback frames against every snippet the REPL has
            // seen, not just the currently-executing one. `executor.interns`
            // is still required because it holds the StringIds referenced by
            // the in-flight frames; `repl.sources` holds every snippet's
            // source text and is what owns any older snippets' sources.
            let error =
                err.into_python_exception(&executor.interns, |fname| repl.sources.get(fname).map(String::as_str));
            // Commit compiler metadata even on runtime errors, matching feed() behavior.
            // Snippets can create new variables or functions before raising, and those
            // values may reference FunctionId/StringId values from the new tables.
            let Executor { name_map, interns, .. } = executor;
            repl.global_name_map = name_map;
            repl.interns = interns;
            Err(Box::new(ReplStartError { repl, error }))
        }
    }
}
