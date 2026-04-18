//! Public interface for running Monty code.
use std::sync::atomic::{AtomicUsize, Ordering};

use ahash::AHashMap;

use crate::{
    ExcType, MontyException,
    bytecode::{Code, Compiler, FrameExit, VM},
    exception_private::RunResult,
    heap::{DropWithHeap, Heap, HeapReader},
    intern::{InternerBuilder, Interns},
    io::PrintWriter,
    namespace::NamespaceId,
    object::MontyObject,
    parse::{parse, parse_with_interner},
    prepare::{prepare, prepare_with_existing_names},
    resource::{NoLimitTracker, ResourceTracker},
    run_progress::{RunProgress, build_run_progress, check_snapshot_from_converted, convert_frame_exit},
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
/// let runner = MontyRun::new("x + 1".to_owned(), "test.py", vec!["x".to_owned()]).unwrap();
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
    pub fn new(code: String, script_name: &str, input_names: Vec<String>) -> Result<Self, MontyException> {
        Executor::new(code, script_name, input_names).map(|executor| Self { executor })
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
    /// * `print` - print output writer
    pub fn run(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: impl ResourceTracker,
        print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        self.executor.run(inputs, resource_tracker, print)
    }

    /// Executes the code to completion with no resource limits, printing to stdout/stderr.
    pub fn run_no_limits(&self, inputs: Vec<MontyObject>) -> Result<MontyObject, MontyException> {
        self.run(inputs, NoLimitTracker, PrintWriter::Stdout)
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
    /// - `RunProgress::FunctionCall(call)` - external function call, call `call.resume(return_value)` to resume
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
        mut print: PrintWriter<'_>,
    ) -> Result<RunProgress<T>, MontyException> {
        let executor = self.executor;

        // Create heap and VM with empty globals, then populate inputs with VM alive
        let mut heap = Heap::new(executor.namespace_size, resource_tracker);
        let globals = executor.empty_globals();
        let (converted, vm_state) = HeapReader::with(&mut heap, |heap| {
            let mut vm = VM::new(globals, heap, &executor.interns, print.reborrow());
            executor.populate_inputs(inputs, &mut vm)?;

            // Start execution
            let vm_result = vm.run_module(&executor.module_code);

            // Three-phase conversion: convert while VM alive, then snapshot, then build progress
            let converted = convert_frame_exit(vm_result, &mut vm);
            let vm_state = check_snapshot_from_converted(&converted, vm);
            Ok((converted, vm_state))
        })?;
        build_run_progress(converted, vm_state, executor, heap)
    }
}

/// Lower level interface to parse code and run it to completion.
///
/// This is an internal type used by [`MontyRun`]. It stores the compiled bytecode and source code
/// for error reporting. Also used by `run_progress` and `repl` modules.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Executor {
    /// Number of slots needed in the global namespace.
    pub(crate) namespace_size: usize,
    /// Maps variable names to their indices in the namespace.
    ///
    /// Used by:
    /// - ref-count tests for looking up variables by name
    /// - REPL incremental compilation to preserve stable global slot IDs across snippets
    pub(crate) name_map: AHashMap<String, NamespaceId>,
    /// Compiled bytecode for the module.
    pub(crate) module_code: Code,
    /// Interned strings used for looking up names and filenames during execution.
    pub(crate) interns: Interns,
    /// Source code for error reporting (extracting preview lines for tracebacks).
    pub(crate) code: String,
    /// Input variable names that were injected for this snippet.
    ///
    /// Used by the REPL path to look up namespace slots for injected inputs.
    /// Empty for the standard (non-REPL) execution path.
    pub(crate) input_names: Vec<String>,
    /// Estimated heap capacity for pre-allocation on subsequent runs.
    /// Uses AtomicUsize for thread-safety (required by PyO3's Sync bound).
    heap_capacity: AtomicUsize,
}

impl Clone for Executor {
    fn clone(&self) -> Self {
        Self {
            namespace_size: self.namespace_size,
            name_map: self.name_map.clone(),
            module_code: self.module_code.clone(),
            interns: self.interns.clone(),
            code: self.code.clone(),
            input_names: self.input_names.clone(),
            heap_capacity: AtomicUsize::new(self.heap_capacity.load(Ordering::Relaxed)),
        }
    }
}

impl Executor {
    /// Creates a new executor with the given code, filename, and input names.
    pub(crate) fn new(code: String, script_name: &str, input_names: Vec<String>) -> Result<Self, MontyException> {
        let parse_result = parse(&code, script_name).map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare(parse_result, input_names).map_err(|e| e.into_python_exc(script_name, &code))?;

        // Create interns with empty functions (functions will be set after compilation)
        let mut interns = Interns::new(prepared.interner, Vec::new());

        // Compile the module to bytecode, which also compiles all nested functions
        let namespace_size_u16 = u16::try_from(prepared.namespace_size).expect("module namespace size exceeds u16");
        let compile_result = Compiler::compile_module(&prepared.nodes, &interns, namespace_size_u16)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        // Set the compiled functions in the interns
        interns.set_functions(compile_result.functions);

        Ok(Self {
            namespace_size: prepared.namespace_size,
            name_map: prepared.name_map,
            module_code: compile_result.code,
            interns,
            code,
            input_names: Vec::new(),
            heap_capacity: AtomicUsize::new(prepared.namespace_size),
        })
    }

    /// Compiles one REPL snippet against existing session metadata.
    ///
    /// This differs from [`new`](Self::new) in three ways required for true
    /// no-replay REPL execution:
    /// - Seeds parsing from `existing_interns` so old `StringId` values stay stable.
    /// - Seeds compilation with existing functions so old `FunctionId` values remain valid.
    /// - Reuses `existing_name_map` and appends new global names only.
    ///
    /// `input_names` are pre-registered in the name map before preparation so they
    /// receive stable namespace slots that the REPL input-injection logic can use.
    pub(crate) fn new_repl_snippet(
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
            heap_capacity: AtomicUsize::new(0),
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
    /// * `print` - Print output writer
    fn run(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: impl ResourceTracker,
        mut print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        let heap_capacity = self.heap_capacity.load(Ordering::Relaxed);
        let mut heap = Heap::new(heap_capacity, resource_tracker);
        let globals = self.empty_globals();

        // Create VM first, then populate inputs with VM alive
        let result = HeapReader::with(&mut heap, |heap| {
            let mut vm = VM::new(globals, heap, &self.interns, print.reborrow());
            self.populate_inputs(inputs, &mut vm)?;
            self.run_to_completion(&mut vm)
        });

        if heap.size() > heap_capacity {
            self.heap_capacity.store(heap.size(), Ordering::Relaxed);
        }

        // Non-REPL execution has exactly one source, so every frame's filename
        // resolves to the same `self.code`.
        result.map_err(|e| e.into_python_exception(&self.interns, |_| Some(self.code.as_str())))
    }

    /// Runs module code on an already-configured VM to completion.
    ///
    /// Executes [`VM::run_module`], then handles `NameLookup` and `ExternalCall`
    /// exits by raising `NameError` through the VM so tracebacks are properly
    /// captured. Finally converts the result via [`frame_exit_to_object`].
    ///
    /// This is the shared non-iterative execution core used by both the standard
    /// `run` path and the REPL's `feed_run` path.
    pub(crate) fn run_to_completion<'a>(&'a self, vm: &mut VM<'_, 'a, impl ResourceTracker>) -> RunResult<MontyObject> {
        let mut frame_exit_result = vm.run_module(&self.module_code);

        // Handle NameLookup and ExternalCall exits by raising NameError through the VM
        // so that traceback information is properly captured. In the non-iterative path,
        // there's no host to resolve names or external functions, so these become NameErrors.
        loop {
            match frame_exit_result {
                Ok(FrameExit::NameLookup { name_id, .. }) => {
                    let name = self.interns.get_str(name_id);
                    let err = ExcType::name_error(name);
                    frame_exit_result = vm.resume_with_exception(err.into());
                }
                Ok(FrameExit::ExternalCall {
                    function_name,
                    args,
                    name_load_ip,
                    ..
                }) => {
                    // In non-iterative execution, an ExtFunction from LoadGlobalCallable/
                    // LoadLocalCallable means the name was undefined — raise NameError.
                    // Restore the frame IP to the load instruction so the traceback
                    // points to the name reference, not the call expression.
                    if let Some(load_ip) = name_load_ip {
                        vm.set_instruction_ip(load_ip);
                    }
                    let name = function_name.as_str(&self.interns);
                    args.drop_with_heap(vm);
                    let err = ExcType::name_error(name);
                    frame_exit_result = vm.resume_with_exception(err.into());
                }
                _ => break,
            }
        }

        frame_exit_to_object(frame_exit_result, vm)
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
        let globals = self.empty_globals();

        HeapReader::with(&mut heap, |heap| {
            // Create VM, populate inputs, and run
            let mut vm = VM::new(globals, heap, &self.interns, PrintWriter::Stdout);
            self.populate_inputs(inputs, &mut vm)?;
            let frame_exit_result = vm.run_module(&self.module_code);

            // Take globals out of the VM so we can inspect them, but keep VM alive
            // for heap access and later conversion.
            let globals = vm.take_globals();

            // Read refcounts BEFORE converting the return value, because
            // `frame_exit_to_object` drops the return value (decrementing its refcount).
            let mut counts = ahash::AHashMap::new();
            let mut unique_ids = HashSet::new();

            for (name, &namespace_id) in &self.name_map {
                let idx = namespace_id.index();
                if idx < globals.len()
                    && let Value::Ref(id) = &globals[idx]
                {
                    counts.insert(name.clone(), vm.heap.get_refcount(*id));
                    unique_ids.insert(*id);
                }
            }
            let unique_refs = unique_ids.len();
            let heap_count = vm.heap.entry_count();

            // Convert return value while VM is still alive (needs access to interns).
            // Non-REPL: single source, so every frame resolves to `self.code`.
            let py_object = frame_exit_to_object(frame_exit_result, &mut vm)
                .map_err(|e| e.into_python_exception(&self.interns, |_| Some(self.code.as_str())))?;

            // Drop globals with proper ref counting
            for value in globals {
                value.drop_with_heap(vm.heap);
            }

            let allocations_since_gc = vm.heap.get_allocations_since_gc();

            Ok(RefCountOutput {
                py_object,
                counts,
                unique_refs,
                heap_count,
                allocations_since_gc,
            })
        })
    }

    /// Creates an empty globals vector with all slots set to `Undefined`.
    ///
    /// Used to initialize global storage before input population. The VM is created
    /// with these empty globals, then [`populate_inputs`](Self::populate_inputs) fills
    /// the input slots while the VM is alive.
    pub(crate) fn empty_globals(&self) -> Vec<Value> {
        (0..self.namespace_size).map(|_| Value::Undefined).collect()
    }

    /// Converts `MontyObject` inputs to `Value`s and writes them into the VM's globals.
    ///
    /// This runs with the VM alive so that `to_value` has access to the full VM context.
    /// On error partway through, the VM's `Drop` impl will drain globals and
    /// properly decrement refcounts for any already-converted values.
    pub(crate) fn populate_inputs(
        &self,
        inputs: Vec<MontyObject>,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> Result<(), MontyException> {
        if inputs.len() > self.namespace_size {
            return Err(MontyException::runtime_error("too many inputs for namespace"));
        }
        for (i, input) in inputs.into_iter().enumerate() {
            let value = input
                .to_value(vm)
                .map_err(|e| MontyException::runtime_error(format!("invalid input type: {e}")))?;
            vm.globals[i] = value;
        }
        Ok(())
    }
}

/// Converts module/frame exit results into plain `MontyObject` outputs.
///
/// Used by non-iterative execution paths where suspendable outcomes (external calls,
/// name lookups) are not supported and should produce errors.
pub(crate) fn frame_exit_to_object(
    frame_exit_result: RunResult<FrameExit>,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<MontyObject> {
    match frame_exit_result? {
        FrameExit::Return(return_value) => Ok(MontyObject::new(return_value, vm)),
        FrameExit::ExternalCall {
            function_name, args, ..
        } => {
            args.drop_with_heap(vm);
            let function_name = function_name.as_str(vm.interns);
            Err(ExcType::not_implemented(format!(
                "External function '{function_name}' not implemented with standard execution"
            ))
            .into())
        }
        FrameExit::OsCall { function, args, .. } => {
            args.drop_with_heap(vm);
            Err(ExcType::not_implemented(format!(
                "OS function '{function}' not implemented with standard execution"
            ))
            .into())
        }
        FrameExit::MethodCall { method_name, args, .. } => {
            args.drop_with_heap(vm);
            let name = method_name.as_str(vm.interns);
            Err(
                ExcType::not_implemented(format!("Method call '{name}' not implemented with standard execution"))
                    .into(),
            )
        }
        FrameExit::ResolveFutures(_) => {
            Err(ExcType::not_implemented("async futures not supported by standard execution.").into())
        }
        FrameExit::NameLookup { name_id, .. } => {
            let name = vm.interns.get_str(name_id);
            Err(ExcType::name_error(name).into())
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
