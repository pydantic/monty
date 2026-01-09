//! Public interface for running Monty code.
use crate::{
    exception_private::{ExcType, ExceptionRaise, RunError},
    expressions::Node,
    heap::Heap,
    intern::{ExtFunctionId, Interns, StringId, MODULE_STRING_ID},
    io::{PrintWriter, StdPrint},
    namespace::Namespaces,
    object::MontyObject,
    parse::{parse, CodeRange},
    prepare::prepare,
    resource::{NoLimitTracker, ResourceTracker},
    run_frame::{RunFrame, RunResult},
    snapshot::{CodePosition, ExternalCall, FrameExit, FunctionFrame, NoSnapshotTracker, SnapshotTracker},
    value::Value,
    MontyException,
};

/// Primary interface for running Monty code.
///
/// `MontyRun` supports two execution modes:
/// - **Simple execution**: Use `run()` or `run_no_limits()` to run code to completion
/// - **Iterative execution**: Use `start()` to start execution which will pause at external function calls and
///   can be resumed later
///
/// # Example
/// ```
/// use monty::{MontyRun, MontyObject};
///
/// let runner = MontyRun::new("x + 1".to_owned(), "test.py", vec!["x".to_owned()], vec![]).unwrap();
/// let result = runner.run_no_limits(vec![MontyObject::Int(41)]).unwrap();
/// assert_eq!(result, MontyObject::Int(42));
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyRun {
    /// The underlying executor containing parsed AST and interns.
    executor: Executor,
}

impl MontyRun {
    /// Creates a new run snapshot by parsing the given code.
    ///
    /// This only parses and prepares the code - no heap or namespaces are created yet.
    /// Call `run_snapshot()` with inputs to start execution.
    ///
    /// # Arguments
    /// * `code` - The Python code to execute
    /// * `script_name` - The script name for error messages
    /// * `input_names` - Names of input variables
    ///
    /// # Errors
    /// Returns `MontyException` if the code cannot be parsed.
    pub fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        external_functions: Vec<String>,
    ) -> Result<Self, MontyException> {
        Executor::new(code, script_name, input_names, external_functions).map(|executor| Self { executor })
    }

    /// Returns the code that was parsed to create this snapshot.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.executor.code
    }

    /// Executes the code and returns both the result and reference count data, used for testing only.
    #[cfg(feature = "ref-count-return")]
    pub fn run_ref_counts(&self, inputs: Vec<MontyObject>) -> Result<RefCountOutput, MontyException> {
        self.executor.run_ref_counts(inputs)
    }

    /// Executes the code to completion assuming not external functions or snapshotting.
    ///
    /// This is marginally faster than running with snapshotting enabled since we don't need
    /// to track the position in code, but does not allow calling of external functions.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace
    /// * `resource_tracker` - Custom resource tracker implementation
    /// * `print` - print print implementation
    pub fn run(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: impl ResourceTracker,
        print: &mut impl PrintWriter,
    ) -> Result<MontyObject, MontyException> {
        self.executor.run_with_tracker(inputs, resource_tracker, print)
    }

    /// Executes the code to completion with no resource limits, printing to stdout/stderr.
    pub fn run_no_limits(&self, inputs: Vec<MontyObject>) -> Result<MontyObject, MontyException> {
        self.run(inputs, NoLimitTracker::default(), &mut StdPrint)
    }

    /// Serializes the runner to a binary format.
    ///
    /// The serialized data can be stored and later restored with `load()`.
    /// This allows caching parsed code to avoid re-parsing on subsequent runs.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn dump(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    /// Deserializes a runner from binary format.
    ///
    /// # Arguments
    /// * `bytes` - The serialized runner data from `dump()`
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn load(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }

    /// Starts execution with the given inputs and resource tracker, consuming self.
    ///
    /// Creates the heap and namespaces, then begins execution.
    ///
    /// For iterative execution, `start()` consumes self and returns a `RunProgress`:
    /// - `RunProgress::FunctionCall { ..., state }` - external function call, call `state.run(return_value)` to resume
    /// - `RunProgress::Complete(value)` - execution finished
    ///
    /// This enables snapshotting execution state and returning control to the host
    /// application during long-running computations.
    ///
    /// # Arguments
    /// * `inputs` - Initial input values (must match length of `input_names` from `new()`)
    /// * `resource_tracker` - Resource tracker for the execution
    /// * `print` - Writer for print output
    ///
    /// # Errors
    /// Returns `MontyException` if:
    /// - The number of inputs doesn't match the expected count
    /// - An input value is invalid (e.g., `MontyObject::Repr`)
    /// - A runtime error occurs during execution
    pub fn start<T: ResourceTracker>(
        self,
        inputs: Vec<MontyObject>,
        resource_tracker: T,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        let mut heap = Heap::new(self.executor.namespace_size, resource_tracker);

        let namespaces = self.executor.prepare_namespaces(inputs, &mut heap)?;

        // Start execution from index 0 (beginning of code)
        let snapshot_tracker = SnapshotTracker::default();
        self.executor
            .run_from_position(heap, namespaces, snapshot_tracker, print)
    }
}

/// Result of a single step of iterative execution.
///
/// This enum owns the execution state, ensuring type-safe state transitions.
/// - `FunctionCall` contains info about an external function call and state to resume
/// - `Complete` contains just the final value (execution is done)
///
/// # Type Parameters
/// * `T` - Resource tracker implementation (e.g., `NoLimitTracker` or `LimitedTracker`)
///
/// Serialization requires `T: Serialize + Deserialize`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub enum RunProgress<T: ResourceTracker> {
    /// Execution paused at an external function call. Call `state.run(return_value)` to resume.
    FunctionCall {
        /// The name of the function being called.
        function_name: String,
        /// The positional arguments passed to the function.
        args: Vec<MontyObject>,
        /// The keyword arguments passed to the function (key, value pairs).
        kwargs: Vec<(MontyObject, MontyObject)>,
        /// The execution state that can be resumed with a return value.
        state: Snapshot<T>,
    },
    /// Execution completed with a final result.
    Complete(MontyObject),
}

impl<T: ResourceTracker> RunProgress<T> {
    /// Consumes the `RunProgress` and returns external function call info and state.
    ///
    /// Returns (function_name, positional_args, keyword_args, state).
    #[allow(clippy::type_complexity)]
    pub fn into_function_call(
        self,
    ) -> Option<(String, Vec<MontyObject>, Vec<(MontyObject, MontyObject)>, Snapshot<T>)> {
        match self {
            RunProgress::FunctionCall {
                function_name,
                args,
                kwargs,
                state,
            } => Some((function_name, args, kwargs, state)),
            RunProgress::Complete(_) => None,
        }
    }

    /// Consumes the `RunProgress` and returns the final value.
    pub fn into_complete(self) -> Option<MontyObject> {
        match self {
            RunProgress::Complete(value) => Some(value),
            RunProgress::FunctionCall { .. } => None,
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

/// Execution state that can be resumed after an external function call.
///
/// This struct owns all runtime state and provides a `run()` method to continue
/// execution with the return value from the external function. When `run()` is
/// called, it consumes self and returns the next `RunProgress`.
///
/// External function calls occur when calling a function that is not a builtin,
/// exception, or user-defined function.
///
/// # Type Parameters
/// * `T` - Resource tracker implementation
///
/// Serialization requires `T: Serialize + Deserialize`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub struct Snapshot<T: ResourceTracker> {
    /// The underlying executor containing parsed AST and interns.
    executor: Executor,
    /// The heap for allocating runtime values.
    heap: Heap<T>,
    /// The namespace stack for variable storage.
    namespaces: Namespaces,
    /// Stack of execution positions for resuming inside nested control flow.
    position_stack: Vec<CodePosition>,
    /// Stack of suspended function frames (outermost first, innermost last).
    /// Empty when external call is at module level.
    call_stack: Vec<FunctionFrame>,
    /// The source position of the external call that suspended execution.
    /// Used to match return values to the correct call site when resuming.
    ext_call_position: CodeRange,
}

/// Return value or exception from an external function.
#[derive(Debug)]
pub enum ExternalResult {
    /// Continues execution with the return value from the external function.
    Return(MontyObject),
    /// Continues execution with the exception raised by the external function.
    Error(MontyException),
}

impl From<MontyObject> for ExternalResult {
    fn from(value: MontyObject) -> Self {
        ExternalResult::Return(value)
    }
}

impl From<MontyException> for ExternalResult {
    fn from(exception: MontyException) -> Self {
        ExternalResult::Error(exception)
    }
}

impl<T: ResourceTracker> Snapshot<T> {
    /// Continues execution with the return value or exception from the external function.
    ///
    /// Consumes self and returns the next execution progress.
    ///
    /// # Arguments
    /// * `result` - The return value or exception from the external function
    /// * `print` - The print writer to use for output
    pub fn run(
        self,
        result: impl Into<ExternalResult>,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        match result.into() {
            ExternalResult::Return(return_value) => self.run_return(return_value, print),
            ExternalResult::Error(exception) => self.run_exception(exception, print),
        }
    }

    /// Continues execution with the return value from the external function.
    fn run_return(
        mut self,
        return_value: MontyObject,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        // Convert MontyObject to Value
        let value = return_value
            .to_value(&mut self.heap, &self.executor.interns)
            .map_err(|_| {
                RunError::internal("invalid return value type")
                    .into_python_exception(&self.executor.interns, &self.executor.code)
            })?;

        // Store the return value in func_return_values map by position.
        // This allows position-based lookup which handles nested calls correctly.
        // The map is cleared in clear_on_function_complete when function frames complete,
        // which handles recursion (each frame gets fresh cached values).
        self.namespaces.set_func_return_value(self.ext_call_position, value);

        if self.call_stack.is_empty() {
            // Module-level resume - continue execution from saved position
            let snapshot_tracker = SnapshotTracker::new(self.position_stack);
            self.executor
                .run_from_position(self.heap, self.namespaces, snapshot_tracker, print)
        } else {
            // Resume inside function call stack
            self.resume_call_stack(print)
        }
    }

    /// Continues execution with the exception raised by the external function.
    fn run_exception(
        mut self,
        exc: MontyException,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        // Convert MontyException to ExceptionRaise and store as pending exception
        let exc_raise: ExceptionRaise = exc.into();
        self.namespaces.set_ext_exception(exc_raise);

        if self.call_stack.is_empty() {
            // Module-level resume - continue execution from saved position
            let snapshot_tracker = SnapshotTracker::new(self.position_stack);
            self.executor
                .run_from_position(self.heap, self.namespaces, snapshot_tracker, print)
        } else {
            // Resume inside function call stack
            self.resume_call_stack(print)
        }
    }

    /// Resumes execution inside the function call stack.
    ///
    /// Pops the innermost function frame, continues execution, and propagates
    /// the result up through the remaining call stack.
    fn resume_call_stack(mut self, print: &mut impl PrintWriter) -> Result<RunProgress<T>, MontyException> {
        // Pop the innermost frame (last in the list)
        let frame = self.call_stack.pop().expect("call_stack should not be empty");

        // Get the function definition
        let function = self.executor.interns.get_function(frame.function_id);

        // Use the function's saved positions, not the caller's position stack
        let mut snapshot_tracker = SnapshotTracker::new(frame.saved_positions);

        // Create a RunFrame for this function and continue execution
        let mut run_frame = RunFrame::function_frame(
            frame.namespace_idx,
            frame.name_id,
            &self.executor.interns,
            &mut snapshot_tracker,
            print,
        );

        // Execute from the saved position
        let result = run_frame.execute(&mut self.namespaces, &mut self.heap, &function.body);

        // Handle the result
        match result {
            Ok(Some(FrameExit::Return(return_value))) => {
                // Function completed - clean up its namespace
                self.namespaces.drop_with_heap(frame.namespace_idx, &mut self.heap);

                // Clear all cached values since this function has consumed them.
                // This clears func_return_values and argument_cache to prevent
                // outer recursion levels from seeing inner levels' cached values.
                self.namespaces.clear_on_function_complete(&mut self.heap);

                // Store the return value in func_return_values map by position.
                // This allows the caller to look it up when re-evaluating the call expression.
                // Using the map (not the vec) ensures values persist across multiple resumes.
                self.namespaces.set_func_return_value(frame.call_position, return_value);

                if self.call_stack.is_empty() {
                    // All functions completed, continue at module level
                    // Use the caller's position_stack (module level), not the function's
                    self.executor.run_from_position(
                        self.heap,
                        self.namespaces,
                        SnapshotTracker::new(self.position_stack),
                        print,
                    )
                } else {
                    // More functions in the stack - continue with the next one
                    // position_stack stays the same (it's the outermost caller's positions)
                    self.resume_call_stack(print)
                }
            }
            Ok(Some(FrameExit::ExternalCall(mut ext_call))) => {
                // Another external call - push this frame back and pause
                // Save the function's current positions
                let saved_positions = snapshot_tracker.into_stack();
                ext_call.push_frame(FunctionFrame {
                    function_id: frame.function_id,
                    namespace_idx: frame.namespace_idx,
                    name_id: frame.name_id,
                    captured_cell_count: frame.captured_cell_count,
                    saved_positions,
                    call_position: frame.call_position,
                });
                // Reverse ext_call.call_stack to outermost-first order (it was built with push)
                ext_call.call_stack.reverse();
                // Combine with remaining call stack (already outermost-first)
                let mut new_call_stack = self.call_stack;
                new_call_stack.append(&mut ext_call.call_stack);
                ext_call.call_stack = new_call_stack;

                let ext_call_position = ext_call.call_position;
                let (args, kwargs) = ext_call.args.into_py_objects(&mut self.heap, &self.executor.interns);
                Ok(RunProgress::FunctionCall {
                    function_name: self.executor.interns.get_external_function_name(ext_call.function_id),
                    args,
                    kwargs,
                    state: Snapshot {
                        executor: self.executor,
                        heap: self.heap,
                        namespaces: self.namespaces,
                        // Use the caller's position_stack (unchanged)
                        position_stack: self.position_stack,
                        call_stack: ext_call.call_stack,
                        ext_call_position,
                    },
                })
            }
            Ok(None) => {
                // Function completed with implicit None - clean up its namespace
                self.namespaces.drop_with_heap(frame.namespace_idx, &mut self.heap);

                // Clear all cached values since this function has consumed them.
                // This clears func_return_values and argument_cache to prevent
                // outer recursion levels from seeing inner levels' cached values.
                self.namespaces.clear_on_function_complete(&mut self.heap);

                // Store the return value (None) in func_return_values map by position.
                // This allows the caller to look it up when re-evaluating the call expression.
                self.namespaces.set_func_return_value(frame.call_position, Value::None);

                if self.call_stack.is_empty() {
                    // All functions completed, continue at module level
                    // Use the caller's position_stack (module level), not the function's
                    self.executor.run_from_position(
                        self.heap,
                        self.namespaces,
                        SnapshotTracker::new(self.position_stack),
                        print,
                    )
                } else {
                    // More functions in the stack - continue with the next one
                    // position_stack stays the same (it's the outermost caller's positions)
                    self.resume_call_stack(print)
                }
            }
            Err(mut e) => {
                // Error occurred - add frames for the suspended call stack and clean up namespaces
                self.namespaces.drop_with_heap(frame.namespace_idx, &mut self.heap);

                // Add frame for where this function was called from.
                // Derive caller name from parent frame (or <module> if no parent).
                let caller_name = self.call_stack.last().map_or(MODULE_STRING_ID, |f| f.name_id);
                add_suspended_frame_info(&mut e, caller_name, frame.call_position);

                // Add frames for remaining call stack (innermost to outermost).
                // Each frame's caller is the previous frame in the stack, or <module> for the outermost.
                for (i, f) in self.call_stack.iter().enumerate().rev() {
                    self.namespaces.drop_with_heap(f.namespace_idx, &mut self.heap);
                    let caller_name = if i > 0 {
                        self.call_stack[i - 1].name_id
                    } else {
                        MODULE_STRING_ID
                    };
                    add_suspended_frame_info(&mut e, caller_name, f.call_position);
                }
                #[cfg(feature = "ref-count-panic")]
                self.namespaces.drop_global_with_heap(&mut self.heap);
                Err(e.into_python_exception(&self.executor.interns, &self.executor.code))
            }
        }
    }
}

/// Lower level interface to parse code and run it to completion.
///
/// This is an internal type used by [`MontyRun`]. It stores the compiled AST and source code
/// for error reporting but does not support external functions or iterative execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Executor {
    namespace_size: usize,
    /// Maps variable names to their indices in the namespace. Used for ref-count testing.
    #[cfg(feature = "ref-count-return")]
    name_map: ahash::AHashMap<String, crate::namespace::NamespaceId>,
    nodes: Vec<Node>,
    /// Interned strings used for looking up names and filenames during execution.
    interns: Interns,
    /// ids to create values to inject into the the namespace to represent external functions.
    external_function_ids: Vec<ExtFunctionId>,
    /// Source code for error reporting (extracting preview lines for tracebacks).
    code: String,
}

impl Executor {
    /// Creates a new executor with the given code, filename, input names, and external functions.
    fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        external_functions: Vec<String>,
    ) -> Result<Self, MontyException> {
        let parse_result = parse(&code, script_name).map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare(parse_result, input_names, &external_functions)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        // incrementing order matches the indexes used in intern::Interns::get_external_function_name
        let external_function_ids = (0..external_functions.len()).map(ExtFunctionId::new).collect();

        Ok(Self {
            namespace_size: prepared.namespace_size,
            #[cfg(feature = "ref-count-return")]
            name_map: prepared.name_map,
            nodes: prepared.nodes,
            interns: Interns::new(prepared.interner, prepared.functions, external_functions),
            external_function_ids,
            code,
        })
    }

    /// Executes the code with a custom resource tracker.
    ///
    /// This provides full control over resource tracking and garbage collection
    /// scheduling. The tracker is called on each allocation and periodically
    /// during execution to check time limits and trigger GC.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace
    /// * `resource_tracker` - Custom resource tracker implementation
    /// * `print` - print print implementation
    ///
    fn run_with_tracker(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: impl ResourceTracker,
        print: &mut impl PrintWriter,
    ) -> Result<MontyObject, MontyException> {
        let mut heap = Heap::new(self.namespace_size, resource_tracker);
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        let mut snapshot_tracker = NoSnapshotTracker;
        let mut frame = RunFrame::module_frame(&self.interns, &mut snapshot_tracker, print);
        let frame_exit_result = frame.execute(&mut namespaces, &mut heap, &self.nodes);

        // Clean up the global namespace before returning (only needed with ref-count-panic)
        #[cfg(feature = "ref-count-panic")]
        namespaces.drop_global_with_heap(&mut heap);

        frame_exit_to_object(frame_exit_result, &mut heap, &self.interns)
            .map_err(|e| e.into_python_exception(&self.interns, &self.code))
    }

    /// Executes the code and returns both the result and reference count data, used for testing only.
    ///
    /// This is used for testing reference counting behavior. Returns:
    /// - The execution result (`Exit`)
    /// - Reference count data as a tuple of:
    ///   - A map from variable names to their reference counts (only for heap-allocated values)
    ///   - The number of unique heap value IDs referenced by variables
    ///   - The total number of live heap values
    ///
    /// For strict matching validation, compare unique_refs_count with heap_entry_count.
    /// If they're equal, all heap values are accounted for by named variables.
    ///
    /// Only available when the `ref-count-return` feature is enabled.
    #[cfg(feature = "ref-count-return")]
    fn run_ref_counts(&self, inputs: Vec<MontyObject>) -> Result<RefCountOutput, MontyException> {
        use std::collections::HashSet;

        use crate::value::Value;

        let mut heap = Heap::new(self.namespace_size, NoLimitTracker::default());
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        let mut snapshot_tracker = NoSnapshotTracker;
        let mut print_writer = StdPrint;
        let mut frame = RunFrame::module_frame(&self.interns, &mut snapshot_tracker, &mut print_writer);
        // Use execute() instead of execute_py_object() so the return value stays alive
        // while we compute refcounts
        let frame_exit_result = frame.execute(&mut namespaces, &mut heap, &self.nodes);

        // Compute ref counts before consuming the heap - return value is still alive in frame_exit
        let final_namespace = namespaces.into_global();
        let mut counts = ahash::AHashMap::new();
        let mut unique_ids = HashSet::new();

        for (name, &namespace_id) in &self.name_map {
            if let Some(Value::Ref(id)) = final_namespace.get_opt(namespace_id) {
                counts.insert(name.clone(), heap.get_refcount(*id));
                unique_ids.insert(*id);
            }
        }
        let unique_refs = unique_ids.len();
        let heap_count = heap.entry_count();

        // Clean up the namespace after reading ref counts but before moving the heap
        for obj in final_namespace {
            obj.drop_with_heap(&mut heap);
        }

        // Now convert the return value to MontyObject (this drops the Value, decrementing refcount)
        let py_object = frame_exit_to_object(frame_exit_result, &mut heap, &self.interns)
            .map_err(|e| e.into_python_exception(&self.interns, &self.code))?;

        Ok(RefCountOutput {
            py_object,
            counts,
            unique_refs,
            heap_count,
        })
    }

    /// Prepares the namespace namespaces for execution.
    ///
    /// Converts each `MontyObject` input to a `Value`, allocating on the heap if needed.
    /// Returns the prepared Namespaces or an error if there are too many inputs or invalid input types.
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
        // register external functions in the namespace first, matching the logic in prepare
        let mut namespace: Vec<Value> = Vec::with_capacity(self.namespace_size);
        for f_id in &self.external_function_ids {
            namespace.push(Value::ExtFunction(*f_id));
        }
        // Convert each MontyObject to a Value, propagating any invalid input errors
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

    /// Internal helper to run execution from a position stack.
    ///
    /// Shared by both `MontyRun` and `Snapshot::run`.
    fn run_from_position<T: ResourceTracker>(
        self,
        mut heap: Heap<T>,
        mut namespaces: Namespaces,
        mut snapshot_tracker: SnapshotTracker,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        let mut frame = RunFrame::module_frame(&self.interns, &mut snapshot_tracker, print);
        let exit = match frame.execute(&mut namespaces, &mut heap, &self.nodes) {
            Ok(exit) => exit,
            Err(e) => {
                // Clean up before propagating error (only needed with ref-count-panic)
                #[cfg(feature = "ref-count-panic")]
                namespaces.drop_global_with_heap(&mut heap);
                return Err(e.into_python_exception(&self.interns, &self.code));
            }
        };

        match exit {
            None => {
                // Clean up the global namespace before returning (only needed with ref-count-panic)
                #[cfg(feature = "ref-count-panic")]
                namespaces.drop_global_with_heap(&mut heap);

                Ok(RunProgress::Complete(MontyObject::None))
            }
            Some(FrameExit::Return(return_value)) => {
                // Clean up the global namespace before returning (only needed with ref-count-panic)
                #[cfg(feature = "ref-count-panic")]
                namespaces.drop_global_with_heap(&mut heap);

                let py_object = MontyObject::new(return_value, &mut heap, &self.interns);
                Ok(RunProgress::Complete(py_object))
            }
            Some(FrameExit::ExternalCall(ExternalCall {
                function_id,
                args,
                mut call_stack,
                call_position: ext_call_position,
            })) => {
                // Reverse call_stack so outermost is first, innermost is last.
                // This allows pop() to return the innermost frame first during resume.
                // Building with push() is O(1) per frame; reversing once is O(n) total.
                call_stack.reverse();

                let (args, kwargs) = args.into_py_objects(&mut heap, &self.interns);
                Ok(RunProgress::FunctionCall {
                    function_name: self.interns.get_external_function_name(function_id),
                    args,
                    kwargs,
                    state: Snapshot {
                        executor: self,
                        heap,
                        namespaces,
                        position_stack: snapshot_tracker.into_stack(),
                        call_stack,
                        ext_call_position,
                    },
                })
            }
        }
    }
}

/// Adds a stack frame to an exception for a suspended function.
///
/// When an exception propagates through the suspended call stack, we need to add
/// frames for each function that was suspended. The `call_position` is where the
/// function was called from (the call site in the calling function).
fn add_suspended_frame_info(error: &mut RunError, name_id: StringId, call_position: CodeRange) {
    match error {
        RunError::Exc(exc) | RunError::UncatchableExc(exc) => {
            exc.add_caller_frame(call_position, name_id);
        }
        RunError::Internal(_) => {}
    }
}

fn frame_exit_to_object(
    frame_exit_result: RunResult<Option<FrameExit>>,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<MontyObject> {
    match frame_exit_result? {
        Some(FrameExit::Return(return_value)) => Ok(MontyObject::new(return_value, heap, interns)),
        Some(FrameExit::ExternalCall(_)) => {
            Err(ExcType::not_implemented("external function calls not supported by standard execution.").into())
        }
        None => Ok(MontyObject::None),
    }
}

#[cfg(feature = "ref-count-return")]
#[derive(Debug)]
pub struct RefCountOutput {
    pub py_object: MontyObject,
    pub counts: ahash::AHashMap<String, usize>,
    pub unique_refs: usize,
    pub heap_count: usize,
}
