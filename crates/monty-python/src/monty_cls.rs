use std::{
    fmt::Write,
    mem,
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{
    ExcType, ExtFunctionResult, FunctionCall, JsonMontyArray, JsonMontyObject, JsonMontyPairs, LimitedTracker,
    MontyException, MontyObject, MontyRun, NameLookupResult, NoLimitTracker, OsCall, ReplFunctionCall, ReplNameLookup,
    ReplOsCall, ReplProgress, ReplResolveFutures, ReplStartError, ResolveFutures, ResourceTracker, RunProgress,
};
use monty::{NameLookup, fs::MountTable};
use monty_type_checking::{SourceFile, type_check};
use pyo3::{
    CastIntoError, IntoPyObjectExt, PyTypeCheck,
    exceptions::{PyBaseException, PyKeyError, PyRuntimeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyTuple, PyType},
};
use pyo3_async_runtimes::tokio::future_into_py;

use crate::{
    async_dispatch::{await_run_transition, dispatch_loop_run},
    convert::{get_docstring, monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    exceptions::{MontyError, MontyTypingError, exc_py_to_monty},
    external::{ExternalFunctionRegistry, dispatch_method_call},
    limits::{CancellationFlag, FutureCancellationGuard, PySignalTracker, extract_limits},
    mount::OsHandler,
    print_target::PrintTarget,
    repl::{EitherRepl, FromCoreRepl, PyMontyRepl, drive_repl_progress_through_os_calls},
    serialization,
};

/// A sandboxed Python interpreter instance.
///
/// Parses and compiles Python code on initialization, then can be run
/// multiple times with different input values. This separates the parsing
/// cost from execution, making repeated runs more efficient.
#[pyclass(name = "Monty", module = "pydantic_monty")]
#[derive(Debug)]
pub struct PyMonty {
    /// The compiled code snapshot, ready to execute.
    runner: MontyRun,
    /// The artificial name of the python code "file"
    script_name: String,
    /// Names of input variables expected by the code.
    input_names: Vec<String>,
    /// Registry of dataclass types for reconstructing original types on output.
    ///
    /// Maps type pointer identity (`u64`) to the original Python type, allowing
    /// `isinstance(result, OriginalClass)` to work correctly after round-tripping through Monty.
    dc_registry: DcRegistry,
}

#[pymethods]
impl PyMonty {
    /// Creates a new Monty interpreter by parsing the given code.
    ///
    /// # Arguments
    /// * `code` - Python code to execute
    /// * `inputs` - List of input variable names available in the code
    /// * `type_check` - Whether to perform type checking on the code
    /// * `type_check_stubs` - Prefix code to be executed before type checking
    /// * `dataclass_registry` - Registry of dataclass types for reconstructing original types on output.
    #[new]
    #[pyo3(signature = (code, *, script_name="main.py", inputs=None, type_check=false, type_check_stubs=None, dataclass_registry=None))]
    fn new(
        py: Python<'_>,
        code: String,
        script_name: &str,
        inputs: Option<&Bound<'_, PyList>>,
        type_check: bool,
        type_check_stubs: Option<&str>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        let input_names = list_str(inputs, "inputs")?;

        if type_check {
            py_type_check(py, &code, script_name, type_check_stubs, "type_stubs.pyi")?;
        }

        // Create the snapshot (parses the code)
        let runner = MontyRun::new(code, script_name, input_names.clone()).map_err(|e| MontyError::new_err(py, e))?;

        Ok(Self {
            runner,
            script_name: script_name.to_string(),
            input_names,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
        })
    }

    /// Registers a dataclass type for proper isinstance() support on output.
    ///
    /// When a dataclass passes through Monty and is returned, it becomes a `MontyDataclass`.
    /// By registering the original type, `isinstance(result, OriginalClass)` will return `True`.
    ///
    /// # Arguments
    /// * `cls` - The dataclass type to register
    ///
    /// # Raises
    /// * `TypeError` if the argument is not a dataclass type
    fn register_dataclass(&self, cls: &Bound<'_, PyType>) -> PyResult<()> {
        self.dc_registry.insert(cls)
    }

    /// Performs static type checking on the code.
    ///
    /// Analyzes the code for type errors without executing it. This uses
    /// a subset of Python's type system supported by Monty.
    ///
    /// # Args
    /// * `prefix_code` - Optional prefix to prepend to the code before type checking,
    ///   e.g. with inputs and external function signatures
    ///
    /// # Raises
    /// * `RuntimeError` if type checking infrastructure fails
    /// * `MontyTypingError` if type errors are found
    #[pyo3(signature = (prefix_code=None))]
    fn type_check(&self, py: Python<'_>, prefix_code: Option<&str>) -> PyResult<()> {
        py_type_check(py, self.runner.code(), &self.script_name, prefix_code, "type_stubs.pyi")
    }

    /// Executes the code and returns the result.
    ///
    /// # Returns
    /// The result of the last expression in the code.
    ///
    /// # Raises
    /// Various Python exceptions matching what the code would raise
    #[expect(clippy::too_many_arguments)]
    #[pyo3(signature = (*, inputs=None, limits=None, external_functions=None, print_callback=None, mount=None, os=None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        inputs: Option<&Bound<'_, PyDict>>,
        limits: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Clone the Arc handle — all clones share the same underlying registry,
        // so auto-registrations during execution are visible to all users.
        let input_values = self.extract_input_values(inputs, &self.dc_registry)?;

        // Build the internal mount table from mount + os parameters.
        let os_handler = OsHandler::from_run_args(py, mount, os)?;

        // Resolve the print target from the Python argument once; the
        // resulting value is threaded through the VM call chain so collector
        // objects keep accumulating across transitions.
        let print_target = PrintTarget::from_py(print_callback)?;

        // Run with appropriate tracker type (must branch due to different generic types)
        if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            self.run_impl(py, input_values, tracker, external_functions, os_handler, print_target)
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            self.run_impl(py, input_values, tracker, external_functions, os_handler, print_target)
        }
    }

    /// Starts code execution, returning a progress snapshot or the final result.
    ///
    /// When `mount` or `os` is provided, OS calls are resolved automatically via
    /// the same logic as [`Monty::run`] (mount table first, then the Python
    /// callback), and the method only returns a snapshot when a non-OS event is
    /// reached (external function, name lookup, future, or completion).
    ///
    /// The auto-dispatch does **not** persist across subsequent `snapshot.resume()`
    /// calls — once a snapshot is returned, any OS call produced by a later resume
    /// surfaces as a `FunctionSnapshot` with `is_os_function=True`, as before.
    #[pyo3(signature = (*, inputs=None, limits=None, print_callback=None, mount=None, os=None))]
    fn start<'py>(
        &self,
        py: Python<'py>,
        inputs: Option<&Bound<'py, PyDict>>,
        limits: Option<&Bound<'py, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Clone the Arc handle — shares the same underlying registry
        let dc_registry = self.dc_registry.clone_ref(py);
        let input_values = self.extract_input_values(inputs, &dc_registry)?;

        let print_target = PrintTarget::from_py(print_callback)?;

        // Validate mount + os and build the handler BEFORE taking mounts or
        // starting the VM, so validation errors don't leave any state taken.
        let os_handler = OsHandler::from_run_args(py, mount, os)?;

        let runner = self.runner.clone();

        // Each `start` transition builds its own writer via `with_writer`, so
        // any collector buffer is only locked for the duration of
        // `runner.start`.
        macro_rules! start_impl {
            ($tracker:expr) => {{
                let progress = match py
                    .detach(|| print_target.with_writer(|writer| runner.start(input_values, $tracker, writer)))
                {
                    Ok(p) => p,
                    Err(e) => {
                        return Err(MontyError::new_err(py, e));
                    }
                };
                // When mount/os is configured, consume OS-call events internally
                // until we reach the first non-OS event. Mounts are taken inside
                // the helper and put back on every exit path.
                if let Some(handler) = &os_handler {
                    drive_run_progress_through_os_calls(py, progress, handler, &print_target, &self.dc_registry)?
                } else {
                    progress
                }
            }};
        }

        // Branch on limits (different generic types)
        let progress = if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            EitherProgress::Limited(start_impl!(tracker))
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            EitherProgress::NoLimit(start_impl!(tracker))
        };
        progress.progress_or_complete(py, self.script_name.clone(), print_target, dc_registry)
    }

    /// Runs the code asynchronously, supporting async external functions.
    ///
    /// Returns a Python awaitable that drives the async dispatch loop.
    /// Unlike `run()`, this handles external functions that return coroutines
    /// by awaiting them on the Python event loop. VM resume calls are offloaded
    /// to a thread pool via `spawn_blocking` to avoid blocking the event loop.
    ///
    /// # Returns
    /// A Python coroutine that resolves to the result of the last expression.
    ///
    /// # Raises
    /// Various Python exceptions matching what the code would raise.
    #[pyo3(signature = (*, inputs=None, limits=None, external_functions=None, print_callback=None, os=None))]
    fn run_async<'py>(
        &self,
        py: Python<'py>,
        inputs: Option<&Bound<'_, PyDict>>,
        limits: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        os: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Some(ref os_cb) = os
            && !os_cb.bind(py).is_callable()
        {
            let msg = format!(
                "TypeError: '{}' object is not callable",
                os_cb.bind(py).get_type().name()?
            );
            return Err(PyTypeError::new_err(msg));
        }

        let input_values = self.extract_input_values(inputs, &self.dc_registry)?;
        let limits = limits.map(extract_limits).transpose()?;
        let dc_registry = self.dc_registry.clone_ref(py);
        let ext_fns = external_functions.map(|d| d.clone().unbind());
        let print_target = PrintTarget::from_py(print_callback)?;
        let runner = self.runner.clone();
        if let Some(limits) = limits {
            Self::run_async_with_tracker(
                py,
                runner,
                input_values,
                ext_fns,
                os,
                dc_registry,
                print_target,
                move |cancel_flag| PySignalTracker::new_with_cancellation(LimitedTracker::new(limits), cancel_flag),
            )
        } else {
            Self::run_async_with_tracker(
                py,
                runner,
                input_values,
                ext_fns,
                os,
                dc_registry,
                print_target,
                move |cancel_flag| PySignalTracker::new_with_cancellation(NoLimitTracker, cancel_flag),
            )
        }
    }

    /// Serializes the Monty instance to a binary format.
    ///
    /// The serialized data can be stored and later restored with `Monty.load()`.
    /// This allows caching parsed code to avoid re-parsing on subsequent runs.
    ///
    /// # Returns
    /// Bytes containing the serialized Monty instance.
    ///
    /// # Raises
    /// `ValueError` if serialization fails.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let serialized = SerializedMonty {
            runner: self.runner.clone(),
            script_name: self.script_name.clone(),
            input_names: self.input_names.clone(),
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Deserializes a Monty instance from binary format.
    ///
    /// # Arguments
    /// * `data` - The serialized Monty data from `dump()`
    /// * `dataclass_registry` - Optional list of dataclasses to register
    ///
    /// # Returns
    /// A new Monty instance.
    ///
    /// # Raises
    /// `ValueError` if deserialization fails.
    #[staticmethod]
    #[pyo3(signature = (data, *, dataclass_registry=None))]
    fn load(
        py: Python<'_>,
        data: &Bound<'_, PyBytes>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        let bytes = data.as_bytes();
        let serialized: SerializedMonty =
            postcard::from_bytes(bytes).map_err(|e| PyValueError::new_err(e.to_string()))?;

        Ok(Self {
            runner: serialized.runner,
            script_name: serialized.script_name,
            input_names: serialized.input_names,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
        })
    }

    fn __repr__(&self) -> String {
        let lines = self.runner.code().lines().count();
        let mut s = format!(
            "Monty(<{} line{} of code>, script_name='{}'",
            lines,
            if lines == 1 { "" } else { "s" },
            self.script_name
        );
        if !self.input_names.is_empty() {
            write!(s, ", inputs={:?}", self.input_names).unwrap();
        }
        s.push(')');
        s
    }
}

impl PyMonty {
    /// Creates the Python awaitable for `run_async()` using a concrete tracker type.
    ///
    /// The tracker builder receives a per-await cancellation flag that is flipped
    /// when the Python task drops the underlying Rust future. The resulting tracker
    /// observes that flag via `check_time()` and aborts active VM execution.
    #[expect(clippy::too_many_arguments)]
    fn run_async_with_tracker<T, F>(
        py: Python<'_>,
        runner: MontyRun,
        input_values: Vec<MontyObject>,
        external_functions: Option<Py<PyDict>>,
        os: Option<Py<PyAny>>,
        dc_registry: DcRegistry,
        print_target: PrintTarget,
        tracker_builder: F,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        T: ResourceTracker + Send + 'static,
        F: FnOnce(CancellationFlag) -> PySignalTracker<T> + Send + 'static,
    {
        future_into_py(py, async move {
            let cancellation_flag = Arc::new(AtomicBool::new(false));
            let mut cancellation_guard = FutureCancellationGuard::new(cancellation_flag.clone());
            // Give the initial `start()` its own handle to the print target so the
            // ongoing buffer keeps accumulating once the dispatch loop takes over
            // ownership of `print_target`.
            let start_target = print_target.clone_handle_detached();
            let tracker = tracker_builder(cancellation_flag);

            let progress = await_run_transition(move || {
                start_target.with_writer(|writer| runner.start(input_values, tracker, writer))
            })
            .await?
            .map_err(|e| Python::attach(|py| MontyError::new_err(py, e)))?;

            let result = dispatch_loop_run(progress, external_functions, os, dc_registry, print_target).await;
            cancellation_guard.disarm();
            result
        })
    }
}

pub(crate) fn py_type_check(
    py: Python<'_>,
    code: &str,
    script_name: &str,
    type_stubs: Option<&str>,
    stubs_name: &str,
) -> PyResult<()> {
    let type_stubs = type_stubs.map(|type_stubs| SourceFile::new(type_stubs, stubs_name));

    let opt_diagnostics =
        type_check(&SourceFile::new(code, script_name), type_stubs.as_ref()).map_err(PyRuntimeError::new_err)?;

    if let Some(diagnostic) = opt_diagnostics {
        Err(MontyTypingError::new_err(py, diagnostic))
    } else {
        Ok(())
    }
}

impl PyMonty {
    /// Extracts input values from a Python dict in the order they were declared.
    ///
    /// Validates that all required inputs are provided. Any dataclass inputs are
    /// automatically registered in `dc_registry` via `py_to_monty` so they can be
    /// properly reconstructed on output.
    fn extract_input_values(
        &self,
        inputs: Option<&Bound<'_, PyDict>>,
        dc_registry: &DcRegistry,
    ) -> PyResult<Vec<::monty::MontyObject>> {
        if self.input_names.is_empty() {
            if inputs.is_some() {
                return Err(PyTypeError::new_err(
                    "No input variables declared but inputs dict was provided",
                ));
            }
            return Ok(vec![]);
        }

        let Some(inputs) = inputs else {
            return Err(PyTypeError::new_err(format!(
                "Missing required inputs: {:?}",
                self.input_names
            )));
        };

        // Extract values in declaration order
        self.input_names
            .iter()
            .map(|name| {
                let value = inputs
                    .get_item(name)?
                    .ok_or_else(|| PyKeyError::new_err(format!("Missing required input: '{name}'")))?;
                py_to_monty(&value, dc_registry)
            })
            .collect::<PyResult<_>>()
    }

    /// Runs code with a generic resource tracker, releasing the GIL during execution.
    ///
    /// Takes explicit field references instead of `&mut self` so that `run()` can
    /// remain `&self` (required for concurrent thread access in PyO3).
    #[expect(clippy::needless_pass_by_value)]
    fn run_impl<'py>(
        &self,
        py: Python<'py>,
        input_values: Vec<MontyObject>,
        tracker: impl ResourceTracker + Send,
        external_functions: Option<&Bound<'_, PyDict>>,
        os_handler: Option<OsHandler>,
        print_target: PrintTarget,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Each VM transition builds its own `PrintWriter` via
        // `print_target.with_writer`, which only holds any collector lock for
        // the duration of that transition.

        // Check if any inputs contain dataclasses (including nested in containers) —
        // if so, we need the iterative path because method calls could happen lazily
        // and need to be dispatched to the host.
        let has_dataclass_inputs = || input_values.iter().any(contains_dataclass);

        if external_functions.is_none() && os_handler.is_none() && !has_dataclass_inputs() {
            let result =
                py.detach(|| print_target.with_writer(|writer| self.runner.run(input_values, tracker, writer)));
            return match result {
                Ok(v) => monty_to_py(py, &v, &self.dc_registry).map(|obj| obj.into_bound(py)),
                Err(err) => Err(MontyError::new_err(py, err)),
            };
        }

        // Take mounts out of their shared slots for zero-overhead execution.
        // They are put back when the run completes (including on error paths).
        let mut mount_table: Option<MountTable> = os_handler.as_ref().map(OsHandler::take).transpose()?;

        // Helper: put mounts back into shared slots.
        let put_back = |table: Option<MountTable>| {
            if let (Some(h), Some(table)) = (&os_handler, table) {
                h.put_back(table);
            }
        };

        let to_err = |py: Python<'_>, e| MontyError::new_err(py, e);

        // Clone the runner since start() consumes it - allows reuse of the parsed code
        let runner = self.runner.clone();
        let mut progress =
            match py.detach(|| print_target.with_writer(|writer| runner.start(input_values, tracker, writer))) {
                Ok(p) => p,
                Err(e) => {
                    put_back(mount_table);
                    return Err(to_err(py, e));
                }
            };

        loop {
            match progress {
                RunProgress::Complete(result) => {
                    put_back(mount_table);
                    return monty_to_py(py, &result, &self.dc_registry).map(|obj| obj.into_bound(py));
                }
                RunProgress::FunctionCall(call) => {
                    // Dataclass method calls have method_call=true and the first arg is the instance
                    let return_value = if call.method_call {
                        dispatch_method_call(py, &call.function_name, &call.args, &call.kwargs, &self.dc_registry)
                    } else if let Some(ext_fns) = external_functions {
                        let registry = ExternalFunctionRegistry::new(py, ext_fns, &self.dc_registry);
                        registry.call(&call.function_name, &call.args, &call.kwargs)
                    } else {
                        put_back(mount_table);
                        return Err(PyRuntimeError::new_err(format!(
                            "External function '{}' called but no external_functions provided",
                            call.function_name
                        )));
                    };

                    progress = match py.detach(|| print_target.with_writer(|writer| call.resume(return_value, writer)))
                    {
                        Ok(p) => p,
                        Err(e) => {
                            put_back(mount_table);
                            return Err(to_err(py, e));
                        }
                    };
                }
                RunProgress::NameLookup(lookup) => {
                    let result = if let Some(ext_fns) = external_functions
                        && let Some(value) = ext_fns.get_item(&lookup.name)?
                    {
                        NameLookupResult::Value(MontyObject::Function {
                            name: lookup.name.clone(),
                            docstring: get_docstring(&value),
                        })
                    } else {
                        NameLookupResult::Undefined
                    };

                    progress = match py.detach(|| print_target.with_writer(|writer| lookup.resume(result, writer))) {
                        Ok(p) => p,
                        Err(e) => {
                            put_back(mount_table);
                            return Err(to_err(py, e));
                        }
                    };
                }
                RunProgress::ResolveFutures(_) => {
                    put_back(mount_table);
                    return Err(PyRuntimeError::new_err("async futures not supported with `Monty.run`"));
                }
                RunProgress::OsCall(call) => {
                    let fallback = os_handler.as_ref().and_then(|h| h.fallback.as_ref());
                    // `handle_mount_os_call` can fail during Python⇄Monty conversion;
                    // put mounts back before propagating so the `MountDir` slot doesn't
                    // get permanently stuck in the "in use" state.
                    let result: ExtFunctionResult = if let Some(table) = &mut mount_table {
                        match handle_mount_os_call(py, &call, table, fallback, &self.dc_registry) {
                            Ok(r) => r,
                            Err(e) => {
                                put_back(mount_table);
                                return Err(e);
                            }
                        }
                    } else {
                        call.function.on_no_handler(&call.args).into()
                    };

                    progress = match py.detach(|| print_target.with_writer(|writer| call.resume(result, writer))) {
                        Ok(p) => p,
                        Err(e) => {
                            put_back(mount_table);
                            return Err(to_err(py, e));
                        }
                    };
                }
            }
        }
    }
}

/// pyclass doesn't support generic types, hence hard coding the generics
#[derive(Debug)]
pub(crate) enum EitherProgress {
    NoLimit(RunProgress<PySignalTracker<NoLimitTracker>>),
    Limited(RunProgress<PySignalTracker<LimitedTracker>>),
    /// REPL progress with back-reference to the owning `PyMontyRepl` for auto-restore.
    ReplNoLimit(ReplProgress<PySignalTracker<NoLimitTracker>>, Py<PyMontyRepl>),
    /// REPL progress with back-reference to the owning `PyMontyRepl` for auto-restore.
    ReplLimited(ReplProgress<PySignalTracker<LimitedTracker>>, Py<PyMontyRepl>),
}

impl EitherProgress {
    /// Auto-dispatches OS-call events on the wrapped progress until a non-OS
    /// event is reached.
    ///
    /// Callers that pass `mount` or `os` to `Monty.start`, `MontyRepl.feed_start`,
    /// or any of the `snapshot.resume(...)` methods use this to continue running
    /// the VM past filesystem / OS operations without yielding control back to
    /// Python. The underlying per-progress helpers ([`drive_run_progress_through_os_calls`]
    /// and [`drive_repl_progress_through_os_calls`]) handle the
    /// mount take/put-back lifecycle and, for the REPL case, rollback of REPL
    /// state on resume errors.
    pub(crate) fn drive_through_os_calls(
        self,
        py: Python<'_>,
        handler: &OsHandler,
        print_target: &PrintTarget,
        dc_registry: &DcRegistry,
    ) -> PyResult<Self> {
        match self {
            Self::NoLimit(p) => Ok(Self::NoLimit(drive_run_progress_through_os_calls(
                py,
                p,
                handler,
                print_target,
                dc_registry,
            )?)),
            Self::Limited(p) => Ok(Self::Limited(drive_run_progress_through_os_calls(
                py,
                p,
                handler,
                print_target,
                dc_registry,
            )?)),
            Self::ReplNoLimit(p, owner) => {
                let next = {
                    let this = owner.get();
                    drive_repl_progress_through_os_calls(py, p, handler, print_target, dc_registry, this)?
                };
                Ok(Self::ReplNoLimit(next, owner))
            }
            Self::ReplLimited(p, owner) => {
                let next = {
                    let this = owner.get();
                    drive_repl_progress_through_os_calls(py, p, handler, print_target, dc_registry, this)?
                };
                Ok(Self::ReplLimited(next, owner))
            }
        }
    }

    /// Converts progress into the appropriate Python object:
    /// function snapshot, name lookup snapshot, future snapshot, or complete.
    pub(crate) fn progress_or_complete(
        self,
        py: Python<'_>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>> {
        match self {
            Self::NoLimit(p) => run_progress_to_py(py, p, script_name, print_callback, dc_registry),
            Self::Limited(p) => run_progress_to_py(py, p, script_name, print_callback, dc_registry),
            Self::ReplNoLimit(p, owner) => repl_progress_to_py(py, p, script_name, print_callback, dc_registry, owner),
            Self::ReplLimited(p, owner) => repl_progress_to_py(py, p, script_name, print_callback, dc_registry, owner),
        }
    }
}

/// Converts a `RunProgress<T>` into the appropriate Python snapshot type.
fn run_progress_to_py<T: ResourceTracker>(
    py: Python<'_>,
    progress: RunProgress<T>,
    script_name: String,
    print_callback: PrintTarget,
    dc_registry: DcRegistry,
) -> PyResult<Bound<'_, PyAny>>
where
    EitherFunctionSnapshot: FromFunctionCall<T> + FromOsCall<T>,
    EitherLookupSnapshot: FromNameLookup<T>,
    EitherFutureSnapshot: FromResolveFutures<T>,
{
    match progress {
        RunProgress::Complete(result) => PyMontyComplete::create(py, result, &dc_registry),
        RunProgress::FunctionCall(call) => {
            PyFunctionSnapshot::function_call(py, call, script_name, print_callback, dc_registry)
        }
        RunProgress::OsCall(call) => PyFunctionSnapshot::os_call(py, call, script_name, print_callback, dc_registry),
        RunProgress::ResolveFutures(state) => {
            PyFutureSnapshot::new_py_any(py, state, script_name, print_callback, dc_registry)
        }
        RunProgress::NameLookup(lookup) => {
            PyNameLookupSnapshot::new_py_any(py, lookup, script_name, print_callback, dc_registry)
        }
    }
}

/// Converts a `ReplProgress<T>` into the appropriate Python snapshot type.
///
/// On completion, restores the REPL state into `repl_owner` before returning `MontyComplete`.
/// The `repl_owner` is propagated into snapshot enum variants so the chain can continue.
fn repl_progress_to_py<T: ResourceTracker>(
    py: Python<'_>,
    progress: ReplProgress<T>,
    script_name: String,
    print_callback: PrintTarget,
    dc_registry: DcRegistry,
    repl_owner: Py<PyMontyRepl>,
) -> PyResult<Bound<'_, PyAny>>
where
    EitherFunctionSnapshot: FromReplFunctionCall<T> + FromReplOsCall<T>,
    EitherLookupSnapshot: FromReplNameLookup<T>,
    EitherFutureSnapshot: FromReplResolveFutures<T>,
    EitherRepl: FromCoreRepl<T>,
{
    match progress {
        ReplProgress::Complete { repl, value } => {
            repl_owner.get().put_repl_after_commit(EitherRepl::from_core(repl));
            PyMontyComplete::create(py, value, &dc_registry)
        }
        ReplProgress::FunctionCall(call) => {
            PyFunctionSnapshot::repl_function_call(py, call, script_name, print_callback, dc_registry, repl_owner)
        }
        ReplProgress::OsCall(call) => {
            PyFunctionSnapshot::repl_os_call(py, call, script_name, print_callback, dc_registry, repl_owner)
        }
        ReplProgress::NameLookup(lookup) => {
            let variable_name = lookup.name.clone();
            PyNameLookupSnapshot::repl_name_lookup(
                py,
                lookup,
                script_name,
                print_callback,
                dc_registry,
                repl_owner,
                variable_name,
            )
        }
        ReplProgress::ResolveFutures(state) => {
            PyFutureSnapshot::repl_resolve_futures(py, state, script_name, print_callback, dc_registry, repl_owner)
        }
    }
}

/// Runtime execution snapshot, holds either a `FunctionCall` or `OsCall` for both
/// resource tracker variants since pyclass structs can't be generic.
///
/// Also holds REPL variants (`ReplFunctionCall`, `ReplOsCall`) for `MontyRepl.feed_start()`.
/// REPL variants carry a `Py<PyMontyRepl>` back-reference so the REPL can be auto-restored
/// on completion or error.
///
/// Used internally by `PyFunctionSnapshot` to store execution state. Both `FunctionCall`
/// and `OsCall` have the same `resume()` signature, so we dispatch to the appropriate
/// inner type based on the variant.
///
/// The `Done` variant indicates the snapshot has been consumed.
///
/// Serde: REPL variants serialize as their non-REPL counterparts (stripping the owner).
/// Deserialization always produces non-REPL variants.
#[derive(Debug)]
pub(crate) enum EitherFunctionSnapshot {
    // Run variants (from Monty.start())
    NoLimitFn(FunctionCall<PySignalTracker<NoLimitTracker>>),
    NoLimitOs(OsCall<PySignalTracker<NoLimitTracker>>),
    LimitedFn(FunctionCall<PySignalTracker<LimitedTracker>>),
    LimitedOs(OsCall<PySignalTracker<LimitedTracker>>),
    // REPL variants (from MontyRepl.feed_start()) — carry the REPL owner
    ReplNoLimitFn(ReplFunctionCall<PySignalTracker<NoLimitTracker>>, Py<PyMontyRepl>),
    ReplNoLimitOs(ReplOsCall<PySignalTracker<NoLimitTracker>>, Py<PyMontyRepl>),
    ReplLimitedFn(ReplFunctionCall<PySignalTracker<LimitedTracker>>, Py<PyMontyRepl>),
    ReplLimitedOs(ReplOsCall<PySignalTracker<LimitedTracker>>, Py<PyMontyRepl>),
    /// Sentinel indicating the snapshot has been consumed via `resume()`.
    Done,
}

/// Helper trait for wrapping `FunctionCall<T>` into `EitherFunctionSnapshot`.
trait FromFunctionCall<T: ResourceTracker> {
    /// Wraps a function call into the appropriate variant.
    fn from_fn(call: FunctionCall<T>) -> Self;
}

impl FromFunctionCall<PySignalTracker<NoLimitTracker>> for EitherFunctionSnapshot {
    fn from_fn(call: FunctionCall<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimitFn(call)
    }
}

impl FromFunctionCall<PySignalTracker<LimitedTracker>> for EitherFunctionSnapshot {
    fn from_fn(call: FunctionCall<PySignalTracker<LimitedTracker>>) -> Self {
        Self::LimitedFn(call)
    }
}

/// Helper trait for wrapping `OsCall<T>` into `EitherFunctionSnapshot`.
trait FromOsCall<T: ResourceTracker> {
    /// Wraps an OS call into the appropriate variant.
    fn from_os(call: OsCall<T>) -> Self;
}

impl FromOsCall<PySignalTracker<NoLimitTracker>> for EitherFunctionSnapshot {
    fn from_os(call: OsCall<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimitOs(call)
    }
}

impl FromOsCall<PySignalTracker<LimitedTracker>> for EitherFunctionSnapshot {
    fn from_os(call: OsCall<PySignalTracker<LimitedTracker>>) -> Self {
        Self::LimitedOs(call)
    }
}

/// Helper trait for wrapping `ReplFunctionCall<T>` into `EitherFunctionSnapshot`.
trait FromReplFunctionCall<T: ResourceTracker> {
    /// Wraps a REPL function call into the appropriate variant.
    fn from_repl_fn(call: ReplFunctionCall<T>, owner: Py<PyMontyRepl>) -> Self;
}

impl FromReplFunctionCall<PySignalTracker<NoLimitTracker>> for EitherFunctionSnapshot {
    fn from_repl_fn(call: ReplFunctionCall<PySignalTracker<NoLimitTracker>>, owner: Py<PyMontyRepl>) -> Self {
        Self::ReplNoLimitFn(call, owner)
    }
}

impl FromReplFunctionCall<PySignalTracker<LimitedTracker>> for EitherFunctionSnapshot {
    fn from_repl_fn(call: ReplFunctionCall<PySignalTracker<LimitedTracker>>, owner: Py<PyMontyRepl>) -> Self {
        Self::ReplLimitedFn(call, owner)
    }
}

/// Helper trait for wrapping `ReplOsCall<T>` into `EitherFunctionSnapshot`.
trait FromReplOsCall<T: ResourceTracker> {
    /// Wraps a REPL OS call into the appropriate variant.
    fn from_repl_os(call: ReplOsCall<T>, owner: Py<PyMontyRepl>) -> Self;
}

impl FromReplOsCall<PySignalTracker<NoLimitTracker>> for EitherFunctionSnapshot {
    fn from_repl_os(call: ReplOsCall<PySignalTracker<NoLimitTracker>>, owner: Py<PyMontyRepl>) -> Self {
        Self::ReplNoLimitOs(call, owner)
    }
}

impl FromReplOsCall<PySignalTracker<LimitedTracker>> for EitherFunctionSnapshot {
    fn from_repl_os(call: ReplOsCall<PySignalTracker<LimitedTracker>>, owner: Py<PyMontyRepl>) -> Self {
        Self::ReplLimitedOs(call, owner)
    }
}

/// Snapshot generated during execution when monty yields to the host for a function call.
#[pyclass(name = "FunctionSnapshot", module = "pydantic_monty")]
#[derive(Debug)]
pub struct PyFunctionSnapshot {
    snapshot: Mutex<EitherFunctionSnapshot>,
    print_callback: PrintTarget,
    dc_registry: DcRegistry,

    /// Name of the script being executed
    #[pyo3(get)]
    pub script_name: String,

    /// Whether this call refers to an OS function
    #[pyo3(get)]
    pub is_os_function: bool,

    /// Whether this call is a dataclass method call (first arg is `self`)
    #[pyo3(get)]
    pub is_method_call: bool,

    /// The name of the function being called.
    #[pyo3(get)]
    pub function_name: String,
    /// The unique identifier for this call
    #[pyo3(get)]
    pub call_id: u32,

    /// Positional args in Monty's native representation. Python callers see
    /// them as a `tuple` via the `args` getter, which converts on each
    /// access (mirroring `MontyComplete.output`); `args_json()` serializes
    /// straight from this vec without a Python round-trip.
    args: Vec<MontyObject>,
    /// Keyword args as (key, value) pairs — same rationale as `args`;
    /// exposed as a Python `dict` via the `kwargs` getter.
    kwargs: Vec<(MontyObject, MontyObject)>,
}

impl PyFunctionSnapshot {
    /// Creates a `PyFunctionSnapshot` for an external function call.
    ///
    /// Extracts display fields from the `FunctionCall` before moving it into
    /// `EitherSnapshot` via the provided `wrap` closure.
    fn function_call<T: ResourceTracker>(
        py: Python<'_>,
        call: FunctionCall<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherFunctionSnapshot: FromFunctionCall<T>,
    {
        let function_name = call.function_name.clone();
        let call_id = call.call_id;
        let method_call = call.method_call;
        let args = call.args.clone();
        let kwargs = call.kwargs.clone();

        let slf = Self {
            snapshot: Mutex::new(EitherFunctionSnapshot::from_fn(call)),
            print_callback,
            script_name,
            is_os_function: false,
            is_method_call: method_call,
            function_name,
            call_id,
            dc_registry,
            args,
            kwargs,
        };
        slf.into_bound_py_any(py)
    }

    /// Creates a `PyFunctionSnapshot` for an OS-level call.
    ///
    /// Extracts display fields from the `OsCall` before moving it into
    /// `EitherSnapshot` via the provided `wrap` closure.
    fn os_call<T: ResourceTracker>(
        py: Python<'_>,
        call: OsCall<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherFunctionSnapshot: FromOsCall<T>,
    {
        let function_name = call.function.to_string();
        let call_id = call.call_id;
        let args = call.args.clone();
        let kwargs = call.kwargs.clone();

        let slf = Self {
            snapshot: Mutex::new(EitherFunctionSnapshot::from_os(call)),
            print_callback,
            script_name,
            is_os_function: true,
            is_method_call: false,
            function_name,
            call_id,
            dc_registry,
            args,
            kwargs,
        };
        slf.into_bound_py_any(py)
    }

    /// Creates a `PyFunctionSnapshot` for a REPL external function call.
    fn repl_function_call<T: ResourceTracker>(
        py: Python<'_>,
        call: ReplFunctionCall<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
        repl_owner: Py<PyMontyRepl>,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherFunctionSnapshot: FromReplFunctionCall<T>,
    {
        let function_name = call.function_name.clone();
        let call_id = call.call_id;
        let method_call = call.method_call;
        let args = call.args.clone();
        let kwargs = call.kwargs.clone();

        let slf = Self {
            snapshot: Mutex::new(EitherFunctionSnapshot::from_repl_fn(call, repl_owner)),
            print_callback,
            script_name,
            is_os_function: false,
            is_method_call: method_call,
            function_name,
            call_id,
            dc_registry,
            args,
            kwargs,
        };
        slf.into_bound_py_any(py)
    }

    /// Creates a `PyFunctionSnapshot` for a REPL OS-level call.
    fn repl_os_call<T: ResourceTracker>(
        py: Python<'_>,
        call: ReplOsCall<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
        repl_owner: Py<PyMontyRepl>,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherFunctionSnapshot: FromReplOsCall<T>,
    {
        let function_name = call.function.to_string();
        let call_id = call.call_id;
        let args = call.args.clone();
        let kwargs = call.kwargs.clone();

        let slf = Self {
            snapshot: Mutex::new(EitherFunctionSnapshot::from_repl_os(call, repl_owner)),
            print_callback,
            script_name,
            is_os_function: true,
            is_method_call: false,
            function_name,
            call_id,
            dc_registry,
            args,
            kwargs,
        };
        slf.into_bound_py_any(py)
    }

    /// Constructs a `PyFunctionSnapshot` from deserialized parts.
    ///
    /// Used by `load_snapshot` and `load_repl_snapshot` to reconstruct snapshot objects.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn from_deserialized(
        py: Python<'_>,
        snapshot: EitherFunctionSnapshot,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
        script_name: String,
        is_os_function: bool,
        is_method_call: bool,
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
    ) -> PyResult<Bound<'_, PyAny>> {
        let slf = Self {
            snapshot: Mutex::new(snapshot),
            print_callback,
            dc_registry,
            script_name,
            is_os_function,
            is_method_call,
            function_name,
            args,
            kwargs,
            call_id,
        };
        slf.into_bound_py_any(py)
    }

    /// Resumes a consumed snapshot with a precomputed external result.
    ///
    /// Both `resume()` and `resume_not_handled()` funnel through this helper so
    /// OS and REPL snapshots share identical state-restoration behavior.
    ///
    /// When `os_handler` is `Some`, the resumed progress is driven through any
    /// pending OS-call events before being converted to a Python snapshot, so
    /// callers who pass `mount=`/`os=` to `resume()` get the same auto-dispatch
    /// semantics as `Monty.start(mount=..., os=...)`.
    fn resume_with_result<'py>(
        &self,
        py: Python<'py>,
        snapshot: EitherFunctionSnapshot,
        external_result: ExtFunctionResult,
        os_handler: Option<&OsHandler>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let to_err = |py: Python<'_>, e| MontyError::new_err(py, e);

        let progress = match snapshot {
            EitherFunctionSnapshot::NoLimitFn(call) => {
                let result = py.detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)));
                EitherProgress::NoLimit(result.map_err(|e| to_err(py, e))?)
            }
            EitherFunctionSnapshot::NoLimitOs(call) => {
                let result = py.detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)));
                EitherProgress::NoLimit(result.map_err(|e| to_err(py, e))?)
            }
            EitherFunctionSnapshot::LimitedFn(call) => {
                let result = py.detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)));
                EitherProgress::Limited(result.map_err(|e| to_err(py, e))?)
            }
            EitherFunctionSnapshot::LimitedOs(call) => {
                let result = py.detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)));
                EitherProgress::Limited(result.map_err(|e| to_err(py, e))?)
            }
            EitherFunctionSnapshot::ReplNoLimitFn(call, owner) => {
                let result = py
                    .detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)))
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplNoLimit(result, owner)
            }
            EitherFunctionSnapshot::ReplNoLimitOs(call, owner) => {
                let result = py
                    .detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)))
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplNoLimit(result, owner)
            }
            EitherFunctionSnapshot::ReplLimitedFn(call, owner) => {
                let result = py
                    .detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)))
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplLimited(result, owner)
            }
            EitherFunctionSnapshot::ReplLimitedOs(call, owner) => {
                let result = py
                    .detach(|| self.print_callback.with_writer(|w| call.resume(external_result, w)))
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplLimited(result, owner)
            }
            EitherFunctionSnapshot::Done => return Err(PyRuntimeError::new_err("Progress already resumed")),
        };

        // Auto-dispatch OS calls if the caller provided `mount`/`os`. For REPL
        // variants the helper handles REPL rollback on any error itself.
        let progress = match os_handler {
            Some(handler) => progress.drive_through_os_calls(py, handler, &self.print_callback, &self.dc_registry)?,
            None => progress,
        };

        let dc_registry = self.dc_registry.clone_ref(py);
        progress.progress_or_complete(
            py,
            self.script_name.clone(),
            self.print_callback.clone_handle(py),
            dc_registry,
        )
    }
}

#[pymethods]
impl PyFunctionSnapshot {
    /// Resumes execution with a result dict.
    ///
    /// `result` must be a dict with exactly one of `'return_value'`,
    /// `'exception'`, or `'future'`. The dict-shaped API matches the inner
    /// values of `FutureSnapshot.resume({call_id: {...}, ...})` so callers
    /// can construct results uniformly.
    ///
    /// When `mount` or `os` is provided, OS calls produced by the resumed
    /// execution are auto-dispatched internally until a non-OS event is reached,
    /// matching the semantics of `Monty.start(mount=..., os=...)`.
    ///
    /// # Raises
    /// * `TypeError` if `result` is not a dict with exactly one of the expected keys
    /// * `RuntimeError` if the snapshot has already been resumed
    #[pyo3(signature = (result, *, mount=None, os=None))]
    pub fn resume<'py>(
        &self,
        py: Python<'py>,
        result: &Bound<'_, PyDict>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Validate everything BEFORE consuming the snapshot. A failure here
        // (bad mount/os, malformed result dict, unconvertible return_value)
        // must leave the snapshot intact so the caller can retry — and for
        // REPL variants, must avoid leaking the REPL stored inside the call.
        let os_handler = OsHandler::from_run_args(py, mount, os)?;
        let external_result = extract_external_result(py, result, &self.dc_registry, self.call_id)?;

        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Snapshot is currently being resumed by another thread"))?;

        if matches!(*snapshot, EitherFunctionSnapshot::Done) {
            Err(PyRuntimeError::new_err("Progress already resumed"))
        } else {
            let snapshot = mem::replace(&mut *snapshot, EitherFunctionSnapshot::Done);
            self.resume_with_result(py, snapshot, external_result, os_handler.as_ref())
        }
    }

    /// Resumes an OS snapshot using Monty's default "not handled" behavior.
    ///
    /// This is only valid for OS function snapshots. It resumes execution as if
    /// no handler had been available for the pending OS call, producing the same
    /// `PermissionError` or `RuntimeError` that Monty would normally raise.
    ///
    /// When `mount` or `os` is provided, subsequent OS calls produced by the
    /// resumed execution are auto-dispatched, matching the semantics of
    /// `Monty.start(mount=..., os=...)`.
    #[pyo3(signature = (*, mount=None, os=None))]
    pub fn resume_not_handled<'py>(
        &self,
        py: Python<'py>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let os_handler = OsHandler::from_run_args(py, mount, os)?;

        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Snapshot is currently being resumed by another thread"))?;

        let external_result = match &*snapshot {
            EitherFunctionSnapshot::NoLimitOs(call) => call.function.on_no_handler(&call.args).into(),
            EitherFunctionSnapshot::LimitedOs(call) => call.function.on_no_handler(&call.args).into(),
            EitherFunctionSnapshot::ReplNoLimitOs(call, _) => call.function.on_no_handler(&call.args).into(),
            EitherFunctionSnapshot::ReplLimitedOs(call, _) => call.function.on_no_handler(&call.args).into(),
            EitherFunctionSnapshot::Done => return Err(PyRuntimeError::new_err("Progress already resumed")),
            _ => {
                return Err(PyTypeError::new_err(
                    "resume_not_handled() is only valid for OS function snapshots",
                ));
            }
        };

        let snapshot = mem::replace(&mut *snapshot, EitherFunctionSnapshot::Done);
        self.resume_with_result(py, snapshot, external_result, os_handler.as_ref())
    }

    /// Serializes the FunctionSnapshot instance to a binary format.
    ///
    /// The serialized data can be stored and later restored with `load_snapshot()`
    /// or `load_repl_snapshot()`. REPL snapshots automatically include the REPL state.
    ///
    /// Note: The `print_callback` is not serialized and must be re-provided when loading.
    ///
    /// # Returns
    /// Bytes containing the serialized FunctionSnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if serialization fails.
    /// `RuntimeError` if the progress has already been resumed.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = serialization::dump_function_snapshot(
            py,
            &self.snapshot,
            &self.script_name,
            self.is_os_function,
            self.is_method_call,
            &self.function_name,
            &self.args,
            &self.kwargs,
            self.call_id,
        )?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Converts the stored Monty args into a Python `tuple` on each access.
    /// Like `MontyComplete.output`, the conversion is redone every call so
    /// the class can stay frozen and we don't keep a second copy of the
    /// data; heavy consumers should bind the result to a local.
    #[getter]
    fn args<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let items: Vec<Py<PyAny>> = self
            .args
            .iter()
            .map(|item| monty_to_py(py, item, &self.dc_registry))
            .collect::<PyResult<_>>()?;
        PyTuple::new(py, items)
    }

    /// Converts the stored Monty kwargs into a Python `dict` on each access.
    /// Same on-demand rationale as `args`.
    #[getter]
    fn kwargs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (k, v) in &self.kwargs {
            dict.set_item(
                monty_to_py(py, k, &self.dc_registry)?,
                monty_to_py(py, v, &self.dc_registry)?,
            )?;
        }
        Ok(dict)
    }

    /// Serializes the positional args as a JSON array using the natural-form
    /// mapping (see [`JsonMontyObject`]) — JSON-native Python values are
    /// emitted bare, non-JSON-native values get a `{"$<tag>": ...}` wrapper.
    fn args_json(&self) -> PyResult<String> {
        serde_json::to_string(&JsonMontyArray(&self.args))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to serialize args as JSON: {e}")))
    }

    /// Serializes the keyword args as a JSON object. Python kwargs always
    /// have string keys, so this is a plain `{"<name>": <value>, ...}`
    /// object; values use the same natural-form mapping as `args_json`.
    fn kwargs_json(&self) -> PyResult<String> {
        serde_json::to_string(&JsonMontyPairs(&self.kwargs))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to serialize kwargs as JSON: {e}")))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "FunctionSnapshot(script_name='{}', function_name='{}', args={}, kwargs={})",
            self.script_name,
            self.function_name,
            self.args(py)?.repr()?,
            self.kwargs(py)?.repr()?
        ))
    }
}

/// Runtime execution snapshot, holds a `NameLookup` for both
/// resource tracker variants since pyclass structs can't be generic.
///
/// Also holds REPL variants with `Py<PyMontyRepl>` for `MontyRepl.feed_start()`.
///
/// The `Done` variant indicates the snapshot has been consumed.
#[derive(Debug)]
pub(crate) enum EitherLookupSnapshot {
    NoLimit(NameLookup<PySignalTracker<NoLimitTracker>>),
    Limited(NameLookup<PySignalTracker<LimitedTracker>>),
    ReplNoLimit(ReplNameLookup<PySignalTracker<NoLimitTracker>>, Py<PyMontyRepl>),
    ReplLimited(ReplNameLookup<PySignalTracker<LimitedTracker>>, Py<PyMontyRepl>),
    /// Sentinel indicating the snapshot has been consumed via `resume()`.
    Done,
}

/// Helper trait for wrapping `NameLookup<T>` into `EitherLookupSnapshot`.
trait FromNameLookup<T: ResourceTracker> {
    /// Wraps a name lookup into the appropriate variant.
    fn from_name_lookup(lookup: NameLookup<T>) -> Self;
}

impl FromNameLookup<PySignalTracker<NoLimitTracker>> for EitherLookupSnapshot {
    fn from_name_lookup(lookup: NameLookup<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimit(lookup)
    }
}

impl FromNameLookup<PySignalTracker<LimitedTracker>> for EitherLookupSnapshot {
    fn from_name_lookup(lookup: NameLookup<PySignalTracker<LimitedTracker>>) -> Self {
        Self::Limited(lookup)
    }
}

/// Helper trait for wrapping `ReplNameLookup<T>` into `EitherLookupSnapshot`.
trait FromReplNameLookup<T: ResourceTracker> {
    /// Wraps a REPL name lookup into the appropriate variant.
    fn from_repl_name_lookup(lookup: ReplNameLookup<T>, owner: Py<PyMontyRepl>) -> Self;
}

impl FromReplNameLookup<PySignalTracker<NoLimitTracker>> for EitherLookupSnapshot {
    fn from_repl_name_lookup(lookup: ReplNameLookup<PySignalTracker<NoLimitTracker>>, owner: Py<PyMontyRepl>) -> Self {
        Self::ReplNoLimit(lookup, owner)
    }
}

impl FromReplNameLookup<PySignalTracker<LimitedTracker>> for EitherLookupSnapshot {
    fn from_repl_name_lookup(lookup: ReplNameLookup<PySignalTracker<LimitedTracker>>, owner: Py<PyMontyRepl>) -> Self {
        Self::ReplLimited(lookup, owner)
    }
}

/// Snapshot generated during execution when monty yields to the host for a name lookup.
#[pyclass(name = "NameLookupSnapshot", module = "pydantic_monty")]
#[derive(Debug)]
pub struct PyNameLookupSnapshot {
    snapshot: Mutex<EitherLookupSnapshot>,
    print_callback: PrintTarget,
    dc_registry: DcRegistry,

    /// Name of the script being executed
    #[pyo3(get)]
    pub script_name: String,

    /// Name of the variable being looked up
    #[pyo3(get)]
    pub variable_name: String,
}

impl PyNameLookupSnapshot {
    /// Creates a `PyNameLookupSnapshot` for an external function call.
    ///
    /// Extracts display fields from the `FunctionCall` before moving it into
    /// `EitherSnapshot` via the provided `wrap` closure.
    fn new_py_any<T: ResourceTracker>(
        py: Python<'_>,
        lookup: NameLookup<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherLookupSnapshot: FromNameLookup<T>,
    {
        let variable_name = lookup.name.clone();

        let slf = Self {
            snapshot: Mutex::new(EitherLookupSnapshot::from_name_lookup(lookup)),
            print_callback,
            dc_registry,
            script_name,
            variable_name,
        };
        slf.into_bound_py_any(py)
    }

    /// Creates a `PyNameLookupSnapshot` for a REPL name lookup.
    fn repl_name_lookup<T: ResourceTracker>(
        py: Python<'_>,
        lookup: ReplNameLookup<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
        repl_owner: Py<PyMontyRepl>,
        variable_name: String,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherLookupSnapshot: FromReplNameLookup<T>,
    {
        let slf = Self {
            snapshot: Mutex::new(EitherLookupSnapshot::from_repl_name_lookup(lookup, repl_owner)),
            print_callback,
            dc_registry,
            script_name,
            variable_name,
        };
        slf.into_bound_py_any(py)
    }

    /// Constructs a `PyNameLookupSnapshot` from deserialized parts.
    pub(crate) fn from_deserialized(
        py: Python<'_>,
        snapshot: EitherLookupSnapshot,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
        script_name: String,
        variable_name: String,
    ) -> PyResult<Bound<'_, PyAny>> {
        let slf = Self {
            snapshot: Mutex::new(snapshot),
            print_callback,
            dc_registry,
            script_name,
            variable_name,
        };
        slf.into_bound_py_any(py)
    }
}

#[pymethods]
impl PyNameLookupSnapshot {
    /// Resumes execution with either a value or undefined.
    ///
    /// When `mount` or `os` is provided, OS calls produced after the name is
    /// resolved are auto-dispatched until a non-OS event is reached, matching
    /// the semantics of `Monty.start(mount=..., os=...)`.
    #[pyo3(signature = (*, mount=None, os=None, **kwargs))]
    pub fn resume<'py>(
        &self,
        py: Python<'py>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Validate everything BEFORE consuming the snapshot — a `py_to_monty`
        // failure on `value` must leave the snapshot intact for retry, and
        // (for REPL variants) avoid leaking the REPL stored inside.
        let os_handler = OsHandler::from_run_args(py, mount, os)?;
        let lookup_result = if let Some(kwargs) = kwargs
            && let Some(value) = kwargs.get_item(intern!(py, "value"))?
        {
            NameLookupResult::Value(py_to_monty(&value, &self.dc_registry)?)
        } else {
            NameLookupResult::Undefined
        };

        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Snapshot is currently being resumed by another thread"))?;

        if matches!(*snapshot, EitherLookupSnapshot::Done) {
            return Err(PyRuntimeError::new_err("Progress already resumed"));
        }
        let snapshot = mem::replace(&mut *snapshot, EitherLookupSnapshot::Done);

        let to_err = |py: Python<'_>, e| MontyError::new_err(py, e);

        let progress = match snapshot {
            EitherLookupSnapshot::NoLimit(snapshot) => {
                let result = py.detach(|| self.print_callback.with_writer(|w| snapshot.resume(lookup_result, w)));
                EitherProgress::NoLimit(result.map_err(|e| to_err(py, e))?)
            }
            EitherLookupSnapshot::Limited(snapshot) => {
                let result = py.detach(|| self.print_callback.with_writer(|w| snapshot.resume(lookup_result, w)));
                EitherProgress::Limited(result.map_err(|e| to_err(py, e))?)
            }
            EitherLookupSnapshot::ReplNoLimit(snapshot, owner) => {
                let result = py
                    .detach(|| self.print_callback.with_writer(|w| snapshot.resume(lookup_result, w)))
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplNoLimit(result, owner)
            }
            EitherLookupSnapshot::ReplLimited(snapshot, owner) => {
                let result = py
                    .detach(|| self.print_callback.with_writer(|w| snapshot.resume(lookup_result, w)))
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplLimited(result, owner)
            }
            EitherLookupSnapshot::Done => return Err(PyRuntimeError::new_err("Progress already resumed")),
        };

        let progress = match os_handler.as_ref() {
            Some(handler) => progress.drive_through_os_calls(py, handler, &self.print_callback, &self.dc_registry)?,
            None => progress,
        };

        // Clone the Arc handle for the next snapshot/complete
        let dc_registry = self.dc_registry.clone_ref(py);
        progress.progress_or_complete(
            py,
            self.script_name.clone(),
            self.print_callback.clone_handle(py),
            dc_registry,
        )
    }

    /// Serializes the NameLookupSnapshot instance to a binary format.
    ///
    /// The serialized data can be stored and later restored with `load_snapshot()`
    /// or `load_repl_snapshot()`. REPL snapshots automatically include the REPL state.
    ///
    /// Note: The `print_callback` is not serialized and must be re-provided when loading.
    ///
    /// # Returns
    /// Bytes containing the serialized NameLookupSnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if serialization fails.
    /// `RuntimeError` if the progress has already been resumed.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = serialization::dump_lookup_snapshot(py, &self.snapshot, &self.script_name, &self.variable_name)?;
        Ok(PyBytes::new(py, &bytes))
    }

    fn __repr__(&self) -> String {
        format!(
            "NameLookupSnapshot(script_name='{}', variable_name={:?})",
            self.script_name, self.variable_name
        )
    }
}

/// Holds a `ResolveFutures` for either resource tracker variant.
///
/// Also holds REPL variants with `Py<PyMontyRepl>` for `MontyRepl.feed_start()`.
///
/// Used internally by `PyFutureSnapshot` to store execution state when
/// awaiting resolution of pending async external calls.
#[derive(Debug)]
pub(crate) enum EitherFutureSnapshot {
    NoLimit(ResolveFutures<PySignalTracker<NoLimitTracker>>),
    Limited(ResolveFutures<PySignalTracker<LimitedTracker>>),
    ReplNoLimit(ReplResolveFutures<PySignalTracker<NoLimitTracker>>, Py<PyMontyRepl>),
    ReplLimited(ReplResolveFutures<PySignalTracker<LimitedTracker>>, Py<PyMontyRepl>),
    /// Sentinel indicating the snapshot has been consumed via `resume()`.
    Done,
}

/// Helper trait for wrapping `ResolveFutures<T>` into `EitherFutureSnapshot`.
trait FromResolveFutures<T: ResourceTracker> {
    /// Wraps a resolve-futures state into the appropriate variant.
    fn from_resolve_futures(state: ResolveFutures<T>) -> Self;
}

impl FromResolveFutures<PySignalTracker<NoLimitTracker>> for EitherFutureSnapshot {
    fn from_resolve_futures(state: ResolveFutures<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimit(state)
    }
}

impl FromResolveFutures<PySignalTracker<LimitedTracker>> for EitherFutureSnapshot {
    fn from_resolve_futures(state: ResolveFutures<PySignalTracker<LimitedTracker>>) -> Self {
        Self::Limited(state)
    }
}

/// Helper trait for wrapping `ReplResolveFutures<T>` into `EitherFutureSnapshot`.
trait FromReplResolveFutures<T: ResourceTracker> {
    /// Wraps a REPL resolve-futures state into the appropriate variant.
    fn from_repl_resolve_futures(state: ReplResolveFutures<T>, owner: Py<PyMontyRepl>) -> Self;
}

impl FromReplResolveFutures<PySignalTracker<NoLimitTracker>> for EitherFutureSnapshot {
    fn from_repl_resolve_futures(
        state: ReplResolveFutures<PySignalTracker<NoLimitTracker>>,
        owner: Py<PyMontyRepl>,
    ) -> Self {
        Self::ReplNoLimit(state, owner)
    }
}

impl FromReplResolveFutures<PySignalTracker<LimitedTracker>> for EitherFutureSnapshot {
    fn from_repl_resolve_futures(
        state: ReplResolveFutures<PySignalTracker<LimitedTracker>>,
        owner: Py<PyMontyRepl>,
    ) -> Self {
        Self::ReplLimited(state, owner)
    }
}

/// Snapshot generated during execution when monty yields to the host to resolve a future.
///
/// Works for both `Monty.start()` and `MontyRepl.feed_start()`.
#[pyclass(name = "FutureSnapshot", module = "pydantic_monty", frozen)]
#[derive(Debug)]
pub struct PyFutureSnapshot {
    snapshot: Mutex<EitherFutureSnapshot>,
    print_callback: PrintTarget,
    dc_registry: DcRegistry,

    /// Name of the script being executed
    #[pyo3(get)]
    pub script_name: String,
}

impl PyFutureSnapshot {
    fn new_py_any<T: ResourceTracker>(
        py: Python<'_>,
        state: ResolveFutures<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherFutureSnapshot: FromResolveFutures<T>,
    {
        let slf = Self {
            snapshot: Mutex::new(EitherFutureSnapshot::from_resolve_futures(state)),
            print_callback,
            dc_registry,
            script_name,
        };
        slf.into_bound_py_any(py)
    }

    /// Constructs a `PyFutureSnapshot` from deserialized parts.
    ///
    /// Used by `load_snapshot` and `load_repl_snapshot` to reconstruct snapshot objects.
    pub(crate) fn from_deserialized(
        py: Python<'_>,
        snapshot: EitherFutureSnapshot,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
        script_name: String,
    ) -> PyResult<Bound<'_, PyAny>> {
        let slf = Self {
            snapshot: Mutex::new(snapshot),
            print_callback,
            dc_registry,
            script_name,
        };
        slf.into_bound_py_any(py)
    }

    /// Creates a `PyFutureSnapshot` for a REPL resolve-futures state.
    fn repl_resolve_futures<T: ResourceTracker>(
        py: Python<'_>,
        state: ReplResolveFutures<T>,
        script_name: String,
        print_callback: PrintTarget,
        dc_registry: DcRegistry,
        repl_owner: Py<PyMontyRepl>,
    ) -> PyResult<Bound<'_, PyAny>>
    where
        EitherFutureSnapshot: FromReplResolveFutures<T>,
    {
        let slf = Self {
            snapshot: Mutex::new(EitherFutureSnapshot::from_repl_resolve_futures(state, repl_owner)),
            print_callback,
            dc_registry,
            script_name,
        };
        slf.into_bound_py_any(py)
    }
}

#[pymethods]
impl PyFutureSnapshot {
    /// Resumes execution with results for one or more futures.
    ///
    /// When `mount` or `os` is provided, OS calls produced after the futures
    /// resolve are auto-dispatched until a non-OS event is reached, matching
    /// the semantics of `Monty.start(mount=..., os=...)`.
    #[pyo3(signature = (results, *, mount=None, os=None))]
    pub fn resume<'py>(
        &self,
        py: Python<'py>,
        results: &Bound<'_, PyDict>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Validate everything BEFORE consuming the snapshot — a malformed
        // `results` dict must leave the snapshot intact for retry, and
        // (for REPL variants) avoid leaking the REPL stored inside.
        let os_handler = OsHandler::from_run_args(py, mount, os)?;
        let external_results = results
            .iter()
            .map(|(key, value)| {
                let call_id = key.extract::<u32>()?;
                let dict = value.cast::<PyDict>()?;
                let value = extract_external_result(py, dict, &self.dc_registry, call_id)?;
                Ok((call_id, value))
            })
            .collect::<PyResult<Vec<_>>>()?;

        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Snapshot is currently being resumed by another thread"))?;

        if matches!(*snapshot, EitherFutureSnapshot::Done) {
            return Err(PyRuntimeError::new_err("Progress already resumed"));
        }
        let snapshot = mem::replace(&mut *snapshot, EitherFutureSnapshot::Done);

        let to_err = |py: Python<'_>, e| MontyError::new_err(py, e);

        let progress = match snapshot {
            EitherFutureSnapshot::NoLimit(snapshot) => {
                let result = py.detach(|| {
                    self.print_callback
                        .with_writer(|w| snapshot.resume(external_results, w))
                });
                EitherProgress::NoLimit(result.map_err(|e| to_err(py, e))?)
            }
            EitherFutureSnapshot::Limited(snapshot) => {
                let result = py.detach(|| {
                    self.print_callback
                        .with_writer(|w| snapshot.resume(external_results, w))
                });
                EitherProgress::Limited(result.map_err(|e| to_err(py, e))?)
            }
            EitherFutureSnapshot::ReplNoLimit(snapshot, owner) => {
                let result = py
                    .detach(|| {
                        self.print_callback
                            .with_writer(|w| snapshot.resume(external_results, w))
                    })
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplNoLimit(result, owner)
            }
            EitherFutureSnapshot::ReplLimited(snapshot, owner) => {
                let result = py
                    .detach(|| {
                        self.print_callback
                            .with_writer(|w| snapshot.resume(external_results, w))
                    })
                    .map_err(|e| restore_repl_from_repl_start_error(py, &owner, *e))?;
                EitherProgress::ReplLimited(result, owner)
            }
            EitherFutureSnapshot::Done => return Err(PyRuntimeError::new_err("Progress already resumed")),
        };

        let progress = match os_handler.as_ref() {
            Some(handler) => progress.drive_through_os_calls(py, handler, &self.print_callback, &self.dc_registry)?,
            None => progress,
        };

        let dc_registry = self.dc_registry.clone_ref(py);
        progress.progress_or_complete(
            py,
            self.script_name.clone(),
            self.print_callback.clone_handle(py),
            dc_registry,
        )
    }

    /// Returns the pending call IDs associated with the FutureSnapshot instance.
    ///
    /// # Returns
    /// A slice of pending call IDs.
    #[getter]
    fn pending_call_ids<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        match &*snapshot {
            EitherFutureSnapshot::NoLimit(snapshot) => PyList::new(py, snapshot.pending_call_ids()),
            EitherFutureSnapshot::Limited(snapshot) => PyList::new(py, snapshot.pending_call_ids()),
            EitherFutureSnapshot::ReplNoLimit(snapshot, _) => PyList::new(py, snapshot.pending_call_ids()),
            EitherFutureSnapshot::ReplLimited(snapshot, _) => PyList::new(py, snapshot.pending_call_ids()),
            EitherFutureSnapshot::Done => Err(PyRuntimeError::new_err("FutureSnapshot already resumed")),
        }
    }

    /// Serializes the FutureSnapshot instance to a binary format.
    ///
    /// The serialized data can be stored and later restored with `load_snapshot()`
    /// or `load_repl_snapshot()`. REPL snapshots automatically include the REPL state.
    ///
    /// Note: The `print_callback` is not serialized and must be re-provided when loading.
    ///
    /// # Returns
    /// Bytes containing the serialized FutureSnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if serialization fails.
    /// `RuntimeError` if the progress has already been resumed.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = serialization::dump_future_snapshot(py, &self.snapshot, &self.script_name)?;
        Ok(PyBytes::new(py, &bytes))
    }

    fn __repr__(&self) -> String {
        let snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        let pending_call_ids = match &*snapshot {
            EitherFutureSnapshot::NoLimit(s) => s.pending_call_ids(),
            EitherFutureSnapshot::Limited(s) => s.pending_call_ids(),
            EitherFutureSnapshot::ReplNoLimit(s, _) => s.pending_call_ids(),
            EitherFutureSnapshot::ReplLimited(s, _) => s.pending_call_ids(),
            EitherFutureSnapshot::Done => &[],
        };
        format!(
            "FutureSnapshot(script_name='{}', pending_call_ids={pending_call_ids:?})",
            self.script_name,
        )
    }
}

/// Terminal result of an iterative Monty run.
///
/// `Monty.start()` and the snapshot `resume()` methods yield `MontyComplete`
/// when execution finishes without requiring the direct `run()` APIs to change
/// their return type.
///
/// The final value is stored as a `MontyObject` and converted to a Python
/// object lazily on each access of the `output` property. This lets
/// `output_json()` serialize the value directly from the Rust representation
/// without a Python round-trip, while still giving Python callers a native
/// Python value when they want one.
#[pyclass(name = "MontyComplete", module = "pydantic_monty", frozen)]
pub struct PyMontyComplete {
    /// Value produced by the last expression of the run, in Monty's native
    /// representation. Converted to a Python value on demand via `output`.
    monty_output: MontyObject,
    /// Dataclass registry required to reconstruct registered dataclass
    /// instances when converting `monty_output` back to Python. Shares the
    /// same underlying Python dict as the registry used during execution.
    dc_registry: DcRegistry,
    // TODO we might want to add stats on execution here like time, allocations, etc.
}

impl PyMontyComplete {
    /// Builds a `MontyComplete` by taking ownership of the final Monty output
    /// value. The registry clone is a cheap refcount bump on the underlying
    /// Python dict.
    fn create<'py>(py: Python<'py>, output: MontyObject, dc_registry: &DcRegistry) -> PyResult<Bound<'py, PyAny>> {
        let slf = Self {
            monty_output: output,
            dc_registry: dc_registry.clone_ref(py),
        };
        slf.into_bound_py_any(py)
    }
}

#[pymethods]
impl PyMontyComplete {
    /// Converts the stored `MontyObject` into a Python value on each access.
    ///
    /// Conversion is intentionally re-done every call: it keeps the class
    /// `frozen` (no interior mutability needed) and avoids retaining a
    /// Python-side copy of the output that may never be read.
    #[getter]
    fn output(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        monty_to_py(py, &self.monty_output, &self.dc_registry)
    }

    /// Serializes the output as JSON using the natural-form mapping
    /// (see [`JsonMontyObject`]): JSON-native Python types become bare JSON
    /// values, non-JSON-native types are wrapped in a `{"$<tag>": ...}`
    /// object. This format is **output-only** and not round-trippable.
    fn output_json(&self) -> PyResult<String> {
        serde_json::to_string(&JsonMontyObject(&self.monty_output))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to serialize output as JSON: {e}")))
    }

    fn __repr__(&self) -> String {
        format!("MontyComplete(output={})", self.monty_output.py_repr())
    }
}

fn list_str(arg: Option<&Bound<'_, PyList>>, name: &str) -> PyResult<Vec<String>> {
    if let Some(names) = arg {
        names
            .iter()
            .map(|item| item.extract::<String>())
            .collect::<PyResult<Vec<_>>>()
            .map_err(|e| PyTypeError::new_err(format!("{name}: {e}")))
    } else {
        Ok(vec![])
    }
}

/// Recursively checks whether a `MontyObject` contains a dataclass, including
/// inside containers like `List`, `Tuple`, and `Dict`.
///
/// This is used to decide whether to take the iterative execution path: dataclass
/// method calls need host dispatch, so if any input (even nested) is a dataclass
/// we must use the iterative runner rather than the non-iterative `run()`.
fn contains_dataclass(obj: &MontyObject) -> bool {
    match obj {
        MontyObject::Dataclass { .. } => true,
        MontyObject::List(items) | MontyObject::Tuple(items) => items.iter().any(contains_dataclass),
        MontyObject::Dict(pairs) => pairs
            .into_iter()
            .any(|(k, v)| contains_dataclass(k) || contains_dataclass(v)),
        _ => false,
    }
}

/// Serialization wrapper for `PyMonty` that includes all fields needed for reconstruction.
#[derive(serde::Serialize, serde::Deserialize)]
struct SerializedMonty {
    runner: MontyRun,
    script_name: String,
    input_names: Vec<String>,
}

/// Extract an external result (object or exception) from a dictionary.
///
/// Any dataclass return values are automatically registered in the `dc_registry` via `py_to_monty`
/// so they can be properly reconstructed on output.
/// Extracts an `ExternalResult` from a Python dict.
///
/// Accepts one of:
/// * `{'return_value': obj}` — a successful return value
/// * `{'exception': exc}` — a concrete `BaseException` instance
/// * `{'exc_type': name, 'message'?: str}` — an exception identified by type name
/// * `{'future': ...}` — a pending coroutine result (`...` is the only valid value)
///
/// The `call_id` is required for `future` results to track the pending call.
fn extract_external_result(
    py: Python<'_>,
    dict: &Bound<'_, PyDict>,
    dc_registry: &DcRegistry,
    call_id: u32,
) -> PyResult<ExtFunctionResult> {
    const ARGS_ERROR: &str = "ExternalResult must be a dict with one of: 'return_value', 'exception', 'exc_type' (with optional 'message'), or 'future'";
    // ExternalExceptionData variant: {'exc_type': str, 'message'?: str} — handled
    // separately from the other variants because it can have two keys.
    if let Some(exc_type_val) = dict.get_item(intern!(py, "exc_type"))? {
        let message_val = dict.get_item(intern!(py, "message"))?;
        let expected_len = if message_val.is_some() { 2 } else { 1 };
        if dict.len() != expected_len {
            return Err(PyTypeError::new_err(ARGS_ERROR));
        }
        let exc_type_str: String = exc_type_val
            .extract()
            .map_err(|_| PyTypeError::new_err("'exc_type' must be a string"))?;
        let exc_type = exc_type_str
            .parse::<ExcType>()
            .map_err(|_| PyTypeError::new_err(format!("Unknown exception type: '{exc_type_str}'")))?;
        let message = message_val
            .map(|m| {
                m.extract::<String>()
                    .map_err(|_| PyTypeError::new_err("'message' must be a string"))
            })
            .transpose()?;
        return Ok(ExtFunctionResult::Error(MontyException::new(exc_type, message)));
    }
    if dict.len() != 1 {
        Err(PyTypeError::new_err(ARGS_ERROR))
    } else if let Some(rv) = dict.get_item(intern!(py, "return_value"))? {
        // Return value provided
        Ok(py_to_monty(&rv, dc_registry)?.into())
    } else if let Some(exc) = dict.get_item(intern!(py, "exception"))? {
        // Exception provided
        if PyBaseException::type_check(&exc) {
            let py_err = PyErr::from_value(exc.into_any());
            Ok(exc_py_to_monty(py, &py_err).into())
        } else {
            let to = PyBaseException::classinfo_object(py);
            Err(CastIntoError::new(exc, to).into())
        }
    } else if let Some(exc) = dict.get_item(intern!(py, "future"))? {
        // Future provided
        if exc.eq(py.Ellipsis()).unwrap_or_default() {
            Ok(ExtFunctionResult::Future(call_id))
        } else {
            Err(PyTypeError::new_err(
                "Value for the 'future' key must be Ellipsis (...)",
            ))
        }
    } else {
        // wrong key in kwargs
        Err(PyTypeError::new_err(ARGS_ERROR))
    }
}

/// Extracts the REPL from a `ReplStartError`, restores it into the owner,
/// and returns the Python exception.
fn restore_repl_from_repl_start_error<T: ResourceTracker>(
    py: Python<'_>,
    repl_owner: &Py<PyMontyRepl>,
    err: ReplStartError<T>,
) -> PyErr
where
    EitherRepl: FromCoreRepl<T>,
{
    repl_owner
        .get()
        .put_repl_after_rollback(EitherRepl::from_core(err.repl));
    MontyError::new_err(py, err.error)
}

/// Auto-dispatches [`RunProgress::OsCall`] events until a non-OS progress is reached.
///
/// Used by [`PyMonty::start`] when the caller supplies a `mount` or `os` argument:
/// the method should behave like `run()` for OS calls (resolve them internally
/// via the mount table and optional Python fallback) but like `start()` for
/// non-OS events (return the snapshot so the caller can drive external functions,
/// name lookups, or futures from Python).
///
/// Mounts are taken out of their shared slots lazily on the first OS call and
/// put back on every exit path (the non-OS return, resume failure, or
/// [`handle_mount_os_call`] error) so the taken/put-back invariant matches
/// `run_impl` without failing on mount contention for progress that never
/// reaches an OS call.
pub(crate) fn drive_run_progress_through_os_calls<T: ResourceTracker + Send>(
    py: Python<'_>,
    mut progress: RunProgress<T>,
    handler: &OsHandler,
    print_target: &PrintTarget,
    dc_registry: &DcRegistry,
) -> PyResult<RunProgress<T>> {
    let mut mount_table: Option<MountTable> = None;
    let fallback = handler.fallback.as_ref();
    let put_back = |mount_table: &mut Option<MountTable>| {
        if let Some(table) = mount_table.take() {
            handler.put_back(table);
        }
    };
    loop {
        match progress {
            RunProgress::OsCall(call) => {
                let table = if let Some(table) = mount_table.as_mut() {
                    table
                } else {
                    let table = handler.take()?;
                    mount_table.insert(table)
                };
                let result = match handle_mount_os_call(py, &call, table, fallback, dc_registry) {
                    Ok(r) => r,
                    Err(e) => {
                        put_back(&mut mount_table);
                        return Err(e);
                    }
                };
                progress = match py.detach(|| print_target.with_writer(|w| call.resume(result, w))) {
                    Ok(p) => p,
                    Err(e) => {
                        put_back(&mut mount_table);
                        return Err(MontyError::new_err(py, e));
                    }
                };
            }
            other => {
                put_back(&mut mount_table);
                return Ok(other);
            }
        }
    }
}

/// Handles an OS call via a Rust [`MountTable`], falling through to the
/// `fallback` callable for unhandled operations.
///
/// The mount table returns `None` for non-filesystem ops and for paths that
/// don't match any mount. In both cases we try the fallback, or fall back to
/// [`OsFunction::on_no_handler`] which returns `PermissionError` for filesystem
/// ops and `RuntimeError` for non-filesystem ops.
pub(crate) fn handle_mount_os_call<T: ResourceTracker>(
    py: Python<'_>,
    call: &OsCall<T>,
    table: &mut MountTable,
    fallback: Option<&Py<PyAny>>,
    dc_registry: &DcRegistry,
) -> PyResult<ExtFunctionResult> {
    match table.handle_os_call(call.function, &call.args, &call.kwargs) {
        Some(Ok(obj)) => Ok(obj.into()),
        Some(Err(mount_err)) => Ok(mount_err.into_exception().into()),
        None => {
            // Intentional: unmounted paths fall through to `os=`.
            if let Some(fb) = fallback {
                call_os_callback(py, call, fb.bind(py), dc_registry)
            } else {
                Ok(call.function.on_no_handler(&call.args).into())
            }
        }
    }
}

/// Calls a Python OS callback with the given OS call's function name, args, and kwargs.
pub(crate) fn call_os_callback<T: ResourceTracker>(
    py: Python<'_>,
    call: &OsCall<T>,
    callback: &Bound<'_, PyAny>,
    dc_registry: &DcRegistry,
) -> PyResult<ExtFunctionResult> {
    call_os_callback_parts(
        py,
        &call.function.to_string(),
        &call.args,
        &call.kwargs,
        callback,
        dc_registry,
        || call.function.on_no_handler(&call.args).into(),
    )
}

/// Shared implementation for dispatching an OS callback from either run or REPL progress.
///
/// Both `OsCall<T>` and `ReplOsCall<T>` expose the same user-facing callback
/// shape, so the marshalling and `NOT_HANDLED` semantics live in one place to
/// keep both bindings variants behaviorally identical.
pub(crate) fn call_os_callback_parts(
    py: Python<'_>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    callback: &Bound<'_, PyAny>,
    dc_registry: &DcRegistry,
    on_not_handled: impl FnOnce() -> ExtFunctionResult,
) -> PyResult<ExtFunctionResult> {
    let py_args: Vec<Py<PyAny>> = args
        .iter()
        .map(|arg| monty_to_py(py, arg, dc_registry))
        .collect::<PyResult<_>>()?;
    let py_args_tuple = PyTuple::new(py, py_args)?;

    let py_kwargs = PyDict::new(py);
    for (k, v) in kwargs {
        py_kwargs.set_item(monty_to_py(py, k, dc_registry)?, monty_to_py(py, v, dc_registry)?)?;
    }

    match callback.call1((function_name, py_args_tuple, py_kwargs)) {
        Ok(result) => {
            let not_handled = crate::get_not_handled(py)?.bind(py);
            if result.is(not_handled) {
                Ok(on_not_handled())
            } else {
                Ok(py_to_monty(&result, dc_registry)?.into())
            }
        }
        Err(err) => Ok(exc_py_to_monty(py, &err).into()),
    }
}
