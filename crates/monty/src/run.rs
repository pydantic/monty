//! Public interface for running Monty code.
use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

pub use monty_types::CompileOptions;
use monty_types::{ExcType, MontyException, MontyObject, PrintWriter, ResourceTracker};
use ruff_python_stdlib::identifiers::is_identifier;

use crate::{
    bytecode::{Code, CodeBuilder, Compiler, FrameExit, Opcode, VM},
    exception_private::{ExcTypeExt, RunError, RunResult},
    function::Function,
    heap::{DropWithContext, Heap, HeapReader},
    intern::{InternerBuilder, Interns, StringId},
    name_map::NameMap,
    namespace::NamespaceId,
    object_bridge::MontyObjectExt,
    parse::{CodeRange, parse, parse_with_interner},
    prepare::{prepare, prepare_with_existing_names},
    run_progress::{
        RunProgress, answer_unserved_lookups, build_run_progress, check_snapshot_from_converted, convert_frame_exit,
    },
    types::str::StringRepr,
    value::Value,
};

/// Primary interface for running Monty code.
///
/// [`MontyRun`] supports two execution modes:
/// - **Simple execution**: Use [`run`](Self::run) or [`run_no_limits`](Self::run_no_limits) to run code to completion
/// - **Iterative execution**: Use [`start`](Self::start) to start execution which will pause at external function calls and
///   can be resumed later
///
/// # Example
/// ```
/// use monty::MontyRun;
/// use monty_types::{CompileOptions, MontyObject};
///
/// let runner = MontyRun::new(
///     "x + 1".to_owned(),
///     "test.py",
///     vec!["x".to_owned()],
///     CompileOptions::default(),
/// )
/// .unwrap();
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
    /// Call [`run`](Self::run) or [`start`](Self::start) with inputs to execute it.
    ///
    /// # Arguments
    /// * `code` - The Python code to execute
    /// * `script_name` - The script name for error messages
    /// * `input_names` - Names of input variables
    /// * `options` - [`CompileOptions`] controlling CPython divergences; usually `CompileOptions::default()`
    ///
    /// # Errors
    /// Returns [`MontyException`] if the code cannot be parsed.
    pub fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        options: CompileOptions,
    ) -> Result<Self, MontyException> {
        Executor::new(code, script_name, input_names, options).map(|executor| Self { executor })
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

    /// Executes the code and returns reference count data while using a custom tracker, used for testing only.
    #[cfg(feature = "ref-count-return")]
    pub fn run_ref_counts_with_tracker(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: ResourceTracker,
    ) -> Result<RefCountOutput, MontyException> {
        self.executor.run_ref_counts_with_tracker(inputs, resource_tracker)
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
        resource_tracker: ResourceTracker,
        print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        self.executor.run(inputs, resource_tracker, print)
    }

    /// Executes the code to completion with no resource limits specified (will use the default),
    /// printing to stdout/stderr.
    pub fn run_no_limits(&self, inputs: Vec<MontyObject>) -> Result<MontyObject, MontyException> {
        self.run(inputs, ResourceTracker::default(), PrintWriter::Stdout)
    }

    /// Starts execution with the given inputs and resource tracker, consuming self.
    ///
    /// Creates the heap and namespaces, then begins execution.
    ///
    /// For iterative execution, [`start`](Self::start) consumes self and returns a [`RunProgress`]:
    /// - [`RunProgress::FunctionCall`] - external function call, call [`FunctionCall::resume`](crate::FunctionCall::resume) to resume
    /// - [`RunProgress::Complete`] - execution finished
    ///
    /// This enables snapshotting execution state and returning control to the host
    /// application during long-running computations.
    ///
    /// # Arguments
    /// * `inputs` - Initial input values (must match length of `input_names` from [`new`](Self::new))
    /// * `resource_tracker` - Resource tracker for the execution
    /// * `print` - Writer for print output
    ///
    /// # Errors
    /// Returns [`MontyException`] if:
    /// - The number of inputs doesn't match the expected count
    /// - An input value is invalid (e.g., [`MontyObject::Repr`])
    /// - A runtime error occurs during execution
    ///
    /// # Panics
    /// This method should not panic under normal operation. Internal assertions
    /// may panic if the VM reaches an inconsistent state (indicating a bug).
    pub fn start(
        self,
        inputs: Vec<MontyObject>,
        resource_tracker: ResourceTracker,
        print: PrintWriter<'_>,
    ) -> Result<RunProgress, MontyException> {
        let executor = self.executor;

        // Create heap and VM with empty globals, then populate inputs with VM alive
        let mut heap = Heap::new(executor.namespace_size(), resource_tracker);
        let globals = executor.empty_globals();
        let (converted, vm_state) =
            HeapReader::with(&mut heap, &mut (&executor, print), |reader, (executor, print)| {
                let mut vm = VM::new(
                    globals,
                    &executor.module_code,
                    reader,
                    &executor.interns,
                    print.reborrow(),
                    executor.assert_repr_max_bytes,
                );
                executor.populate_inputs(inputs, &mut vm)?;

                // Start execution
                let vm_result = vm.run_module();

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
    /// Module-level global names.
    pub(crate) globals: NameMap,
    /// Compiled bytecode for the module. Wrapped in `Arc` to avoid needing to deep clone.
    pub(crate) module_code: Arc<Code>,
    /// Interned strings used for looking up names and filenames during execution.
    pub(crate) interns: Interns,
    /// Source code for error reporting (extracting preview lines for tracebacks).
    pub(crate) code: String,
    /// Namespace slots that the REPL input-injection path writes into.
    ///
    /// Pre-resolved at snippet-construction time so the per-call hot path
    /// (`inject_inputs_into_vm`) is an O(1) slot index instead of an
    /// O(N-interns) `Interns::get_string_id_by_name` lookup per input.
    /// One entry per input value, in the order the embedder passed them.
    /// Empty for the standard (non-REPL) execution path.
    pub(crate) input_slots: Vec<NamespaceId>,
    /// UTF-8 byte cap for each operand repr in introspected assert messages.
    /// Stored with the compiled program and passed to every VM.
    pub(crate) assert_repr_max_bytes: u32,
    /// Estimated heap capacity for pre-allocation on subsequent runs.
    /// Uses AtomicUsize for thread-safety (required by PyO3's Sync bound).
    heap_capacity: AtomicUsize,
}

impl Clone for Executor {
    fn clone(&self) -> Self {
        Self {
            globals: self.globals.clone(),
            module_code: self.module_code.clone(),
            interns: self.interns.clone(),
            code: self.code.clone(),
            input_slots: self.input_slots.clone(),
            assert_repr_max_bytes: self.assert_repr_max_bytes,
            heap_capacity: AtomicUsize::new(self.heap_capacity.load(Ordering::Relaxed)),
        }
    }
}

impl Executor {
    /// Creates a new executor with the given code, filename, input names, and compile options.
    pub(crate) fn new(
        code: String,
        script_name: &str,
        input_names: Vec<String>,
        options: CompileOptions,
    ) -> Result<Self, MontyException> {
        check_identifier(&input_names)?;
        let parse_result = parse(&code, script_name).map_err(|e| e.into_python_exc(script_name, &code))?;
        let prepared = prepare(parse_result, input_names).map_err(|e| e.into_python_exc(script_name, &code))?;

        // Compile the module to bytecode, which also compiles all nested functions.
        // The compiler enforces the bytecode-format namespace-size limit and reports
        // it as a `SyntaxError` rather than panicking on the `u16` cast.
        let namespace_size = prepared.globals.len();
        let mut functions = Vec::new();
        let module_code = Compiler::compile_module(
            &prepared.nodes,
            &prepared.interner,
            &prepared.globals,
            &mut functions,
            options,
        )
        .map_err(|e| e.into_python_exc(script_name, &code))?;

        Ok(Self {
            globals: prepared.globals,
            module_code: Arc::new(module_code),
            interns: Interns::new(prepared.interner, functions),
            code,
            input_slots: Vec::new(),
            assert_repr_max_bytes: options.assert_message_annotations.max_bytes(),
            heap_capacity: AtomicUsize::new(namespace_size),
        })
    }

    /// Returns the size of the module's global namespace (number of slots).
    #[inline]
    pub(crate) fn namespace_size(&self) -> usize {
        self.globals.len()
    }

    /// Compiles one REPL snippet against the session's compiler tables.
    ///
    /// This differs from [`new`](Self::new) in that it *extends* the session's
    /// `NameMap` and [`Interns`] rather than building fresh ones, so old
    /// `StringId`/`FunctionId` values and global slots stay stable and the
    /// snippet runs without replaying earlier code.
    ///
    /// The tables are moved into the returned executor (nothing is cloned — this
    /// is what keeps feed cost independent of session size) and must be handed
    /// back to the session once the snippet is finished with. On failure they
    /// are left in place: the name slots and functions the rejected snippet
    /// appended are rolled back so they can't eat into the `u16` id spaces,
    /// while its interned strings stay (u32 ids, harmless and stable).
    ///
    /// `input_names` are pre-registered in the globals map before preparation so
    /// they receive stable namespace slots that the REPL input-injection logic
    /// can use.
    pub(crate) fn new_repl_snippet(
        code: String,
        script_name: &str,
        globals: &mut NameMap,
        interns: &mut Interns,
        input_names: &[String],
        options: CompileOptions,
    ) -> Result<Self, MontyException> {
        check_identifier(input_names)?;

        let globals_len = globals.len();
        let (mut interner, mut functions) = mem::take(interns).into_builder();
        let compiled = compile_repl_snippet(
            &code,
            script_name,
            globals,
            &mut interner,
            &mut functions,
            input_names,
            options,
        );
        // Whether or not compilation succeeded, the extended tables are the
        // session's tables from here on (`compile_module` has already rolled
        // back `functions` on failure).
        *interns = Interns::new(interner, functions);
        if compiled.is_err() {
            globals.truncate(globals_len);
        }
        let (module_code, input_slots) = compiled?;

        Ok(Self {
            globals: mem::take(globals),
            module_code: Arc::new(module_code),
            interns: mem::take(interns),
            code,
            input_slots,
            assert_repr_max_bytes: options.assert_message_annotations.max_bytes(),
            heap_capacity: AtomicUsize::new(0),
        })
    }

    /// Builds a synthetic REPL input that calls one existing global with host arguments.
    ///
    /// The argument tuple occupies a temporary namespace slot whose name mapping
    /// must not be committed, so `existing_globals` is a throwaway copy. The
    /// session's [`Interns`] are extended in place (two ids, no parse) and moved
    /// into the executor on success; on failure they stay with the caller.
    #[expect(
        clippy::too_many_arguments,
        reason = "synthetic calls combine existing REPL and call-site metadata"
    )]
    pub(crate) fn new_repl_function_call(
        name: &str,
        name_id: StringId,
        callable_slot: NamespaceId,
        arg_count: usize,
        script_name: &str,
        mut existing_globals: NameMap,
        interns: &mut Interns,
        options: CompileOptions,
    ) -> Result<Self, MontyException> {
        const CALL_ARGS_NAME: &str = "<monty-call-args>";

        let code = if arg_count == 0 {
            format!("{name}()")
        } else {
            format!("{name}(...)")
        };
        let filename = interns.intern(script_name);
        let range = CodeRange {
            filename,
            start_byte: 0,
            end_byte: u32::try_from(code.len()).unwrap_or(u32::MAX),
        };
        let args_name_id = interns.intern(CALL_ARGS_NAME);
        let args_slot = existing_globals
            .ensure_slot(args_name_id, range)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        let mut builder = CodeBuilder::new();
        builder.new_code_region(0);
        builder.set_location(range, None);
        builder
            .emit_load_global_callable(callable_slot.as_u16(), name_id)
            .map_err(|e| e.into_python_exc(script_name, &code))?;
        builder
            .emit_u16(Opcode::LoadGlobal, args_slot.as_u16())
            .map_err(|e| e.into_python_exc(script_name, &code))?;
        builder
            .emit_u8(Opcode::CallFunctionExtended, 0)
            .map_err(|e| e.into_python_exc(script_name, &code))?;
        builder
            .emit(Opcode::ReturnValue)
            .map_err(|e| e.into_python_exc(script_name, &code))?;

        Ok(Self {
            globals: existing_globals,
            module_code: Arc::new(builder.build(0)),
            interns: mem::take(interns),
            code,
            input_slots: vec![args_slot],
            assert_repr_max_bytes: options.assert_message_annotations.max_bytes(),
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
        resource_tracker: ResourceTracker,
        print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        let heap_capacity = self.heap_capacity.load(Ordering::Relaxed);
        let mut heap = Heap::new(heap_capacity, resource_tracker);
        let globals = self.empty_globals();

        // Create VM first, then populate inputs with VM alive
        let result = HeapReader::with(&mut heap, &mut (self, print), |reader, (executor, print)| {
            let mut vm = VM::new(
                globals,
                &executor.module_code,
                reader,
                &executor.interns,
                print.reborrow(),
                executor.assert_repr_max_bytes,
            );
            executor.populate_inputs(inputs, &mut vm)?;
            executor.run_to_completion(&mut vm)
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
    /// Executes [`VM::run_module`], then answers the lookup and `ExternalCall`
    /// exits no host will serve by raising `NameError` / `AttributeError`
    /// through the VM so tracebacks are properly captured. Finally converts
    /// the result via [`frame_exit_to_object`].
    ///
    /// This is the shared non-iterative execution core used by both the standard
    /// `run` path and the REPL's `feed_run` path.
    pub(crate) fn run_to_completion<'h>(&'h self, vm: &mut VM<'h>) -> RunResult<MontyObject> {
        let mut frame_exit_result = vm.run_module();

        // In the non-iterative path there's no host to resolve names, lazy
        // attributes or external functions, so lookups are answered `Undefined`
        // and a called external function is an undefined name.
        loop {
            match answer_unserved_lookups(frame_exit_result, vm) {
                Ok(FrameExit::ExternalCall {
                    function_name,
                    args,
                    name_load_ip,
                    ..
                }) => {
                    // In non-iterative execution, an ExtFunction from LoadGlobalCallable
                    // means the name was undefined — raise NameError.
                    // Restore the frame IP to the load instruction so the traceback
                    // points to the name reference, not the call expression.
                    if let Some(load_ip) = name_load_ip {
                        vm.set_instruction_ip(load_ip);
                    }
                    let name = function_name.as_str(&self.interns);
                    args.drop_with(vm);
                    let err = ExcType::name_error(name);
                    frame_exit_result = vm.resume_with_exception(err.into());
                }
                other => return frame_exit_to_object(other, vm),
            }
        }
    }

    /// Executes the code and returns both the result and reference count data, used for testing only.
    #[cfg(feature = "ref-count-return")]
    fn run_ref_counts(&self, inputs: Vec<MontyObject>) -> Result<RefCountOutput, MontyException> {
        self.run_ref_counts_with_tracker(inputs, ResourceTracker::default())
    }

    /// Executes the code and returns both the result and reference count data with a custom tracker,
    /// used for testing only.
    ///
    /// This is used for testing reference counting behavior with a custom tracker. Returns
    /// the execution result plus, in [`RefCountOutput`], a map from variable names to their
    /// reference counts (heap-allocated values only), any live-but-unreachable heap entries,
    /// and the total live heap population.
    ///
    /// For strict-matching validation, assert that `unreachable` is empty: every live heap
    /// object should be reachable from a named variable, so anything left over is a leak.
    ///
    /// Only available when the `ref-count-return` feature is enabled.
    #[cfg(feature = "ref-count-return")]
    fn run_ref_counts_with_tracker(
        &self,
        inputs: Vec<MontyObject>,
        resource_tracker: ResourceTracker,
    ) -> Result<RefCountOutput, MontyException> {
        let mut heap = Heap::new(self.namespace_size(), resource_tracker);
        let globals = self.empty_globals();

        HeapReader::with(&mut heap, &mut &*self, |reader, executor| {
            // Create VM, populate inputs, and run
            let mut vm = VM::new(
                globals,
                &executor.module_code,
                reader,
                &executor.interns,
                PrintWriter::Stdout,
                executor.assert_repr_max_bytes,
            );
            executor.populate_inputs(inputs, &mut vm)?;
            // Lookups are answered before the globals are taken below: an
            // armed `hasattr()` / `getattr()` effect runs the module on.
            let frame_exit_result = answer_unserved_lookups(vm.run_module(), &mut vm);

            // Tasks the module left running (a sibling detached from a failed
            // gather, say) hold real references, and are not reachable from
            // any name — so tear the scheduler down first and hold the
            // leak check to what survives that.
            vm.__finalize_tasks_for_tests();
            vm.__force_gc_for_tests();

            // Take globals out of the VM so we can inspect them, but keep VM alive
            // for heap access and later conversion.
            let globals = vm.take_globals();

            // Read refcounts BEFORE converting the return value, because
            // `frame_exit_to_object` drops the return value (decrementing its refcount).
            let mut counts = ahash::AHashMap::new();
            let mut roots = Vec::new();

            for (namespace_id, name_id) in executor.globals.iter() {
                let idx = namespace_id.index();
                if idx < globals.len()
                    && let Value::Ref(id) = &globals[idx]
                {
                    counts.insert(executor.interns.get_str(name_id).to_owned(), vm.heap.get_refcount(*id));
                    roots.push(*id);
                }
            }
            // The module's result is a root too: it is still owned by the pending
            // `FrameExit::Return` here, since `frame_exit_to_object` below is what drops it.
            if let Ok(FrameExit::Return(Value::Ref(id))) = &frame_exit_result {
                roots.push(*id);
            }
            // Those are the only roots: locals are gone once the module frame exits, so
            // anything still live must hang off a name or the result to not be a leak.
            let unreachable: Vec<String> = vm
                .heap
                .unreachable_entries(roots)
                .into_iter()
                .map(|(id, ty)| format!("{} (id {})", ty.name(vm.heap, &executor.interns), id.index()))
                .collect();
            let heap_count = vm.heap.entry_count();

            // Convert return value while VM is still alive (needs access to interns).
            // Non-REPL: single source, so every frame resolves to `executor.code`.
            let py_object = frame_exit_to_object(frame_exit_result, &mut vm)
                .map_err(|e| e.into_python_exception(&executor.interns, |_| Some(executor.code.as_str())))?;

            // Drop globals with proper ref counting
            globals.drop_with(vm.heap);

            let allocations_since_gc = vm.heap.get_allocations_since_gc();

            Ok(RefCountOutput {
                py_object,
                counts,
                unreachable,
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
        (0..self.namespace_size()).map(|_| Value::Undefined).collect()
    }

    /// Converts `MontyObject` inputs to `Value`s and writes them into the VM's globals.
    ///
    /// This runs with the VM alive so that `to_value` has access to the full VM context.
    /// On error partway through, the VM's `Drop` impl will drain globals and
    /// properly decrement refcounts for any already-converted values.
    pub(crate) fn populate_inputs(&self, inputs: Vec<MontyObject>, vm: &mut VM<'_>) -> Result<(), MontyException> {
        if inputs.len() > self.namespace_size() {
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
/// Used by non-iterative execution paths: lookups are answered as no host
/// would (see [`answer_unserved_lookups`]) and the remaining suspendable
/// outcomes (external calls, futures) produce errors.
pub(crate) fn frame_exit_to_object(frame_exit_result: RunResult<FrameExit>, vm: &mut VM<'_>) -> RunResult<MontyObject> {
    // Suspensions this path cannot service. The error is built from a borrow
    // so one `drop_with` releases whatever the exit owns, fields added later
    // included.
    let exit = match answer_unserved_lookups(frame_exit_result, vm)? {
        FrameExit::Return(return_value) => return Ok(MontyObject::new(return_value, vm)),
        exit => exit,
    };
    let error: RunError = match &exit {
        FrameExit::Return(_) => unreachable!("returns are handled above"),
        FrameExit::ExternalCall { function_name, .. } => {
            let function_name = function_name.as_str(vm.interns);
            ExcType::not_implemented(format!(
                "External function '{function_name}' not implemented with standard execution"
            ))
            .into()
        }
        FrameExit::OsCall { function_call, .. } => ExcType::not_implemented(format!(
            "OS function '{}' not implemented with standard execution",
            function_call.name()
        ))
        .into(),
        FrameExit::MethodCall { method_name, .. } => {
            let name = method_name.as_str(vm.interns);
            ExcType::not_implemented(format!("Method call '{name}' not implemented with standard execution")).into()
        }
        FrameExit::ResolveFutures(_) => {
            ExcType::not_implemented("async futures not supported by standard execution.").into()
        }
        FrameExit::NameLookup { .. } | FrameExit::AttrLookup { .. } => unreachable!("lookups are answered above"),
    };
    exit.drop_with(vm);
    Err(error)
}

/// Output from `run_ref_counts` containing reference count and heap information.
///
/// Used for testing GC behavior and reference counting correctness.
#[cfg(feature = "ref-count-return")]
#[derive(Debug)]
pub struct RefCountOutput {
    pub py_object: MontyObject,
    pub counts: ahash::AHashMap<String, usize>,
    /// Live heap entries reachable from no named variable, described as
    /// `"<type> (id N)"`. Non-empty means the run leaked: a missed `drop_with`
    /// left an object alive that nothing can reach. Reachability is transitive,
    /// so objects owned by another object are accounted for by their owner.
    pub unreachable: Vec<String>,
    pub heap_count: usize,
    /// Number of GC-tracked allocations since the last cycle collection.
    ///
    /// If the collector ran during execution, this will be much lower than
    /// the total number of GC-tracked allocations performed. Compare against
    /// the configured `gc_interval` to verify GC fired at the expected
    /// cadence.
    pub allocations_since_gc: u32,
}

/// Parse → prepare → compile pipeline for one REPL snippet, extending the
/// session tables in place.
///
/// Split out of [`Executor::new_repl_snippet`] so every stage works on borrowed
/// tables and any `?` early-return leaves them with the caller. Returns the
/// module code and the namespace slot of each input, in order.
fn compile_repl_snippet(
    code: &str,
    script_name: &str,
    globals: &mut NameMap,
    interner: &mut InternerBuilder,
    functions: &mut Vec<Function>,
    input_names: &[String],
    options: CompileOptions,
) -> Result<(Code, Vec<NamespaceId>), MontyException> {
    // Pre-register input names so they get stable slots before preparation,
    // and capture each input's slot index so injection doesn't have to do a
    // name→StringId lookup at call time (one slot per input value, in order).
    //
    // Surfaced via the standard parse/prepare error path; if the embedder
    // hands over more than `u16::MAX + 1` names the bytecode encoding can't
    // represent them all.
    let mut input_slots = Vec::with_capacity(input_names.len());
    for name in input_names {
        let name_id = interner.intern(name);
        let slot = globals
            .ensure_slot(name_id, CodeRange::default())
            .map_err(|e| e.into_python_exc(script_name, code))?;
        input_slots.push(slot);
    }

    let nodes = parse_with_interner(code, script_name, interner).map_err(|e| e.into_python_exc(script_name, code))?;
    let nodes =
        prepare_with_existing_names(nodes, interner, globals).map_err(|e| e.into_python_exc(script_name, code))?;
    let module_code = Compiler::compile_module(&nodes, interner, globals, functions, options)
        .map_err(|e| e.into_python_exc(script_name, code))?;
    Ok((module_code, input_slots))
}

/// Check if input names are valid Python identifiers.
///
/// `is_identifier` also checks that the names are not keywords.
fn check_identifier(input_names: &[String]) -> Result<(), MontyException> {
    for name in input_names {
        if !is_identifier(name) {
            return Err(MontyException::new(
                ExcType::SyntaxError,
                Some(format!("Input name {} not a valid identifier", StringRepr(name))),
            ));
        }
    }
    Ok(())
}
