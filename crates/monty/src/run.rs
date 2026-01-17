//! Public interface for running Monty code.
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    ExcType, MontyException,
    bytecode::{Code, Compiler, FrameExit, VM, VMSnapshot},
    exception_private::{RunError, RunResult},
    heap::Heap,
    intern::{ExtFunctionId, Interns},
    io::{PrintWriter, StdPrint},
    namespace::Namespaces,
    object::MontyObject,
    parse::parse,
    prepare::prepare,
    resource::{NoLimitTracker, ResourceTracker},
    value::Value,
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
        self.executor.run(inputs, resource_tracker, print)
    }

    /// Executes the code to completion with no resource limits, printing to stdout/stderr.
    pub fn run_no_limits(&self, inputs: Vec<MontyObject>) -> Result<MontyObject, MontyException> {
        self.run(inputs, NoLimitTracker, &mut StdPrint)
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
    ///
    /// # Panics
    /// This method should not panic under normal operation. Internal assertions
    /// may panic if the VM reaches an inconsistent state (indicating a bug).
    pub fn start<T: ResourceTracker>(
        self,
        inputs: Vec<MontyObject>,
        resource_tracker: T,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        let executor = self.executor;

        // Create heap and prepare namespaces
        let mut heap = Heap::new(executor.namespace_size, resource_tracker);
        let mut namespaces = executor.prepare_namespaces(inputs, &mut heap)?;

        // Create and run VM - scope the VM borrow so we can move heap/namespaces after
        let (result, vm_state) = {
            let mut vm = VM::new(&mut heap, &mut namespaces, &executor.interns, print);
            let result = vm.run_module(&executor.module_code);

            // Handle the result - convert VM to snapshot if needed for external call
            if let Ok(FrameExit::ExternalCall { .. }) = &result {
                // Need to snapshot the VM for resumption
                (result, Some(vm.into_snapshot()))
            } else {
                // Clean up VM state
                vm.cleanup();
                (result, None)
            }
        };

        // Now handle the result with owned heap and namespaces
        match result {
            Ok(FrameExit::Return(value)) => {
                // Clean up the global namespace before returning (only needed with ref-count-panic)
                #[cfg(feature = "ref-count-panic")]
                namespaces.drop_global_with_heap(&mut heap);

                // Convert to MontyObject
                let obj = MontyObject::new(value, &mut heap, &executor.interns);
                Ok(RunProgress::Complete(obj))
            }
            Ok(FrameExit::ExternalCall { ext_function_id, args }) => {
                // Get function name and convert args to MontyObjects (includes both positional and kwargs)
                let function_name = executor.interns.get_external_function_name(ext_function_id);
                let (args_py, kwargs_py) = args.into_py_objects(&mut heap, &executor.interns);

                Ok(RunProgress::FunctionCall {
                    function_name,
                    args: args_py,
                    kwargs: kwargs_py,
                    state: Snapshot {
                        executor,
                        vm_state: vm_state.expect("snapshot should exist for ExternalCall"),
                        heap,
                        namespaces,
                    },
                })
            }
            Err(err) => {
                // Clean up the global namespace before returning (only needed with ref-count-panic)
                #[cfg(feature = "ref-count-panic")]
                namespaces.drop_global_with_heap(&mut heap);

                // Convert to MontyException
                Err(err.into_python_exception(&executor.interns, &executor.code))
            }
        }
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
#[expect(clippy::large_enum_variant)]
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
    #[must_use]
    #[expect(clippy::type_complexity)]
    pub fn into_function_call(
        self,
    ) -> Option<(String, Vec<MontyObject>, Vec<(MontyObject, MontyObject)>, Snapshot<T>)> {
        match self {
            Self::FunctionCall {
                function_name,
                args,
                kwargs,
                state,
            } => Some((function_name, args, kwargs, state)),
            Self::Complete(_) => None,
        }
    }

    /// Consumes the `RunProgress` and returns the final value.
    #[must_use]
    pub fn into_complete(self) -> Option<MontyObject> {
        match self {
            Self::Complete(value) => Some(value),
            Self::FunctionCall { .. } => None,
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
    /// The executor containing compiled code and interns.
    executor: Executor,
    /// The VM state containing stack, frames, and exception state.
    vm_state: VMSnapshot,
    /// The heap containing all allocated objects.
    heap: Heap<T>,
    /// The namespaces containing all variable bindings.
    namespaces: Namespaces,
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
        Self::Return(value)
    }
}

impl From<MontyException> for ExternalResult {
    fn from(exception: MontyException) -> Self {
        Self::Error(exception)
    }
}

/// Helper enum for resuming execution with either a return value or an exception.
///
/// Used by `Snapshot::run` to decide whether to call `VM::resume` (for normal returns)
/// or `VM::resume_with_exception` (for external function errors).
enum ResumeWith {
    /// External function returned a value normally.
    Value(Value),
    /// External function raised an exception.
    Exception(RunError),
}

impl<T: ResourceTracker> Snapshot<T> {
    /// Continues execution with the return value or exception from the external function.
    ///
    /// Consumes self and returns the next execution progress.
    ///
    /// # Arguments
    /// * `result` - The return value or exception from the external function
    /// * `print` - The print writer to use for output
    ///
    /// # Panics
    /// This method should not panic under normal operation. Internal assertions
    /// may panic if the VM reaches an inconsistent state (indicating a bug).
    pub fn run(
        mut self,
        result: impl Into<ExternalResult>,
        print: &mut impl PrintWriter,
    ) -> Result<RunProgress<T>, MontyException> {
        let ext_result = result.into();

        // Convert return value or exception before creating VM (to avoid borrow conflicts)
        let resume_with = match ext_result {
            ExternalResult::Return(obj) => match obj.to_value(&mut self.heap, &self.executor.interns) {
                Ok(value) => ResumeWith::Value(value),
                Err(e) => {
                    return Err(MontyException::runtime_error(format!("invalid return type: {e}")));
                }
            },
            ExternalResult::Error(exc) => ResumeWith::Exception(exc.into()),
        };

        // Scope the VM borrow so we can move heap/namespaces after
        let (result, vm_state) = {
            // Restore the VM from the snapshot
            let mut vm = VM::restore(
                self.vm_state,
                &self.executor.module_code,
                &mut self.heap,
                &mut self.namespaces,
                &self.executor.interns,
                print,
            );

            // Resume execution with the result or exception
            let vm_result = match resume_with {
                ResumeWith::Value(value) => vm.resume(value),
                ResumeWith::Exception(error) => vm.resume_with_exception(error),
            };

            // Handle the result - convert VM to snapshot if needed for external call
            if let Ok(FrameExit::ExternalCall { .. }) = &vm_result {
                // Need to snapshot the VM for resumption
                (vm_result, Some(vm.into_snapshot()))
            } else {
                // Clean up VM state
                vm.cleanup();
                (vm_result, None)
            }
        };

        // Now handle the result with owned heap and namespaces
        match result {
            Ok(FrameExit::Return(value)) => {
                // Clean up the global namespace before returning (only needed with ref-count-panic)
                #[cfg(feature = "ref-count-panic")]
                self.namespaces.drop_global_with_heap(&mut self.heap);

                // Convert to MontyObject
                let obj = MontyObject::new(value, &mut self.heap, &self.executor.interns);
                Ok(RunProgress::Complete(obj))
            }
            Ok(FrameExit::ExternalCall { ext_function_id, args }) => {
                // Get function name and convert args to MontyObjects (includes both positional and kwargs)
                let function_name = self.executor.interns.get_external_function_name(ext_function_id);
                let (args_py, kwargs_py) = args.into_py_objects(&mut self.heap, &self.executor.interns);

                Ok(RunProgress::FunctionCall {
                    function_name,
                    args: args_py,
                    kwargs: kwargs_py,
                    state: Self {
                        executor: self.executor,
                        vm_state: vm_state.expect("snapshot should exist for ExternalCall"),
                        heap: self.heap,
                        namespaces: self.namespaces,
                    },
                })
            }
            Err(err) => {
                // Clean up the global namespace before returning (only needed with ref-count-panic)
                #[cfg(feature = "ref-count-panic")]
                self.namespaces.drop_global_with_heap(&mut self.heap);

                // Convert to MontyException
                Err(err.into_python_exception(&self.executor.interns, &self.executor.code))
            }
        }
    }
}

/// Lower level interface to parse code and run it to completion.
///
/// This is an internal type used by [`MontyRun`]. It stores the compiled bytecode and source code
/// for error reporting.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Executor {
    /// Number of slots needed in the global namespace.
    namespace_size: usize,
    /// Maps variable names to their indices in the namespace. Used for ref-count testing.
    #[cfg(feature = "ref-count-return")]
    name_map: ahash::AHashMap<String, crate::namespace::NamespaceId>,
    /// Compiled bytecode for the module.
    module_code: Code,
    /// Interned strings used for looking up names and filenames during execution.
    interns: Interns,
    /// IDs to create values to inject into the the namespace to represent external functions.
    external_function_ids: Vec<ExtFunctionId>,
    /// Source code for error reporting (extracting preview lines for tracebacks).
    code: String,
    /// Estimated heap capacity for pre-allocation on subsequent runs.
    /// Uses AtomicUsize for thread-safety (required by PyO3's Sync bound).
    heap_capacity: AtomicUsize,
}

impl Clone for Executor {
    fn clone(&self) -> Self {
        Self {
            namespace_size: self.namespace_size,
            #[cfg(feature = "ref-count-return")]
            name_map: self.name_map.clone(),
            module_code: self.module_code.clone(),
            interns: self.interns.clone(),
            external_function_ids: self.external_function_ids.clone(),
            code: self.code.clone(),
            heap_capacity: AtomicUsize::new(self.heap_capacity.load(Ordering::Relaxed)),
        }
    }
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

        // Incrementing order matches the indexes used in intern::Interns::get_external_function_name
        let external_function_ids = (0..external_functions.len()).map(ExtFunctionId::new).collect();

        // Create interns with empty functions (functions will be set after compilation)
        let mut interns = Interns::new(prepared.interner, Vec::new(), external_functions);

        // Compile the module to bytecode, which also compiles all nested functions
        let namespace_size_u16 = u16::try_from(prepared.namespace_size).expect("module namespace size exceeds u16");
        let compile_result = Compiler::compile_module(&prepared.nodes, &interns, namespace_size_u16)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        // Set the compiled functions in the interns
        interns.set_functions(compile_result.functions);

        Ok(Self {
            namespace_size: prepared.namespace_size,
            #[cfg(feature = "ref-count-return")]
            name_map: prepared.name_map,
            module_code: compile_result.code,
            interns,
            external_function_ids,
            code,
            heap_capacity: AtomicUsize::new(prepared.namespace_size),
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
    /// * `print` - Print implementation for print() output
    fn run(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: impl ResourceTracker,
        print: &mut impl PrintWriter,
    ) -> Result<MontyObject, MontyException> {
        let heap_capacity = self.heap_capacity.load(Ordering::Relaxed);
        let mut heap = Heap::new(heap_capacity, resource_tracker);
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        // Create and run VM
        let mut vm = VM::new(&mut heap, &mut namespaces, &self.interns, print);
        let frame_exit_result = vm.run_module(&self.module_code);

        // Clean up VM state before it goes out of scope
        vm.cleanup();

        if heap.size() > heap_capacity {
            self.heap_capacity.store(heap.size(), Ordering::Relaxed);
        }

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

        let mut heap = Heap::new(self.namespace_size, NoLimitTracker);
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        // Create and run VM with StdPrint for output
        let mut print = StdPrint;
        let mut vm = VM::new(&mut heap, &mut namespaces, &self.interns, &mut print);
        let frame_exit_result = vm.run_module(&self.module_code);

        // Compute ref counts before consuming the heap - return value is still alive
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

        let allocations_since_gc = heap.get_allocations_since_gc();

        Ok(RefCountOutput {
            py_object,
            counts,
            unique_refs,
            heap_count,
            allocations_since_gc,
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
}

fn frame_exit_to_object(
    frame_exit_result: RunResult<FrameExit>,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<MontyObject> {
    match frame_exit_result? {
        FrameExit::Return(return_value) => Ok(MontyObject::new(return_value, heap, interns)),
        FrameExit::ExternalCall { .. } => {
            Err(ExcType::not_implemented("external function calls not supported by standard execution.").into())
        }
    }
}

/// Output from `run_ref_counts` containing reference count and heap information.
///
/// Used for testing GC behavior and reference counting correctness.
#[cfg(feature = "ref-count-return")]
#[derive(Debug)]
pub struct RefCountOutput {
    pub py_object: MontyObject,
    pub counts: ahash::AHashMap<String, usize>,
    pub unique_refs: usize,
    pub heap_count: usize,
    /// Number of GC-tracked allocations since the last garbage collection.
    ///
    /// If GC ran during execution, this will be lower than the total number of
    /// allocations. Compare this against expected allocation count to verify GC ran.
    pub allocations_since_gc: u32,
}
