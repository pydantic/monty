//! Public interface for running Monty code.
use std::{
    borrow::Cow,
    mem,
    sync::{Arc, LazyLock},
};

use monty_types::{
    AssertMessageAnnotations, ExcType, ExtFunctionResult, MontyException, MontyObject, OsFunctionCall, PrintWriter,
    ResourceTracker,
};
pub use monty_types::{CompileOptions, OsPolicy};
use ruff_python_stdlib::identifiers::is_identifier;

use crate::{
    bytecode::{Code, CodeBuilder, Compiler, FrameExit, Opcode, VM},
    exception_private::{ExcTypeExt, RunError, RunResult},
    heap::{DropWithContext, Heap, HeapReader},
    intern::{CompileInterns, Interns, StringId},
    name_map::NameMap,
    namespace::NamespaceId,
    object_bridge::MontyObjectExt,
    parse::{CodeRange, parse_with_interner, source_nesting_exception},
    prepare::prepare_with_existing_names,
    run_progress::{
        RunProgress, answer_unserved_lookups, build_run_progress, check_snapshot_from_converted, convert_frame_exit,
        resume_with_result,
    },
    source_map::SourceMap,
    types::str::StringRepr,
    value::Value,
    virtual_path::{canonical_cwd, posix_join},
};

/// Primary interface for running Monty code.
///
/// [`MontyRun`] supports two execution modes:
/// - **Simple execution**: Use [`run`](Self::run) or [`run_no_limits`](Self::run_no_limits) to run code to completion
/// - **Iterative execution**: Use [`start`](Self::start) to start execution which will pause at external function calls and
///   can be resumed later
///
/// Deserialization requires trusted, unmodified state; see [`crate::Dump::load`].
///
/// # Example
/// ```
/// use monty::MontyRun;
/// use monty_types::{CompileOptions, MontyObject};
///
/// let mut runner = MontyRun::new(
///     "x + 1".to_owned(),
///     "test.py",
///     vec!["x".to_owned()],
///     CompileOptions::default(),
/// )
/// .unwrap();
/// let result = runner.run_no_limits(vec![MontyObject::int(41)]).unwrap();
/// assert_eq!(result, MontyObject::int(42));
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyRun {
    /// The underlying executor containing compiled bytecode and interns.
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
    /// * `script_name` - The script name for error messages; its final path
    ///   component is what `__file__` places under the working directory
    ///   (`/main.py` for `main.py` or `src/main.py` at the root)
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
        &self.executor.program.code
    }

    /// Replaces the default clock, sleep and random initialization policies.
    /// Applies to [`run`](Self::run) and [`start`](Self::start); `CallHost`
    /// suspends to the host. Wall-clock access exposes timing information;
    /// see `docs/security.md`.
    ///
    /// ```
    /// use monty::MontyRun;
    /// use monty_types::{OsPolicy, CompileOptions, DateTimeSource, MontyObject};
    ///
    /// let code = "from datetime import date\ndate.today().year".to_owned();
    /// let datetime = DateTimeSource::Fixed { unix_seconds: 1_700_000_000, microsecond: 0 };
    /// let calls = OsPolicy { datetime, ..OsPolicy::default() };
    /// let mut runner = MontyRun::new(code, "today.py", vec![], CompileOptions::default()).unwrap().with_os_policy(calls);
    /// assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::int(2023));
    /// ```
    #[must_use]
    pub fn with_os_policy(mut self, os_policy: OsPolicy) -> Self {
        self.executor.program.os_policy = Arc::new(os_policy);
        self
    }

    /// Sets the sandbox working directory the run starts in (default `/`).
    ///
    /// `cwd` is an absolute POSIX virtual path, passed through
    /// [`normalize_virtual_path`](monty_types::normalize_virtual_path) so
    /// `os.getcwd()` reports a canonical directory: it is what relative paths
    /// in `open()` / `os` / `pathlib` calls resolve against before reaching
    /// the host. Hosts typically pass the first mount's virtual path.
    pub fn set_cwd(&mut self, cwd: &str) {
        self.executor.program.cwd = canonical_cwd(cwd);
    }

    /// Executes the code and returns both the result and reference count data, used for testing only.
    #[cfg(feature = "ref-count-return")]
    pub fn run_ref_counts(&mut self, inputs: Vec<MontyObject>) -> Result<RefCountOutput, MontyException> {
        self.executor.run_ref_counts(inputs)
    }

    /// Executes the code and returns reference count data while using a custom tracker, used for testing only.
    #[cfg(feature = "ref-count-return")]
    pub fn run_ref_counts_with_tracker(
        &mut self,
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
    /// Takes `&mut self` because a run extends the compiled program: `eval()` /
    /// `exec()` append the code they compile to the runner's tables.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace
    /// * `resource_tracker` - Custom resource tracker implementation
    /// * `print` - print output writer
    pub fn run(
        &mut self,
        inputs: Vec<MontyObject>,
        resource_tracker: ResourceTracker,
        print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        self.executor.run(inputs, resource_tracker, print)
    }

    /// Executes the code to completion with no resource limits specified (will use the default),
    /// printing to stdout/stderr.
    pub fn run_no_limits(&mut self, inputs: Vec<MontyObject>) -> Result<MontyObject, MontyException> {
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
    /// - An input value is invalid (e.g. a [`MontyObject::repr`] value)
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
        let mut executor = self.executor;

        // Create heap and VM with empty globals, then populate inputs with VM alive
        let mut heap = Heap::new(executor.namespace_size(), resource_tracker);
        let globals = executor.empty_globals();
        let (converted, vm_state) =
            HeapReader::with(&mut heap, &mut (&mut executor, print), |reader, (executor, print)| {
                let mut vm = VM::new(
                    globals,
                    &mut executor.tables,
                    &executor.program,
                    reader,
                    print.reborrow(),
                );
                populate_inputs(inputs, &mut vm)?;

                // Start execution
                let vm_result = vm.run_external();

                // Three-phase conversion: convert while VM alive, then snapshot, then build progress
                let converted = convert_frame_exit(vm_result, &mut vm);
                let vm_state = check_snapshot_from_converted(&converted, vm);
                Ok((converted, vm_state))
            })?;
        build_run_progress(converted, vm_state, executor, heap)
    }
}

/// Compiled program and session tables used by `MontyRun`, `run_progress` and `repl`.
/// The VM borrows the program and committed intern entries immutably, but the
/// global-name map mutably so runtime compilation can add module slots.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Executor {
    /// Compiler tables the run may extend.
    pub(crate) tables: SessionTables,
    /// Everything else a run needs, fixed once compiled.
    pub(crate) program: Program,
    /// Estimated heap capacity for pre-allocation on subsequent runs.
    heap_capacity: usize,
}

/// Session-owned compiler tables, transferred to each snippet's executor.
/// The VM borrows names mutably and committed intern entries immutably.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SessionTables {
    /// Module-level global names, slot by slot.
    pub(crate) global_names: NameMap,
    /// Interned strings and compiled functions used during execution.
    pub(crate) interns: Interns,
}

/// The module code, source and environment, borrowed immutably during execution.
/// Separate from [`SessionTables`] so global names can grow without moving the module's code.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Program {
    /// Immutable module bytecode, shared by runner clones and borrowed by module frames.
    pub(crate) module_code: Arc<Code>,
    /// Source code for error reporting (extracting preview lines for
    /// tracebacks). Shared with the REPL's per-snippet source table rather
    /// than copied, since a snippet's text is the largest thing a feed carries.
    pub(crate) code: Arc<str>,
    /// Pre-resolved input slots, avoiding name lookups during REPL input injection.
    /// Empty for the standard execution path.
    pub(crate) input_slots: Vec<NamespaceId>,
    /// UTF-8 byte cap for each operand repr in introspected assert messages.
    /// Stored with the compiled program and passed to every VM.
    pub(crate) assert_repr_max_bytes: u32,
    /// The options the program was compiled with; `eval()` / `exec()` compile
    /// their snippets the same way.
    pub(crate) options: CompileOptions,
    /// OS-call policies shared with the REPL session; see [`MontyRun::with_os_policy`].
    pub(crate) os_policy: Arc<OsPolicy>,
    /// The user-facing script name (`main.py`), whose final component
    /// `__file__` is derived from. For REPL snippets this is the session's
    /// name (shared with it, not copied per feed), not the `<python-input-N>`
    /// name the snippet was parsed under.
    pub(crate) script_name: Arc<str>,
    /// Sandbox working directory every VM built from this executor starts in;
    /// `/` unless the host set one (see [`MontyRun::set_cwd`]). Shared with
    /// the REPL session like `script_name`.
    pub(crate) cwd: Arc<str>,
}

/// Per-run environment handed to a fresh VM: the sandbox working directory,
/// what `__file__` derives from and the assert-repr cap. Built by
/// [`Program::vm_env`] so every `VM::new` call site agrees on how the
/// values derive from the executor. Borrows rather than clones: a VM is
/// built per run, so this must not allocate.
pub(crate) struct VmEnv<'h> {
    /// Working directory `os.getcwd()` reports and relative paths resolve
    /// against. Borrowed from the executor until `os.chdir` replaces it.
    pub(crate) cwd: Cow<'h, str>,
    /// Working directory the run started in; `__file__` is `script_name`'s
    /// final component placed under it, unaffected by a later `os.chdir`.
    pub(crate) initial_cwd: &'h str,
    /// User-facing script name (`main.py`), the basis of `__file__`.
    pub(crate) script_name: &'h str,
    /// UTF-8 byte cap for each operand repr in introspected assert messages.
    pub(crate) assert_repr_max_bytes: u32,
    /// Compile options for code compiled at runtime by `eval()` / `exec()`.
    pub(crate) options: CompileOptions,
    /// Clock, sleep and random initialization policies.
    pub(crate) os_policy: &'h OsPolicy,
}

impl VmEnv<'_> {
    /// `__file__`: the script name under the starting working directory,
    /// computed on read since most runs never look at it.
    pub(crate) fn file(&self) -> String {
        posix_join(self.initial_cwd, self.script_basename())
    }

    /// The script name as the sandbox sees it — what `sys.argv[0]` reports and
    /// what [`file`](Self::file) places under the working directory.
    ///
    /// Only the final path component is kept because the script name is a
    /// host-side label that may be a host path (`monty /home/me/app.py`), and
    /// host directory structure must not leak into the sandbox.
    pub(crate) fn script_basename(&self) -> &str {
        self.script_name.rsplit(['/', '\\']).next().unwrap_or_default()
    }
}

impl Default for VmEnv<'static> {
    /// The environment of a VM built without an executor (in-module tests):
    /// root working directory, no script.
    fn default() -> Self {
        Self {
            cwd: Cow::Borrowed(DEFAULT_CWD),
            initial_cwd: DEFAULT_CWD,
            script_name: "",
            assert_repr_max_bytes: AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            options: CompileOptions::default(),
            os_policy: &DEFAULT_OS_POLICY,
        }
    }
}

/// Policies for VMs constructed without an executor.
static DEFAULT_OS_POLICY: LazyLock<OsPolicy> = LazyLock::new(OsPolicy::default);

/// The sandbox working directory used until a host sets one.
pub(crate) const DEFAULT_CWD: &str = "/";

/// Session identity a REPL snippet executor inherits: the user-facing script
/// name and the working directory the snippet starts in. Both are shared
/// (`Arc`) so building a snippet executor allocates neither.
#[derive(Clone, Copy)]
pub(crate) struct ReplSession<'a> {
    /// User-facing script name (`main.py`), the basis of `__file__`.
    pub(crate) script_name: &'a Arc<str>,
    /// Absolute virtual working directory for the snippet.
    pub(crate) cwd: &'a Arc<str>,
    /// Clock, sleep and random initialization policies.
    pub(crate) os_policy: &'a Arc<OsPolicy>,
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
        source_nesting_exception(&code, script_name, options.source_scan_threshold)?;
        let mut interns = Interns::new(&code);
        let mut globals = NameMap::new();
        let (module_code, _) = compile_module_source(
            &code,
            script_name,
            &mut globals,
            CompileInterns::direct(&mut interns),
            input_names,
            options,
        )?;
        let namespace_size = globals.len();

        Ok(Self {
            tables: SessionTables {
                global_names: globals,
                interns,
            },
            program: Program {
                module_code: Arc::new(module_code),
                code: Arc::from(code),
                input_slots: Vec::new(),
                assert_repr_max_bytes: options.assert_message_annotations.max_bytes(),
                options,
                os_policy: Arc::new(OsPolicy::default()),
                script_name: Arc::from(script_name),
                cwd: Arc::from(DEFAULT_CWD),
            },
            heap_capacity: namespace_size,
        })
    }

    /// Returns the size of the module's global namespace (number of slots).
    #[inline]
    pub(crate) fn namespace_size(&self) -> usize {
        self.tables.global_names.len()
    }

    /// Compiles privately against the session's existing IDs and global slots.
    /// On success the tables move into the executor; on failure they remain unchanged.
    /// `script_name` identifies this feed's source; `session` supplies the user-facing
    /// filename and working directory.
    pub(crate) fn new_repl_snippet(
        code: Arc<str>,
        script_name: &str,
        globals: &mut NameMap,
        interns: &mut Interns,
        input_names: &[String],
        options: CompileOptions,
        session: ReplSession<'_>,
    ) -> Result<Self, MontyException> {
        check_identifier(input_names)?;

        // Preparation assigns provisional global slots alongside the private intern IDs.
        let globals_len = globals.len();
        let compiled = compile_module_source(
            &code,
            script_name,
            globals,
            CompileInterns::new(interns),
            input_names,
            options,
        );
        if compiled.is_err() {
            globals.truncate(globals_len);
        }
        let (module_code, input_slots) = compiled?;

        Ok(Self {
            tables: SessionTables {
                global_names: mem::take(globals),
                interns: interns.take(),
            },
            program: Program {
                module_code: Arc::new(module_code),
                code,
                input_slots,
                assert_repr_max_bytes: options.assert_message_annotations.max_bytes(),
                options,
                os_policy: Arc::clone(session.os_policy),
                script_name: Arc::clone(session.script_name),
                cwd: Arc::clone(session.cwd),
            },
            heap_capacity: 0,
        })
    }

    /// Builds a synthetic REPL input that calls one existing global with host arguments.
    ///
    /// The argument tuple occupies a namespace slot named `<monty-call-args>`,
    /// which no Python source can spell; the caller commits the map back after
    /// the call (the function may have bound new globals) and clears that slot,
    /// so it is reused by the next call. The session's [`Interns`] are extended
    /// in place (two ids, no parse) and moved into the executor on success; on
    /// failure they stay with the caller.
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
        session: ReplSession<'_>,
    ) -> Result<Self, MontyException> {
        const CALL_ARGS_NAME: &str = "<monty-call-args>";

        let code = if arg_count == 0 {
            format!("{name}()")
        } else {
            format!("{name}(...)")
        };
        let mut overlay = CompileInterns::new(interns);
        let filename = overlay.intern(script_name);
        let range = CodeRange {
            filename,
            start_byte: 0,
            end_byte: u32::try_from(code.len()).unwrap_or(u32::MAX),
        };
        let args_name_id = overlay.intern(CALL_ARGS_NAME);
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

        let module_code = builder.build(&SourceMap::new(&code));
        overlay.commit();
        let tables = SessionTables {
            global_names: existing_globals,
            interns: interns.take(),
        };

        Ok(Self {
            tables,
            program: Program {
                module_code: Arc::new(module_code),
                code: Arc::from(code),
                input_slots: vec![args_slot],
                assert_repr_max_bytes: options.assert_message_annotations.max_bytes(),
                options,
                os_policy: Arc::clone(session.os_policy),
                script_name: Arc::clone(session.script_name),
                cwd: Arc::clone(session.cwd),
            },
            heap_capacity: 0,
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
        &mut self,
        inputs: Vec<MontyObject>,
        resource_tracker: ResourceTracker,
        print: PrintWriter<'_>,
    ) -> Result<MontyObject, MontyException> {
        let heap_capacity = self.heap_capacity;
        let mut heap = Heap::new(heap_capacity, resource_tracker);
        let globals = self.empty_globals();

        // Create VM first, then populate inputs with VM alive
        let result = HeapReader::with(&mut heap, &mut (&mut *self, print), |reader, (executor, print)| {
            let mut vm = VM::new(
                globals,
                &mut executor.tables,
                &executor.program,
                reader,
                print.reborrow(),
            );
            populate_inputs(inputs, &mut vm)?;
            Program::run_to_completion(&mut vm)
        });

        if heap.size() > heap_capacity {
            self.heap_capacity = heap.size();
        }

        // Non-REPL execution has exactly one source, so every frame's filename
        // resolves to the same `self.code`.
        result.map_err(|e| e.into_python_exception(&self.tables.interns, |_| Some(&*self.program.code)))
    }

    /// Executes the code and returns both the result and reference count data, used for testing only.
    #[cfg(feature = "ref-count-return")]
    fn run_ref_counts(&mut self, inputs: Vec<MontyObject>) -> Result<RefCountOutput, MontyException> {
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
        &mut self,
        inputs: Vec<MontyObject>,
        resource_tracker: ResourceTracker,
    ) -> Result<RefCountOutput, MontyException> {
        let mut heap = Heap::new(self.namespace_size(), resource_tracker);
        let globals = self.empty_globals();

        HeapReader::with(&mut heap, &mut &mut *self, |reader, executor| {
            // Create VM, populate inputs, and run
            let mut vm = VM::new(
                globals,
                &mut executor.tables,
                &executor.program,
                reader,
                PrintWriter::Stdout,
            );
            populate_inputs(inputs, &mut vm)?;
            // Lookups are answered before the globals are taken below: an
            // armed `hasattr()` / `getattr()` effect runs the module on.
            let frame_exit_result = answer_unserved_lookups(vm.run_external(), &mut vm);

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

            for (namespace_id, name_id) in vm.global_names.iter() {
                let idx = namespace_id.index();
                if idx < globals.len()
                    && let Value::Ref(id) = &globals[idx]
                {
                    counts.insert(vm.interns.get_str(name_id).to_owned(), vm.heap.get_refcount(*id));
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
                .map(|(id, ty)| format!("{} (id {})", ty.name(vm.heap, vm.interns), id.index()))
                .collect();
            let heap_count = vm.heap.entry_count();

            // Convert return value while VM is still alive (needs access to interns).
            // Non-REPL: single source, so every frame resolves to `executor.code`.
            let value = frame_exit_to_object(frame_exit_result, &mut vm)
                .map_err(|e| e.into_python_exception(vm.interns, |_| Some(&*executor.program.code)))?;

            // Drop globals with proper ref counting
            globals.drop_with(vm.heap);

            let allocations_since_gc = vm.heap.get_allocations_since_gc();

            Ok(RefCountOutput {
                value,
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
    /// with these empty globals, then [`populate_inputs`] fills
    /// the input slots while the VM is alive.
    pub(crate) fn empty_globals(&self) -> Vec<Value> {
        (0..self.namespace_size()).map(|_| Value::Undefined).collect()
    }
}

impl Program {
    /// Builds the [`VmEnv`] a VM run from this program starts with.
    pub(crate) fn vm_env(&self) -> VmEnv<'_> {
        VmEnv {
            cwd: Cow::Borrowed(&self.cwd),
            initial_cwd: &self.cwd,
            script_name: &self.script_name,
            assert_repr_max_bytes: self.assert_repr_max_bytes,
            options: self.options,
            os_policy: &self.os_policy,
        }
    }

    /// A program with no code, for in-module tests that need a VM.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            module_code: Arc::new(Code::empty()),
            code: Arc::from(""),
            input_slots: Vec::new(),
            assert_repr_max_bytes: AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            options: CompileOptions::default(),
            os_policy: Arc::new(OsPolicy::default()),
            script_name: Arc::from(""),
            cwd: Arc::from(DEFAULT_CWD),
        }
    }

    /// Runs module code on an already-configured VM to completion.
    ///
    /// Executes [`VM::run_external`], then answers the lookup and `ExternalCall`
    /// exits no host will serve by raising `NameError` / `AttributeError`
    /// through the VM so tracebacks are properly captured. Finally converts
    /// the result via [`frame_exit_to_object`].
    ///
    /// This is the shared non-iterative execution core used by both the standard
    /// `run` path and the REPL's `feed_run` path.
    pub(crate) fn run_to_completion(vm: &mut VM<'_>) -> RunResult<MontyObject> {
        let mut frame_exit_result = vm.run_external();

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
                    let err = ExcType::name_error(function_name.as_str(vm.interns));
                    args.drop_with(vm);
                    frame_exit_result = vm.resume_with_exception(err.into());
                }
                // Standard execution waits inline, excluding sleep from execution time.
                Ok(FrameExit::OsCall {
                    function_call: OsFunctionCall::SystemSleep(delay) | OsFunctionCall::AsyncSystemSleep(delay),
                    effect,
                    ..
                }) => {
                    vm.pending_effect = effect;
                    vm.heap.tracker.sandbox_sleep(delay);
                    frame_exit_result = resume_with_result(vm, ExtFunctionResult::Return(MontyObject::none()), None);
                }
                other => return frame_exit_to_object(other, vm),
            }
        }
    }
}

/// Converts `MontyObject` inputs to `Value`s and writes them into the VM's globals.
///
/// This runs with the VM alive so that `to_value` has access to the full VM context.
/// On error partway through, the VM's `Drop` impl will drain globals and
/// properly decrement refcounts for any already-converted values.
fn populate_inputs(inputs: Vec<MontyObject>, vm: &mut VM<'_>) -> Result<(), MontyException> {
    if inputs.len() > vm.globals.len() {
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

/// Converts module/frame exit results into exported `MontyObject` outputs.
///
/// Used by non-iterative execution paths: lookups are answered as no host
/// would (see [`answer_unserved_lookups`]) and the remaining suspendable
/// outcomes (external calls, futures) produce errors.
pub(crate) fn frame_exit_to_object(frame_exit_result: RunResult<FrameExit>, vm: &mut VM<'_>) -> RunResult<MontyObject> {
    // Suspensions this path cannot service. The error is built from a borrow
    // so one `drop_with` releases whatever the exit owns, fields added later
    // included.
    let exit = match answer_unserved_lookups(frame_exit_result, vm)? {
        FrameExit::Return(return_value) => return Ok(MontyObject::export(return_value, vm)),
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
    pub value: MontyObject,
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

/// Compiles module source through the supplied tables, committing any overlay on success.
/// On failure the caller restores provisional global slots or discards a fresh program's tables.
/// The source must already have passed the nesting scan; `MontyRun::new` and
/// the REPL feeds run it.
fn compile_module_source(
    code: &str,
    script_name: &str,
    globals: &mut NameMap,
    mut interns: CompileInterns<'_>,
    input_names: impl IntoIterator<Item = impl AsRef<str>>,
    options: CompileOptions,
) -> Result<(Code, Vec<NamespaceId>), MontyException> {
    let input_names = input_names.into_iter();
    let mut input_slots = Vec::with_capacity(input_names.size_hint().0);
    for name in input_names {
        let name_id = interns.intern(name.as_ref());
        let slot = globals
            .ensure_slot(name_id, CodeRange::default())
            .map_err(|e| e.into_python_exc(script_name, code))?;
        input_slots.push(slot);
    }
    let nodes =
        parse_with_interner(code, script_name, &mut interns).map_err(|e| e.into_python_exc(script_name, code))?;
    let nodes =
        prepare_with_existing_names(nodes, &interns, globals).map_err(|e| e.into_python_exc(script_name, code))?;
    let module_code = Compiler::compile_module(&nodes, code, &mut interns, globals, options)
        .map_err(|e| e.into_python_exc(script_name, code))?;
    interns.commit();
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
