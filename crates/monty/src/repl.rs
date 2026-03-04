//! Stateful REPL execution support for Monty.
//!
//! This module implements incremental snippet execution where each new snippet
//! is compiled and executed against persistent heap/namespace state without
//! replaying previously executed snippets.

use ahash::AHashMap;
use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{InterpolatedStringErrorType, LexicalErrorType, ParseErrorType, parse_module};

use crate::{
    ExcType, MontyException,
    asyncio::CallId,
    bytecode::{Code, Compiler, FrameExit, VM, VMContext, VMSnapshot},
    exception_private::{RunError, RunResult},
    heap::{DropWithHeap, Heap},
    intern::{ExtFunctionId, InternerBuilder, Interns},
    io::PrintWriter,
    namespace::{GLOBAL_NS_IDX, NamespaceId, Namespaces},
    object::MontyObject,
    observer::{ExternalCallKind, ExternalCallReturnKind, RuntimeObserverHandle},
    os::OsFunction,
    parse::{parse, parse_with_interner},
    prepare::{prepare, prepare_with_existing_names},
    progress_runtime_ids::{RuntimeIdCardinality, RuntimeIdSlices, checked_runtime_id_payload},
    resource::ResourceTracker,
    run::{ExternalResult, MontyFuture, emit_external_call_requested, emit_external_call_returned},
    runtime_id::RuntimeValueId,
    value::Value,
};

/// Compiled snippet/module representation used only by REPL execution.
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
    /// Compiled bytecode for the snippet/module.
    module_code: Code,
    /// Interned strings and compiled functions for this snippet/module.
    interns: Interns,
    /// IDs to create values in the namespace representing external functions.
    external_function_ids: Vec<ExtFunctionId>,
    /// Source code used for traceback/error rendering.
    code: String,
}

impl ReplExecutor {
    /// Compiles the initial REPL module.
    ///
    /// This is equivalent to normal module compilation but scoped to REPL
    /// infrastructure so `run.rs` can remain unchanged.
    fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        external_functions: Vec<String>,
    ) -> Result<Self, MontyException> {
        let parse_result = parse(&code, script_name).map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare(parse_result, input_names, &external_functions)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        let external_function_ids = (0..external_functions.len()).map(ExtFunctionId::new).collect();

        let mut interns = Interns::new(prepared.interner, Vec::new(), external_functions);
        let namespace_size_u16 = u16::try_from(prepared.namespace_size).expect("module namespace size exceeds u16");
        let compile_result = Compiler::compile_module(&prepared.nodes, &interns, namespace_size_u16)
            .map_err(|e| e.into_python_exc(script_name, &code))?;
        interns.set_functions(compile_result.functions);

        Ok(Self {
            namespace_size: prepared.namespace_size,
            name_map: prepared.name_map,
            module_code: compile_result.code,
            interns,
            external_function_ids,
            code,
        })
    }

    /// Compiles one incremental REPL snippet against existing session metadata.
    ///
    /// This differs from normal compilation in three ways required for true
    /// no-replay execution:
    /// - Seeds parsing from `existing_interns` so old `StringId` values stay stable.
    /// - Seeds compilation with existing functions so old `FunctionId` values remain valid.
    /// - Reuses `existing_name_map` and appends new global names only.
    fn new_repl_snippet(
        code: String,
        script_name: &str,
        external_functions: Vec<String>,
        existing_name_map: AHashMap<String, NamespaceId>,
        existing_interns: &Interns,
    ) -> Result<Self, MontyException> {
        let seeded_interner = InternerBuilder::from_interns(existing_interns, &code);
        let parse_result = parse_with_interner(&code, script_name, seeded_interner)
            .map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare_with_existing_names(parse_result, existing_name_map)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        let external_function_ids = (0..external_functions.len()).map(ExtFunctionId::new).collect();

        let existing_functions = existing_interns.functions_clone();
        let mut interns = Interns::new(prepared.interner, Vec::new(), external_functions);
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
            external_function_ids,
            code,
        })
    }

    /// Builds the runtime namespace stack for module execution.
    ///
    /// External function bindings are inserted first, then input values, then
    /// remaining slots are initialized to `Undefined`.
    fn prepare_namespaces(
        &self,
        inputs: Vec<MontyObject>,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Namespaces, MontyException> {
        let Some(extra) = self
            .namespace_size
            .checked_sub(self.external_function_ids.len() + inputs.len())
        else {
            return Err(MontyException::runtime_error("too many inputs for namespace"));
        };

        let mut namespace = Vec::with_capacity(self.namespace_size);
        for f_id in &self.external_function_ids {
            namespace.push(Value::ExtFunction(*f_id));
        }
        for input in inputs {
            namespace.push(
                input
                    .to_value(heap, &self.interns)
                    .map_err(|e| MontyException::runtime_error(format!("invalid input type: {e}")))?,
            );
        }
        if extra > 0 {
            namespace.extend((0..extra).map(|_| Value::Undefined));
        }
        Ok(Namespaces::new(namespace))
    }
}

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
            ext_function_id, args, ..
        } => {
            args.drop_with_heap(heap);
            let function_name = interns.get_external_function_name(ext_function_id);
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
    }
}

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

/// Stateful REPL session that executes snippets incrementally without replay.
///
/// `MontyRepl` preserves heap and global namespace state between snippets.
/// Each `feed()` compiles and executes only the new snippet against the current
/// state, avoiding the cost and semantic risks of replaying prior code.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct MontyRepl<T: ResourceTracker> {
    /// Script name used only for initial module parse and runtime error messages.
    ///
    /// Incremental `feed()` snippets intentionally use internal script names
    /// like `<python-input-0>` to match CPython's interactive traceback style.
    script_name: String,
    /// Counter for generated `<python-input-N>` snippet filenames.
    #[serde(default)]
    next_input_id: u64,
    /// External function names declared for this session.
    external_function_names: Vec<String>,
    /// Stable mapping of global variable names to namespace slot IDs.
    global_name_map: AHashMap<String, NamespaceId>,
    /// Persistent intern table across snippets so intern/function IDs remain valid.
    interns: Interns,
    /// Persistent heap across snippets.
    heap: Heap<T>,
    /// Persistent namespace stack across snippets.
    namespaces: Namespaces,
}

impl<T: ResourceTracker> MontyRepl<T> {
    /// Creates a new stateful REPL by compiling and executing initial code once.
    ///
    /// This provides the same initialization behavior as a normal run, then keeps
    /// the resulting heap/global namespace for incremental snippet execution.
    ///
    /// # Returns
    /// A tuple of:
    /// - `MontyRepl<T>`: initialized REPL session
    /// - `MontyObject`: result of the initial execution
    ///
    /// # Errors
    /// Returns `MontyException` for parse/compile/runtime failures.
    pub fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        external_function_names: Vec<String>,
        inputs: Vec<MontyObject>,
        resource_tracker: T,
        print: &mut PrintWriter<'_>,
    ) -> Result<(Self, MontyObject), MontyException> {
        let executor = ReplExecutor::new(code, script_name, input_names, external_function_names.clone())?;

        let mut heap = Heap::new(executor.namespace_size, resource_tracker);
        let mut namespaces = executor.prepare_namespaces(inputs, &mut heap)?;

        let mut vm = VM::new(VMContext::new(&mut heap, &mut namespaces, &executor.interns, print));
        let frame_exit_result = vm.run_module(&executor.module_code);
        vm.cleanup();

        let output = frame_exit_to_object(frame_exit_result, &mut heap, &executor.interns)
            .map_err(|e| e.into_python_exception(&executor.interns, &executor.code))?;

        let repl = Self {
            script_name: script_name.to_owned(),
            next_input_id: 0,
            external_function_names,
            global_name_map: executor.name_map,
            interns: executor.interns,
            heap,
            namespaces,
        };

        Ok((repl, output))
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
    pub fn start(self, code: &str, print: &mut PrintWriter<'_>) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        self.start_with_observer(code, print, RuntimeObserverHandle::disabled())
    }

    /// Starts executing a new snippet with a runtime observer.
    pub fn start_with_observer(
        self,
        code: &str,
        print: &mut PrintWriter<'_>,
        observer: RuntimeObserverHandle,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let mut this = self;
        if code.is_empty() {
            return Ok(ReplProgress::Complete {
                repl: this,
                value: MontyObject::None,
            });
        }

        let input_script_name = this.next_input_script_name();
        let executor = match ReplExecutor::new_repl_snippet(
            code.to_owned(),
            &input_script_name,
            this.external_function_names.clone(),
            this.global_name_map.clone(),
            &this.interns,
        ) {
            Ok(exec) => exec,
            Err(error) => return Err(Box::new(ReplStartError { repl: this, error })),
        };

        this.ensure_global_namespace_size(executor.namespace_size);

        let (vm_result, vm_state) = {
            let mut vm = VM::new_with_observer(
                VMContext::new(&mut this.heap, &mut this.namespaces, &executor.interns, print),
                observer.clone(),
            );
            let vm_result = vm.run_module(&executor.module_code);
            let vm_state = vm.check_snapshot(&vm_result);
            (vm_result, vm_state)
        };

        handle_repl_vm_result(vm_result, vm_state, executor, this, observer)
    }

    /// Starts snippet execution with `PrintWriter::Stdout` and no additional host output wiring.
    pub fn start_no_print(self, code: &str) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        self.start(code, &mut PrintWriter::Stdout)
    }

    /// Starts snippet execution with `PrintWriter::Stdout` and a runtime observer.
    pub fn start_no_print_with_observer(
        self,
        code: &str,
        observer: RuntimeObserverHandle,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        self.start_with_observer(code, &mut PrintWriter::Stdout, observer)
    }

    /// Feeds and executes a new snippet against the current REPL state.
    ///
    /// This compiles only `code` using the existing global slot map, extends the
    /// global namespace if new names are introduced, and executes the snippet once.
    /// Previously executed snippets are never replayed. If execution raises after
    /// partially mutating globals, those mutations remain visible in later feeds,
    /// matching Python REPL semantics.
    ///
    /// # Errors
    /// Returns `MontyException` for syntax/compile/runtime failures.
    pub fn feed(&mut self, code: &str, print: &mut PrintWriter<'_>) -> Result<MontyObject, MontyException> {
        if code.is_empty() {
            return Ok(MontyObject::None);
        }

        let input_script_name = self.next_input_script_name();
        let executor = ReplExecutor::new_repl_snippet(
            code.to_owned(),
            &input_script_name,
            self.external_function_names.clone(),
            self.global_name_map.clone(),
            &self.interns,
        )?;

        let ReplExecutor {
            namespace_size,
            name_map,
            module_code,
            interns,
            code,
            ..
        } = executor;

        self.ensure_global_namespace_size(namespace_size);

        let mut vm = VM::new(VMContext::new(&mut self.heap, &mut self.namespaces, &interns, print));
        let frame_exit_result = vm.run_module(&module_code);
        vm.cleanup();

        // Commit compiler metadata even on runtime errors.
        // Snippets can mutate globals before raising, and those values may contain
        // FunctionId/StringId values that must be interpreted with the updated tables.
        self.global_name_map = name_map;
        self.interns = interns;

        frame_exit_to_object(frame_exit_result, &mut self.heap, &self.interns)
            .map_err(|e| e.into_python_exception(&self.interns, &code))
    }

    /// Executes a snippet with no additional host output wiring.
    pub fn feed_no_print(&mut self, code: &str) -> Result<MontyObject, MontyException> {
        self.feed(code, &mut PrintWriter::Stdout)
    }

    /// Grows the global namespace to at least `namespace_size`.
    ///
    /// Newly introduced slots are initialized to `Undefined` to keep slot alignment
    /// with the compiler's global-name map.
    fn ensure_global_namespace_size(&mut self, namespace_size: usize) {
        let global = self.namespaces.get_mut(GLOBAL_NS_IDX).mut_vec();
        if global.len() < namespace_size {
            global.resize_with(namespace_size, || Value::Undefined);
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
    /// This includes heap + namespaces + global slot mapping, allowing snapshot/restore
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
        #[cfg(feature = "ref-count-panic")]
        self.namespaces.drop_global_with_heap(&mut self.heap);
    }
}

/// Result of a single suspendable REPL snippet execution.
///
/// This mirrors `RunProgress` but returns the updated `MontyRepl` on completion
/// so callers can continue feeding additional snippets without replaying prior code.
#[derive(Debug, serde::Serialize)]
#[serde(bound(serialize = "T: serde::Serialize"))]
pub enum ReplProgress<T: ResourceTracker> {
    /// Execution paused at an external function call or dataclass method call.
    FunctionCall {
        /// The name of the function or method being called.
        function_name: String,
        /// The positional arguments passed to the function.
        args: Vec<MontyObject>,
        /// Stable runtime IDs for `args`, preserving positional order.
        #[serde(default)]
        arg_runtime_ids: Vec<RuntimeValueId>,
        /// The keyword arguments passed to the function (key, value pairs).
        kwargs: Vec<(MontyObject, MontyObject)>,
        /// Stable runtime IDs for keyword `(key, value)` pairs, preserving order.
        #[serde(default)]
        kwarg_runtime_ids: Vec<(RuntimeValueId, RuntimeValueId)>,
        /// Unique identifier for this call (used for async correlation).
        call_id: u32,
        /// Whether this is a dataclass method call (first arg is `self`).
        method_call: bool,
        /// Repl execution state that can be resumed.
        state: ReplSnapshot<T>,
    },
    /// Execution paused for an OS-level operation.
    OsCall {
        /// The OS function to execute.
        function: OsFunction,
        /// The positional arguments for the OS function.
        args: Vec<MontyObject>,
        /// Stable runtime IDs for `args`, preserving positional order.
        #[serde(default)]
        arg_runtime_ids: Vec<RuntimeValueId>,
        /// The keyword arguments passed to the function (key, value pairs).
        kwargs: Vec<(MontyObject, MontyObject)>,
        /// Stable runtime IDs for keyword `(key, value)` pairs, preserving order.
        #[serde(default)]
        kwarg_runtime_ids: Vec<(RuntimeValueId, RuntimeValueId)>,
        /// Unique identifier for this call (used for async correlation).
        call_id: u32,
        /// Repl execution state that can be resumed.
        state: ReplSnapshot<T>,
    },
    /// All async tasks are blocked waiting for external futures to resolve.
    ResolveFutures(ReplFutureSnapshot<T>),
    /// Snippet execution completed with the updated REPL and result value.
    Complete {
        /// Updated REPL session state to continue feeding snippets.
        repl: MontyRepl<T>,
        /// Final result produced by the snippet.
        value: MontyObject,
    },
}

#[derive(serde::Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
enum ReplProgressUnchecked<T: ResourceTracker> {
    FunctionCall {
        function_name: String,
        args: Vec<MontyObject>,
        #[serde(default)]
        arg_runtime_ids: Vec<RuntimeValueId>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        #[serde(default)]
        kwarg_runtime_ids: Vec<(RuntimeValueId, RuntimeValueId)>,
        call_id: u32,
        method_call: bool,
        state: ReplSnapshot<T>,
    },
    OsCall {
        function: OsFunction,
        args: Vec<MontyObject>,
        #[serde(default)]
        arg_runtime_ids: Vec<RuntimeValueId>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        #[serde(default)]
        kwarg_runtime_ids: Vec<(RuntimeValueId, RuntimeValueId)>,
        call_id: u32,
        state: ReplSnapshot<T>,
    },
    ResolveFutures(ReplFutureSnapshot<T>),
    Complete {
        repl: MontyRepl<T>,
        value: MontyObject,
    },
}

impl<T: ResourceTracker> ReplProgressUnchecked<T> {
    fn into_checked(self) -> Result<ReplProgress<T>, String> {
        match self {
            Self::FunctionCall {
                function_name,
                args,
                arg_runtime_ids,
                kwargs,
                kwarg_runtime_ids,
                call_id,
                method_call,
                state,
            } => {
                let cardinality =
                    RuntimeIdCardinality::new(args.len(), arg_runtime_ids.len(), kwargs.len(), kwarg_runtime_ids.len());
                crate::progress_runtime_ids::validate_runtime_id_cardinality(
                    "ReplProgress::FunctionCall",
                    &cardinality,
                )?;
                let checked_payload = checked_runtime_id_payload(args, arg_runtime_ids, kwargs, kwarg_runtime_ids);

                Ok(ReplProgress::FunctionCall {
                    function_name,
                    args: checked_payload.args,
                    arg_runtime_ids: checked_payload.arg_runtime_ids,
                    kwargs: checked_payload.kwargs,
                    kwarg_runtime_ids: checked_payload.kwarg_runtime_ids,
                    call_id,
                    method_call,
                    state,
                })
            }
            Self::OsCall {
                function,
                args,
                arg_runtime_ids,
                kwargs,
                kwarg_runtime_ids,
                call_id,
                state,
            } => {
                let cardinality =
                    RuntimeIdCardinality::new(args.len(), arg_runtime_ids.len(), kwargs.len(), kwarg_runtime_ids.len());
                crate::progress_runtime_ids::validate_runtime_id_cardinality("ReplProgress::OsCall", &cardinality)?;
                let checked_payload = checked_runtime_id_payload(args, arg_runtime_ids, kwargs, kwarg_runtime_ids);

                Ok(ReplProgress::OsCall {
                    function,
                    args: checked_payload.args,
                    arg_runtime_ids: checked_payload.arg_runtime_ids,
                    kwargs: checked_payload.kwargs,
                    kwarg_runtime_ids: checked_payload.kwarg_runtime_ids,
                    call_id,
                    state,
                })
            }
            Self::ResolveFutures(state) => Ok(ReplProgress::ResolveFutures(state)),
            Self::Complete { repl, value } => Ok(ReplProgress::Complete { repl, value }),
        }
    }
}

impl<'de, T> serde::Deserialize<'de> for ReplProgress<T>
where
    T: ResourceTracker + serde::de::DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <ReplProgressUnchecked<T> as serde::Deserialize>::deserialize(deserializer)?
            .into_checked()
            .map_err(serde::de::Error::custom)
    }
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
    /// Consumes the progress and returns external function call info and state.
    ///
    /// Returns:
    /// (
    ///   function_name,
    ///   positional_args,
    ///   keyword_args,
    ///   positional_arg_runtime_ids,
    ///   keyword_arg_runtime_ids,
    ///   call_id,
    ///   method_call,
    ///   state,
    /// ).
    #[must_use]
    #[expect(clippy::type_complexity)]
    pub fn into_function_call(
        self,
    ) -> Option<(
        String,
        Vec<MontyObject>,
        Vec<(MontyObject, MontyObject)>,
        Vec<RuntimeValueId>,
        Vec<(RuntimeValueId, RuntimeValueId)>,
        u32,
        bool,
        ReplSnapshot<T>,
    )> {
        match self {
            Self::FunctionCall {
                function_name,
                args,
                arg_runtime_ids,
                kwargs,
                kwarg_runtime_ids,
                call_id,
                method_call,
                state,
            } => Some((
                function_name,
                args,
                kwargs,
                arg_runtime_ids,
                kwarg_runtime_ids,
                call_id,
                method_call,
                state,
            )),
            _ => None,
        }
    }

    /// Consumes the progress and returns pending futures state.
    #[must_use]
    pub fn into_resolve_futures(self) -> Option<ReplFutureSnapshot<T>> {
        match self {
            Self::ResolveFutures(state) => Some(state),
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

    /// Returns runtime IDs for function-call or OS-call arguments.
    ///
    /// The first slice maps to positional args, and the second maps to keyword
    /// `(key, value)` pairs in the same order as the exposed host payload.
    #[must_use]
    pub fn runtime_ids(&self) -> Option<RuntimeIdSlices<'_>> {
        crate::progress_runtime_ids::progress_runtime_ids!(self)
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

/// REPL execution state that can be resumed after an external call.
///
/// This is the REPL-aware counterpart to `Snapshot`. Resuming continues the
/// same snippet and ultimately returns `ReplProgress::Complete` with the
/// updated REPL session.
///
/// Snapshots can also carry optional embedder-owned extension bytes; Monty
/// persists them but never interprets them.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplSnapshot<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: ReplExecutor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
    /// call_id used when resuming with an unresolved future.
    pending_call_id: u32,
    /// Optional embedder-owned bytes persisted with this snapshot.
    #[serde(default, rename = "snapshot_extension")]
    extension_bytes: Option<Vec<u8>>,
    /// Optional runtime observer handle for resumed execution.
    #[serde(skip, default = "RuntimeObserverHandle::disabled")]
    observer: RuntimeObserverHandle,
}

impl<T: ResourceTracker> ReplSnapshot<T> {
    /// Attaches embedder-owned snapshot extension bytes to this state.
    ///
    /// These bytes are serialized alongside the snapshot without interpretation
    /// by Monty. The host controls their content and versioning.
    #[must_use]
    pub fn with_snapshot_extension(mut self, snapshot_extension: Vec<u8>) -> Self {
        self.extension_bytes = Some(snapshot_extension);
        self
    }

    /// Returns the embedder-owned snapshot extension bytes, if present.
    #[must_use]
    pub fn snapshot_extension(&self) -> Option<&[u8]> {
        self.extension_bytes.as_deref()
    }

    /// Installs a runtime observer for subsequent resume calls.
    #[must_use]
    pub fn with_observer(mut self, observer: RuntimeObserverHandle) -> Self {
        self.observer = observer;
        self
    }

    /// Continues snippet execution with an external result.
    ///
    /// # Arguments
    /// * `result` - Return value, raised exception, or pending future marker
    /// * `print` - Writer used for Python `print()`
    pub fn run(
        self,
        result: impl Into<ExternalResult>,
        print: &mut PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let observer = self.observer.clone();
        self.run_with_observer(result, print, observer)
    }

    /// Continues snippet execution with an explicit runtime observer.
    pub fn run_with_observer(
        self,
        result: impl Into<ExternalResult>,
        print: &mut PrintWriter<'_>,
        observer: RuntimeObserverHandle,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let Self {
            mut repl,
            executor,
            vm_state,
            pending_call_id,
            ..
        } = self;

        let ext_result = result.into();

        let context = VMContext::new(&mut repl.heap, &mut repl.namespaces, &executor.interns, print);
        let mut vm = VM::restore_with_observer(vm_state, &executor.module_code, context, observer.clone());

        let vm_result = match ext_result {
            ExternalResult::Return(obj) => {
                emit_external_call_returned(&observer, pending_call_id, ExternalCallReturnKind::Return);
                vm.resume(obj)
            }
            ExternalResult::Error(exc) => {
                emit_external_call_returned(&observer, pending_call_id, ExternalCallReturnKind::Error);
                vm.resume_with_exception(exc.into())
            }
            ExternalResult::Future => {
                emit_external_call_returned(&observer, pending_call_id, ExternalCallReturnKind::Future);
                let call_id = CallId::new(pending_call_id);
                vm.add_pending_call(call_id);
                vm.push_created(Value::ExternalFuture(call_id));
                vm.run()
            }
        };

        let vm_state = vm.check_snapshot(&vm_result);

        handle_repl_vm_result(vm_result, vm_state, executor, repl, observer)
    }

    /// Continues snippet execution by pushing an unresolved `ExternalFuture`.
    ///
    /// This is the REPL-aware async pattern equivalent to `Snapshot::run_pending`.
    pub fn run_pending(self, print: &mut PrintWriter<'_>) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        self.run(MontyFuture, print)
    }
}

/// REPL execution state blocked on unresolved external futures.
///
/// This is the REPL-aware counterpart to `FutureSnapshot`.
///
/// Snapshots can also carry optional embedder-owned extension bytes; Monty
/// persists them but never interprets them.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ReplFutureSnapshot<T: ResourceTracker> {
    /// Persistent REPL session state while this snippet is suspended.
    repl: MontyRepl<T>,
    /// Compiled snippet and intern/function tables for this execution.
    executor: ReplExecutor,
    /// VM stack/frame state at suspension.
    vm_state: VMSnapshot,
    /// Pending call IDs expected by this snapshot.
    pending_call_ids: Vec<u32>,
    /// Optional embedder-owned bytes persisted with this snapshot.
    #[serde(default, rename = "snapshot_extension")]
    extension_bytes: Option<Vec<u8>>,
    /// Optional runtime observer handle for resumed execution.
    #[serde(skip, default = "RuntimeObserverHandle::disabled")]
    observer: RuntimeObserverHandle,
}

impl<T: ResourceTracker> ReplFutureSnapshot<T> {
    /// Attaches embedder-owned snapshot extension bytes to this state.
    ///
    /// These bytes are serialized alongside the snapshot without interpretation
    /// by Monty. The host controls their content and versioning.
    #[must_use]
    pub fn with_snapshot_extension(mut self, snapshot_extension: Vec<u8>) -> Self {
        self.extension_bytes = Some(snapshot_extension);
        self
    }

    /// Returns the embedder-owned snapshot extension bytes, if present.
    #[must_use]
    pub fn snapshot_extension(&self) -> Option<&[u8]> {
        self.extension_bytes.as_deref()
    }

    /// Installs a runtime observer for subsequent resume calls.
    #[must_use]
    pub fn with_observer(mut self, observer: RuntimeObserverHandle) -> Self {
        self.observer = observer;
        self
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
        results: Vec<(u32, ExternalResult)>,
        print: &mut PrintWriter<'_>,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let observer = self.observer.clone();
        self.resume_with_observer(results, print, observer)
    }

    /// Resumes snippet execution with an explicit runtime observer.
    pub fn resume_with_observer(
        self,
        results: Vec<(u32, ExternalResult)>,
        print: &mut PrintWriter<'_>,
        observer: RuntimeObserverHandle,
    ) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
        let Self {
            mut repl,
            executor,
            vm_state,
            pending_call_ids,
            extension_bytes,
            ..
        } = self;

        let invalid_call_id = results
            .iter()
            .find(|(call_id, _)| !pending_call_ids.contains(call_id))
            .map(|(call_id, _)| *call_id);

        let context = VMContext::new(&mut repl.heap, &mut repl.namespaces, &executor.interns, print);
        let mut vm = VM::restore_with_observer(vm_state, &executor.module_code, context, observer.clone());

        if let Some(call_id) = invalid_call_id {
            vm.cleanup();
            let error = MontyException::runtime_error(format!(
                "unknown call_id {call_id}, expected one of: {pending_call_ids:?}"
            ));
            return Err(Box::new(ReplStartError { repl, error }));
        }

        for (call_id, ext_result) in results {
            match ext_result {
                ExternalResult::Return(obj) => {
                    emit_external_call_returned(&observer, call_id, ExternalCallReturnKind::Return);
                    if let Err(e) = vm.resolve_future(call_id, obj) {
                        vm.cleanup();
                        let error =
                            MontyException::runtime_error(format!("Invalid return type for call {call_id}: {e}"));
                        return Err(Box::new(ReplStartError { repl, error }));
                    }
                }
                ExternalResult::Error(exc) => {
                    emit_external_call_returned(&observer, call_id, ExternalCallReturnKind::Error);
                    vm.fail_future(call_id, RunError::from(exc));
                }
                ExternalResult::Future => {
                    emit_external_call_returned(&observer, call_id, ExternalCallReturnKind::Future);
                }
            }
        }

        if let Some(error) = vm.take_failed_task_error() {
            vm.cleanup();
            let error = error.into_python_exception(&executor.interns, &executor.code);
            return Err(Box::new(ReplStartError { repl, error }));
        }

        let main_task_ready = vm.prepare_current_task_after_resolve();

        let loaded_task = match vm.load_ready_task_if_needed() {
            Ok(loaded) => loaded,
            Err(e) => {
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
                    extension_bytes,
                    observer,
                }));
            }
        }

        let vm_result = vm.run();
        let vm_state = vm.check_snapshot(&vm_result);

        handle_repl_vm_result(vm_result, vm_state, executor, repl, observer)
    }
}

/// Handles a `FrameExit` result and converts it to REPL progress.
///
/// This mirrors `handle_vm_result` but preserves REPL heap/namespaces on
/// completion by returning `ReplProgress::Complete { repl, value }`.
/// On runtime errors, the REPL is preserved inside a `ReplStartError`.
///
/// `HostArgs` stores converted host-call payloads:
/// `args`/`kwargs` are host-facing values and `arg_runtime_ids`/`kwarg_runtime_ids`
/// preserve runtime identity metadata aligned to those argument lists.
struct HostArgs {
    args: Vec<MontyObject>,
    arg_runtime_ids: Vec<RuntimeValueId>,
    kwargs: Vec<(MontyObject, MontyObject)>,
    kwarg_runtime_ids: Vec<(RuntimeValueId, RuntimeValueId)>,
}

impl HostArgs {
    /// Converts VM argument storage into host-call payloads with runtime IDs.
    ///
    /// Takes VM-side `args: ArgValues` plus `heap: &mut Heap<T>` and
    /// `interns: &Interns` (`T: ResourceTracker`), and returns `HostArgs` where:
    /// `args` are positional values, `arg_runtime_ids` match those positions,
    /// `kwargs` are key/value pairs, and `kwarg_runtime_ids` align to each pair.
    fn from_vm_args<T: ResourceTracker>(args: crate::args::ArgValues, heap: &mut Heap<T>, interns: &Interns) -> Self {
        let host_args = args.into_py_objects_with_runtime_ids(heap, interns);
        Self {
            args: host_args.args,
            arg_runtime_ids: host_args.arg_runtime_ids,
            kwargs: host_args.kwargs,
            kwarg_runtime_ids: host_args.kwarg_runtime_ids,
        }
    }

    fn into_function_call_progress<T: ResourceTracker>(
        self,
        function_name: String,
        call_id: u32,
        method_call: bool,
        state: ReplSnapshot<T>,
    ) -> ReplProgress<T> {
        ReplProgress::FunctionCall {
            function_name,
            args: self.args,
            arg_runtime_ids: self.arg_runtime_ids,
            kwargs: self.kwargs,
            kwarg_runtime_ids: self.kwarg_runtime_ids,
            call_id,
            method_call,
            state,
        }
    }

    fn into_os_call_progress<T: ResourceTracker>(
        self,
        function: OsFunction,
        call_id: u32,
        state: ReplSnapshot<T>,
    ) -> ReplProgress<T> {
        ReplProgress::OsCall {
            function,
            args: self.args,
            arg_runtime_ids: self.arg_runtime_ids,
            kwargs: self.kwargs,
            kwarg_runtime_ids: self.kwarg_runtime_ids,
            call_id,
            state,
        }
    }
}

/// Builds a runtime error describing a missing REPL VM snapshot.
///
/// This centralises the message used when resumable REPL builders are invoked
/// without `vm_state`, which indicates an internal state mismatch.
fn missing_repl_snapshot_error(context: &str) -> MontyException {
    MontyException::runtime_error(format!("internal error: missing VM snapshot for {context}"))
}

/// Shared REPL-owned state for converting `FrameExit` values into progress.
///
/// For `T: ResourceTracker`, this carries mutable REPL ownership (`repl`) plus
/// compiler metadata and observer state; `vm_state` may be `None`, so builders
/// that require suspension state must validate before constructing snapshots.
struct ReplProgressContext<T: ResourceTracker> {
    /// Suspended VM state, present when execution yielded a resumable operation.
    vm_state: Option<VMSnapshot>,
    /// Compiled snippet/module executor used to convert values and names.
    executor: ReplExecutor,
    /// Owning REPL session state and resource tracker used for value conversion.
    repl: MontyRepl<T>,
    /// Runtime observer handle used to emit progress lifecycle notifications.
    observer: RuntimeObserverHandle,
}

/// Classifies the suspended REPL call variant for shared builder paths.
///
/// This allows one generic call-progress constructor to reuse argument
/// conversion and snapshot logic while preserving variant-specific payload
/// semantics (function, method, or OS call).
enum ReplCallKind {
    Function(String),
    Method(String),
    Os(OsFunction),
}

/// Builds call-progress output for all suspended REPL call kinds.
///
/// This exists to keep conversion/emission/snapshot logic in one place; it
/// consumes `context`, so callers must not expect to reuse `repl` or `executor`
/// after delegation, and snapshot creation will fail if `vm_state` is absent.
fn build_repl_external_call_progress_generic<T: ResourceTracker>(
    kind: ReplCallKind,
    args: crate::args::ArgValues,
    call_id: CallId,
    mut context: ReplProgressContext<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    let host_args = HostArgs::from_vm_args(args, &mut context.repl.heap, &context.executor.interns);
    let (observer_kind, snapshot_desc) = match &kind {
        ReplCallKind::Function(_) => (ExternalCallKind::Function, "external call"),
        ReplCallKind::Method(_) => (ExternalCallKind::Method, "method call"),
        ReplCallKind::Os(_) => (ExternalCallKind::Os, "OS call"),
    };
    let state = build_repl_snapshot(context, call_id.raw(), snapshot_desc)?;
    emit_external_call_requested(
        &state.observer,
        call_id.raw(),
        observer_kind,
        host_args.arg_runtime_ids.as_slice(),
        host_args.kwarg_runtime_ids.as_slice(),
    );
    let progress = match kind {
        ReplCallKind::Function(name) => host_args.into_function_call_progress(name, call_id.raw(), false, state),
        ReplCallKind::Method(name) => host_args.into_function_call_progress(name, call_id.raw(), true, state),
        ReplCallKind::Os(function) => host_args.into_os_call_progress(function, call_id.raw(), state),
    };
    Ok(progress)
}

/// Builds a resumable `ReplSnapshot` from shared REPL progress context.
///
/// This isolates snapshot assembly and missing-state validation so all
/// call-progress paths emit identical errors when `vm_state` is unexpectedly
/// unavailable.
fn build_repl_snapshot<T: ResourceTracker>(
    context: ReplProgressContext<T>,
    pending_call_id: u32,
    snapshot_context: &str,
) -> Result<ReplSnapshot<T>, Box<ReplStartError<T>>> {
    let ReplProgressContext {
        vm_state,
        executor,
        repl,
        observer,
    } = context;
    let Some(vm_state) = vm_state else {
        return Err(Box::new(ReplStartError {
            repl,
            error: missing_repl_snapshot_error(snapshot_context),
        }));
    };
    Ok(ReplSnapshot {
        repl,
        executor,
        vm_state,
        pending_call_id,
        extension_bytes: None,
        observer,
    })
}

/// Commits compiled metadata from a snippet executor into persistent REPL state.
///
/// This exists because snippet execution can mutate symbol/function tables even
/// when execution later fails; callers should run it exactly once when handing
/// ownership of `executor` back to `repl`.
fn commit_repl_executor_metadata<T: ResourceTracker>(repl: &mut MontyRepl<T>, executor: ReplExecutor) {
    let ReplExecutor { name_map, interns, .. } = executor;
    repl.global_name_map = name_map;
    repl.interns = interns;
}

/// Builds `ReplProgress::Complete` from a returned VM value.
///
/// This converts the value using the current heap and commits executor metadata
/// so future snippets observe updated symbols and intern tables.
fn build_repl_complete_progress<T: ResourceTracker>(
    value: Value,
    mut context: ReplProgressContext<T>,
) -> ReplProgress<T> {
    let output = MontyObject::new(value, &mut context.repl.heap, &context.executor.interns);
    commit_repl_executor_metadata(&mut context.repl, context.executor);
    ReplProgress::Complete {
        repl: context.repl,
        value: output,
    }
}

/// Builds external-function call progress for REPL suspension.
///
/// This resolves the external function name and delegates to the generic call
/// builder so request emission and snapshot behaviour stay uniform.
fn build_repl_external_call_progress<T: ResourceTracker>(
    ext_function_id: ExtFunctionId,
    args: crate::args::ArgValues,
    call_id: CallId,
    context: ReplProgressContext<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    let function_name = context.executor.interns.get_external_function_name(ext_function_id);
    build_repl_external_call_progress_generic(ReplCallKind::Function(function_name), args, call_id, context)
}

/// Builds OS-call progress for REPL suspension.
///
/// This exists as a thin adapter so OS calls share the generic conversion and
/// snapshot pipeline without duplicating observer emission logic.
fn build_repl_os_call_progress<T: ResourceTracker>(
    function: OsFunction,
    args: crate::args::ArgValues,
    call_id: CallId,
    context: ReplProgressContext<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    build_repl_external_call_progress_generic(ReplCallKind::Os(function), args, call_id, context)
}

/// Builds method-call progress for REPL suspension.
///
/// This resolves the interned method name, then delegates so method-call
/// progress follows the same snapshot/error semantics as other call kinds.
fn build_repl_method_call_progress<T: ResourceTracker>(
    method_name: crate::value::EitherStr,
    args: crate::args::ArgValues,
    call_id: CallId,
    context: ReplProgressContext<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    let function_name = method_name.into_string(&context.executor.interns);
    build_repl_external_call_progress_generic(ReplCallKind::Method(function_name), args, call_id, context)
}

/// Builds resolve-futures progress when a REPL snippet is blocked on futures.
///
/// This packages pending call IDs into `ReplFutureSnapshot`; like other
/// suspendable paths, it errors if `vm_state` is missing because no resume
/// state can be produced.
fn build_repl_resolve_futures_progress<T: ResourceTracker>(
    pending_call_ids: Vec<CallId>,
    context: ReplProgressContext<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    let ReplProgressContext {
        vm_state,
        executor,
        repl,
        observer,
    } = context;
    let Some(vm_state) = vm_state else {
        return Err(Box::new(ReplStartError {
            repl,
            error: missing_repl_snapshot_error("ResolveFutures"),
        }));
    };
    let pending_call_ids = pending_call_ids.into_iter().map(CallId::raw).collect();
    Ok(ReplProgress::ResolveFutures(ReplFutureSnapshot {
        repl,
        executor,
        vm_state,
        pending_call_ids,
        extension_bytes: None,
        observer,
    }))
}

/// Maps an internal VM `RunError` into a REPL-preserving start error.
///
/// This centralises error conversion while ensuring executor metadata is still
/// committed, because snippets may define symbols before failing.
fn build_repl_runtime_error_progress<T: ResourceTracker>(
    err: RunError,
    mut context: ReplProgressContext<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    let error = err.into_python_exception(&context.executor.interns, &context.executor.code);
    // Commit compiler metadata even on runtime errors, matching feed() behavior.
    // Snippets can create new variables or functions before raising, and those
    // values may reference FunctionId/StringId values from the new tables.
    commit_repl_executor_metadata(&mut context.repl, context.executor);
    Err(Box::new(ReplStartError {
        repl: context.repl,
        error,
    }))
}

/// Dispatches a REPL `FrameExit` into the corresponding progress builder.
///
/// This keeps branch logic local and moves `context` into downstream builders,
/// so each branch can safely consume and return owned REPL state.
fn dispatch_repl_frame_exit<T: ResourceTracker>(
    frame_exit: FrameExit,
    context: ReplProgressContext<T>,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    match frame_exit {
        FrameExit::Return(value) => Ok(build_repl_complete_progress(value, context)),
        FrameExit::ExternalCall {
            ext_function_id,
            args,
            call_id,
        } => build_repl_external_call_progress(ext_function_id, args, call_id, context),
        FrameExit::OsCall {
            function,
            args,
            call_id,
        } => build_repl_os_call_progress(function, args, call_id, context),
        FrameExit::MethodCall {
            method_name,
            args,
            call_id,
        } => build_repl_method_call_progress(method_name, args, call_id, context),
        FrameExit::ResolveFutures(pending_call_ids) => build_repl_resolve_futures_progress(pending_call_ids, context),
    }
}

/// Converts a VM run result into `ReplProgress` using shared context plumbing.
///
/// This exists to centralise success/error dispatch and ensures ownership of
/// `repl`, `executor`, and `vm_state` is consumed into the returned progress or
/// boxed start error.
fn handle_repl_vm_result<T: ResourceTracker>(
    result: RunResult<FrameExit>,
    vm_state: Option<VMSnapshot>,
    executor: ReplExecutor,
    repl: MontyRepl<T>,
    observer: RuntimeObserverHandle,
) -> Result<ReplProgress<T>, Box<ReplStartError<T>>> {
    let context = ReplProgressContext {
        vm_state,
        executor,
        repl,
        observer,
    };
    match result {
        Ok(frame_exit) => dispatch_repl_frame_exit(frame_exit, context),
        Err(err) => build_repl_runtime_error_progress(err, context),
    }
}
