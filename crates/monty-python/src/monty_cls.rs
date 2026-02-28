use std::{
    borrow::Cow,
    fmt::Write,
    sync::{Mutex, PoisonError},
};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{
    ExternalResult, FunctionCall, LimitedTracker, MontyException, MontyObject, MontyRepl as CoreMontyRepl, MontyRun,
    NameLookupResult, NoLimitTracker, OsCall, PrintWriter, PrintWriterCallback, ResolveFutures, ResourceTracker,
    RunProgress,
};
use monty::ExcType;
use monty_type_checking::{SourceFile, type_check};
use pyo3::{
    IntoPyObjectExt,
    exceptions::{PyKeyError, PyRuntimeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyTuple, PyType},
};
use send_wrapper::SendWrapper;

use crate::{
    convert::{monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    exceptions::{MontyError, MontyTypingError, exc_py_to_monty},
    external::{ExternalFunctionRegistry, dispatch_method_call},
    limits::{PySignalTracker, extract_limits},
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
            py_type_check(py, &code, script_name, type_check_stubs)?;
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
        py_type_check(py, self.runner.code(), &self.script_name, prefix_code)
    }

    /// Executes the code and returns the result.
    ///
    /// # Returns
    /// The result of the last expression in the code
    ///
    /// # Raises
    /// Various Python exceptions matching what the code would raise
    #[pyo3(signature = (*, inputs=None, limits=None, external_functions=None, print_callback=None, os=None))]
    fn run(
        &self,
        py: Python<'_>,
        inputs: Option<&Bound<'_, PyDict>>,
        limits: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        // Clone the Arc handle — all clones share the same underlying registry,
        // so auto-registrations during execution are visible to all users.
        let input_values = self.extract_input_values(inputs, &self.dc_registry)?;

        if let Some(os_callback) = os
            && !os_callback.is_callable()
        {
            let msg = format!("TypeError: '{}' object is not callable", os_callback.get_type().name()?);
            return Err(PyTypeError::new_err(msg));
        }

        // Build print writer
        let mut print_cb;
        let print_writer = match print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::new(cb);
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        // Run with appropriate tracker type (must branch due to different generic types)
        if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            self.run_impl(py, input_values, tracker, external_functions, os, print_writer)
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            self.run_impl(py, input_values, tracker, external_functions, os, print_writer)
        }
    }

    #[pyo3(signature = (*, inputs=None, limits=None, print_callback=None))]
    fn start<'py>(
        &self,
        py: Python<'py>,
        inputs: Option<&Bound<'py, PyDict>>,
        limits: Option<&Bound<'py, PyDict>>,
        print_callback: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Clone the Arc handle — shares the same underlying registry
        let dc_registry = self.dc_registry.clone_ref(py);
        let input_values = self.extract_input_values(inputs, &dc_registry)?;

        // Build print writer - CallbackStringPrint is Send so GIL can be released
        let mut print_cb;
        let print_writer = match &print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::new(cb);
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        let runner = self.runner.clone();
        let mut print_writer = SendWrapper::new(print_writer);

        // Helper macro to start execution with GIL released
        macro_rules! start_impl {
            ($tracker:expr) => {{
                py.detach(|| runner.start(input_values, $tracker, &mut print_writer))
                    .map_err(|e| MontyError::new_err(py, e))?
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
        progress.progress_or_complete(
            py,
            self.script_name.clone(),
            print_callback.map(Bound::unbind),
            dc_registry,
        )
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

fn py_type_check(py: Python<'_>, code: &str, script_name: &str, type_stubs: Option<&str>) -> PyResult<()> {
    let type_stubs = type_stubs.map(|type_stubs| SourceFile::new(type_stubs, "type_stubs.pyi"));

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
    fn run_impl(
        &self,
        py: Python<'_>,
        input_values: Vec<MontyObject>,
        tracker: impl ResourceTracker + Send,
        external_functions: Option<&Bound<'_, PyDict>>,
        os: Option<&Bound<'_, PyAny>>,
        mut print_output: PrintWriter<'_>,
    ) -> PyResult<Py<PyAny>> {
        // wrap print_output in SendWrapper so that it can be accessed inside the py.detach calls despite
        // no `Send` bound - py.detach() is overly restrictive to prevent `Bound` types going inside
        let mut print_output = SendWrapper::new(&mut print_output);

        // Check if any inputs contain dataclasses (including nested in containers) —
        // if so, we need the iterative path because method calls could happen lazily
        // and need to be dispatched to the host.
        let has_dataclass_inputs = || input_values.iter().any(contains_dataclass);

        if external_functions.is_none() && os.is_none() && !has_dataclass_inputs() {
            return match py.detach(|| self.runner.run(input_values, tracker, &mut print_output)) {
                Ok(v) => monty_to_py(py, &v, &self.dc_registry),
                Err(err) => Err(MontyError::new_err(py, err)),
            };
        }
        // Clone the runner since start() consumes it - allows reuse of the parsed code
        let runner = self.runner.clone();
        let mut progress = py
            .detach(|| runner.start(input_values, tracker, &mut print_output))
            .map_err(|e| MontyError::new_err(py, e))?;

        loop {
            match progress {
                RunProgress::Complete(result) => return monty_to_py(py, &result, &self.dc_registry),
                RunProgress::FunctionCall(call) => {
                    // Dataclass method calls have method_call=true and the first arg is the instance
                    let return_value = if call.method_call {
                        dispatch_method_call(py, &call.function_name, &call.args, &call.kwargs, &self.dc_registry)
                    } else if let Some(ext_fns) = external_functions {
                        let registry = ExternalFunctionRegistry::new(py, ext_fns, &self.dc_registry);
                        registry.call(&call.function_name, &call.args, &call.kwargs)
                    } else {
                        return Err(PyRuntimeError::new_err(format!(
                            "External function '{}' called but no external_functions provided",
                            call.function_name
                        )));
                    };

                    progress = py
                        .detach(|| call.resume(return_value, &mut print_output))
                        .map_err(|e| MontyError::new_err(py, e))?;
                }
                RunProgress::NameLookup(lookup) => {
                    let result = if let Some(ext_fns) = external_functions {
                        if let Some(value) = ext_fns.get_item(&lookup.name)? {
                            if value.is_callable() {
                                NameLookupResult::Value(MontyObject::Function {
                                    name: lookup.name.clone(),
                                    docstring: String::new(),
                                })
                            } else {
                                NameLookupResult::Value(py_to_monty(&value, &self.dc_registry)?)
                            }
                        } else {
                            NameLookupResult::Undefined
                        }
                    } else {
                        NameLookupResult::Undefined
                    };

                    progress = py
                        .detach(|| lookup.resume(result, &mut print_output))
                        .map_err(|e| MontyError::new_err(py, e))?;
                }
                RunProgress::ResolveFutures(_) => {
                    return Err(PyRuntimeError::new_err("async futures not supported with `Monty.run`"));
                }
                RunProgress::OsCall(call) => {
                    let result: ExternalResult = if let Some(os_callback) = os {
                        // Convert args to Python
                        let py_args: Vec<Py<PyAny>> = call
                            .args
                            .iter()
                            .map(|arg| monty_to_py(py, arg, &self.dc_registry))
                            .collect::<PyResult<_>>()?;
                        let py_args_tuple = PyTuple::new(py, py_args)?;

                        // Convert kwargs to Python dict
                        let py_kwargs = PyDict::new(py);
                        for (k, v) in &call.kwargs {
                            py_kwargs.set_item(
                                monty_to_py(py, k, &self.dc_registry)?,
                                monty_to_py(py, v, &self.dc_registry)?,
                            )?;
                        }

                        // call the os callback, if an exception is raised, return it to monty
                        match os_callback.call1((call.function.to_string(), py_args_tuple, py_kwargs)) {
                            Ok(result) => py_to_monty(&result, &self.dc_registry)?.into(),
                            Err(err) => exc_py_to_monty(py, &err).into(),
                        }
                    } else {
                        MontyException::new(
                            ExcType::NotImplementedError,
                            Some(format!("OS function '{}' not implemented", call.function)),
                        )
                        .into()
                    };

                    progress = py
                        .detach(|| call.resume(result, &mut print_output))
                        .map_err(|e| MontyError::new_err(py, e))?;
                }
            }
        }
    }
}

/// pyclass doesn't support generic types, hence hard coding the generics
#[derive(Debug)]
enum EitherProgress {
    NoLimit(RunProgress<PySignalTracker<NoLimitTracker>>),
    Limited(RunProgress<PySignalTracker<LimitedTracker>>),
}

impl EitherProgress {
    /// Converts a `RunProgress` into the appropriate Python object (snapshot or complete).
    ///
    /// NameLookup events are auto-resolved by treating unresolved names as external functions.
    /// This allows the `start()`/`resume()` path to work without upfront declaration of
    /// external function names — the host will handle the actual function calls when they
    /// arrive as `FunctionCall` events.
    fn progress_or_complete(
        self,
        py: Python<'_>,
        script_name: String,
        print_callback: Option<Py<PyAny>>,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>> {
        // Auto-resolve any NameLookup events before converting to Python objects.
        // In the start/resume path, all unresolved names are assumed to be external
        // functions — the host provides the actual implementation via resume().
        let resolved = self.resolve_name_lookups(py)?;

        match resolved {
            Self::NoLimit(p) => match p {
                RunProgress::Complete(result) => PyMontyComplete::create(py, &result, &dc_registry),
                RunProgress::FunctionCall(call) => Self::function_call_snapshot(
                    py,
                    call,
                    EitherSnapshot::wrap_fn_no_limit,
                    script_name,
                    print_callback,
                    dc_registry,
                ),
                RunProgress::ResolveFutures(state) => Self::future_snapshot(
                    py,
                    EitherFutureSnapshot::NoLimit(state),
                    script_name,
                    print_callback,
                    dc_registry,
                ),
                RunProgress::OsCall(call) => Self::os_call_snapshot(
                    py,
                    call,
                    EitherSnapshot::wrap_os_no_limit,
                    script_name,
                    print_callback,
                    dc_registry,
                ),
                RunProgress::NameLookup(_) => {
                    unreachable!("NameLookup should have been resolved by resolve_name_lookups")
                }
            },
            Self::Limited(p) => match p {
                RunProgress::Complete(result) => PyMontyComplete::create(py, &result, &dc_registry),
                RunProgress::FunctionCall(call) => Self::function_call_snapshot(
                    py,
                    call,
                    EitherSnapshot::wrap_fn_limited,
                    script_name,
                    print_callback,
                    dc_registry,
                ),
                RunProgress::ResolveFutures(state) => Self::future_snapshot(
                    py,
                    EitherFutureSnapshot::Limited(state),
                    script_name,
                    print_callback,
                    dc_registry,
                ),
                RunProgress::OsCall(call) => Self::os_call_snapshot(
                    py,
                    call,
                    EitherSnapshot::wrap_os_limited,
                    script_name,
                    print_callback,
                    dc_registry,
                ),
                RunProgress::NameLookup(_) => {
                    unreachable!("NameLookup should have been resolved by resolve_name_lookups")
                }
            },
        }
    }

    /// Auto-resolves NameLookup events by treating unresolved names as external functions.
    ///
    /// When the start/resume path encounters an unresolved name, it assumes the name refers
    /// to an external function and provides a `MontyObject::Function` value. The VM caches
    /// this in the namespace slot so subsequent accesses don't trigger another NameLookup.
    /// When the function is actually called, it yields a `FunctionCall` for the host to handle.
    fn resolve_name_lookups(self, py: Python<'_>) -> PyResult<Self> {
        let mut current = self;
        loop {
            match current {
                Self::NoLimit(RunProgress::NameLookup(lookup)) => {
                    let result = NameLookupResult::Value(MontyObject::Function {
                        name: lookup.name.clone(),
                        docstring: String::new(),
                    });
                    current = Self::NoLimit(
                        lookup
                            .resume(result, &mut PrintWriter::Stdout)
                            .map_err(|e| MontyError::new_err(py, e))?,
                    );
                }
                Self::Limited(RunProgress::NameLookup(lookup)) => {
                    let result = NameLookupResult::Value(MontyObject::Function {
                        name: lookup.name.clone(),
                        docstring: String::new(),
                    });
                    current = Self::Limited(
                        lookup
                            .resume(result, &mut PrintWriter::Stdout)
                            .map_err(|e| MontyError::new_err(py, e))?,
                    );
                }
                other => return Ok(other),
            }
        }
    }

    /// Creates a `PyMontySnapshot` for an external function call.
    ///
    /// Extracts display fields from the `FunctionCall` before moving it into
    /// `EitherSnapshot` via the provided `wrap` closure.
    fn function_call_snapshot<T: ResourceTracker>(
        py: Python<'_>,
        call: FunctionCall<T>,
        wrap: fn(FunctionCall<T>) -> EitherSnapshot,
        script_name: String,
        print_callback: Option<Py<PyAny>>,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>> {
        let function_name = call.function_name.clone();
        let call_id = call.call_id;
        let method_call = call.method_call;
        let items: PyResult<Vec<Py<PyAny>>> = call
            .args
            .iter()
            .map(|item| monty_to_py(py, item, &dc_registry))
            .collect();
        let dict = PyDict::new(py);
        for (k, v) in &call.kwargs {
            dict.set_item(monty_to_py(py, k, &dc_registry)?, monty_to_py(py, v, &dc_registry)?)?;
        }

        let slf = PyMontySnapshot {
            snapshot: Mutex::new(wrap(call)),
            print_callback,
            script_name,
            is_os_function: false,
            is_method_call: method_call,
            function_name,
            args: PyTuple::new(py, items?)?.unbind(),
            kwargs: dict.unbind(),
            call_id,
            dc_registry,
        };
        slf.into_bound_py_any(py)
    }

    /// Creates a `PyMontySnapshot` for an OS-level call.
    ///
    /// Extracts display fields from the `OsCall` before moving it into
    /// `EitherSnapshot` via the provided `wrap` closure.
    fn os_call_snapshot<T: ResourceTracker>(
        py: Python<'_>,
        call: OsCall<T>,
        wrap: fn(OsCall<T>) -> EitherSnapshot,
        script_name: String,
        print_callback: Option<Py<PyAny>>,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>> {
        let function_name = call.function.to_string();
        let call_id = call.call_id;
        let items: PyResult<Vec<Py<PyAny>>> = call
            .args
            .iter()
            .map(|item| monty_to_py(py, item, &dc_registry))
            .collect();
        let dict = PyDict::new(py);
        for (k, v) in &call.kwargs {
            dict.set_item(monty_to_py(py, k, &dc_registry)?, monty_to_py(py, v, &dc_registry)?)?;
        }

        let slf = PyMontySnapshot {
            snapshot: Mutex::new(wrap(call)),
            print_callback,
            script_name,
            is_os_function: true,
            is_method_call: false,
            function_name,
            args: PyTuple::new(py, items?)?.unbind(),
            kwargs: dict.unbind(),
            call_id,
            dc_registry,
        };
        slf.into_bound_py_any(py)
    }

    fn future_snapshot(
        py: Python<'_>,
        snapshot: EitherFutureSnapshot,
        script_name: String,
        print_callback: Option<Py<PyAny>>,
        dc_registry: DcRegistry,
    ) -> PyResult<Bound<'_, PyAny>> {
        let slf = PyMontyFutureSnapshot {
            snapshot: Mutex::new(snapshot),
            print_callback,
            dc_registry,
            script_name,
        };
        slf.into_bound_py_any(py)
    }
}

/// Runtime REPL session holder for pyclass interoperability.
///
/// PyO3 classes cannot be generic, so this enum stores REPL sessions for both
/// resource tracker variants.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherRepl {
    NoLimit(CoreMontyRepl<PySignalTracker<NoLimitTracker>>),
    Limited(CoreMontyRepl<PySignalTracker<LimitedTracker>>),
}

#[pyclass(name = "MontyRepl", module = "pydantic_monty", frozen)]
#[derive(Debug)]
pub struct PyMontyRepl {
    repl: Mutex<EitherRepl>,
    print_callback: Option<Py<PyAny>>,
    dc_registry: DcRegistry,

    /// Name of the script being executed.
    #[pyo3(get)]
    pub script_name: String,
}

#[pymethods]
impl PyMontyRepl {
    /// Creates a REPL session directly from source code.
    ///
    /// This mirrors `Monty` construction but returns a stateful REPL that can
    /// be fed incrementally without replay.
    ///
    /// # Returns
    /// `(repl, output)` where `output` is the initial execution result.
    #[staticmethod]
    #[pyo3(signature = (code, *, script_name="main.py", inputs=None, start_inputs=None, limits=None, print_callback=None, dataclass_registry=None))]
    #[expect(clippy::too_many_arguments)]
    fn create(
        py: Python<'_>,
        code: String,
        script_name: &str,
        inputs: Option<&Bound<'_, PyList>>,
        start_inputs: Option<&Bound<'_, PyDict>>,
        limits: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<(Self, Py<PyAny>)> {
        let input_names = list_str(inputs, "inputs")?;
        let dc_registry = DcRegistry::from_list(py, dataclass_registry)?;
        let input_values = Self::extract_repl_input_values(&input_names, start_inputs, &dc_registry)?;
        let print_callback = print_callback.map(|c| c.clone().unbind());
        let print_callback_for_create = print_callback.as_ref();
        let script_name = script_name.to_string();
        let (repl, output) = Self::create_repl(
            py,
            code,
            script_name.clone(),
            input_names,
            input_values,
            limits,
            print_callback_for_create,
        )?;

        let output = monty_to_py(py, &output, &dc_registry)?;
        let repl = Self {
            repl: Mutex::new(repl),
            print_callback,
            dc_registry,
            script_name,
        };
        Ok((repl, output))
    }

    /// Feeds and executes a single incremental REPL snippet.
    ///
    /// The snippet is compiled against existing session state and executed once
    /// without replaying previously fed snippets.
    #[pyo3(signature = (code, *, print_callback=None))]
    fn feed<'py>(&self, py: Python<'py>, code: &str, print_callback: Option<Py<PyAny>>) -> PyResult<Bound<'py, PyAny>> {
        let print_callback = print_callback.or_else(|| self.print_callback.as_ref().map(|cb| cb.clone_ref(py)));

        let mut print_cb;
        let mut print_writer = match print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb);
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        let mut repl = self
            .repl
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;

        let output = match &mut *repl {
            EitherRepl::NoLimit(repl) => repl.feed(code, &mut print_writer),
            EitherRepl::Limited(repl) => repl.feed(code, &mut print_writer),
        }
        .map_err(|e| MontyError::new_err(py, e))?;

        Ok(monty_to_py(py, &output, &self.dc_registry)?.into_bound(py))
    }

    /// Serializes this REPL session to bytes.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        #[derive(serde::Serialize)]
        struct SerializedRepl<'a> {
            repl: &'a EitherRepl,
            script_name: &'a str,
        }

        let repl = self.repl.lock().unwrap_or_else(PoisonError::into_inner);

        let serialized = SerializedRepl {
            repl: &repl,
            script_name: &self.script_name,
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Restores a REPL session from `dump()` bytes.
    #[staticmethod]
    #[pyo3(signature = (data, *, print_callback=None, dataclass_registry=None))]
    fn load(
        py: Python<'_>,
        data: &Bound<'_, PyBytes>,
        print_callback: Option<Py<PyAny>>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        #[derive(serde::Deserialize)]
        struct SerializedReplOwned {
            repl: EitherRepl,
            script_name: String,
        }

        let serialized: SerializedReplOwned =
            postcard::from_bytes(data.as_bytes()).map_err(|e| PyValueError::new_err(e.to_string()))?;

        Ok(Self {
            repl: Mutex::new(serialized.repl),
            print_callback,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            script_name: serialized.script_name,
        })
    }

    fn __repr__(&self) -> String {
        format!("MontyRepl(script_name='{}')", self.script_name)
    }
}

impl PyMontyRepl {
    /// Creates a core REPL and returns both the stored REPL state enum and initial output.
    ///
    /// This helper centralizes REPL bootstrapping for `create()`.
    fn create_repl(
        py: Python<'_>,
        code: String,
        script_name: String,
        input_names: Vec<String>,
        input_values: Vec<MontyObject>,
        limits: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Py<PyAny>>,
    ) -> PyResult<(EitherRepl, MontyObject)> {
        let mut print_cb;
        let mut print_writer = match print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb.clone_ref(py));
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            let print_writer = SendWrapper::new(&mut print_writer);
            let (repl, output) = py
                .detach(move || {
                    CoreMontyRepl::new(
                        code,
                        &script_name,
                        input_names,
                        input_values,
                        tracker,
                        print_writer.take(),
                    )
                })
                .map_err(|e| MontyError::new_err(py, e))?;
            Ok((EitherRepl::Limited(repl), output))
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            let print_writer = SendWrapper::new(&mut print_writer);
            let (repl, output) = py
                .detach(move || {
                    CoreMontyRepl::new(
                        code,
                        &script_name,
                        input_names,
                        input_values,
                        tracker,
                        print_writer.take(),
                    )
                })
                .map_err(|e| MontyError::new_err(py, e))?;
            Ok((EitherRepl::NoLimit(repl), output))
        }
    }

    /// Extracts initial input values in declaration order for direct REPL creation.
    ///
    /// This matches the same validation behavior as `Monty.start()`.
    /// Any dataclass inputs are automatically registered in the `dc_registry` via `py_to_monty`
    /// so they can be properly reconstructed on output.
    fn extract_repl_input_values(
        input_names: &[String],
        inputs: Option<&Bound<'_, PyDict>>,
        dc_registry: &DcRegistry,
    ) -> PyResult<Vec<::monty::MontyObject>> {
        if input_names.is_empty() {
            if inputs.is_some() {
                return Err(PyTypeError::new_err(
                    "No input variables declared but inputs dict was provided",
                ));
            }
            return Ok(vec![]);
        }

        let Some(inputs) = inputs else {
            return Err(PyTypeError::new_err(format!(
                "Missing required inputs: {input_names:?}"
            )));
        };

        input_names
            .iter()
            .map(|name| {
                let value = inputs
                    .get_item(name)?
                    .ok_or_else(|| PyKeyError::new_err(format!("Missing required input: '{name}'")))?;
                py_to_monty(&value, dc_registry)
            })
            .collect::<PyResult<_>>()
    }
}

/// Runtime execution snapshot, holds either a `FunctionCall` or `OsCall` for both
/// resource tracker variants since pyclass structs can't be generic.
///
/// Used internally by `PyMontySnapshot` to store execution state. Both `FunctionCall`
/// and `OsCall` have the same `resume()` signature, so we dispatch to the appropriate
/// inner type based on the variant.
///
/// The `Done` variant indicates the snapshot has been consumed.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherSnapshot {
    NoLimitFn(FunctionCall<PySignalTracker<NoLimitTracker>>),
    NoLimitOs(OsCall<PySignalTracker<NoLimitTracker>>),
    LimitedFn(FunctionCall<PySignalTracker<LimitedTracker>>),
    LimitedOs(OsCall<PySignalTracker<LimitedTracker>>),
    /// Sentinel indicating the snapshot has been consumed via `resume()`.
    Done,
}

impl EitherSnapshot {
    fn wrap_fn_no_limit(call: FunctionCall<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimitFn(call)
    }

    fn wrap_fn_limited(call: FunctionCall<PySignalTracker<LimitedTracker>>) -> Self {
        Self::LimitedFn(call)
    }

    fn wrap_os_no_limit(call: OsCall<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimitOs(call)
    }

    fn wrap_os_limited(call: OsCall<PySignalTracker<LimitedTracker>>) -> Self {
        Self::LimitedOs(call)
    }
}

#[pyclass(name = "MontySnapshot", module = "pydantic_monty")]
#[derive(Debug)]
pub struct PyMontySnapshot {
    snapshot: Mutex<EitherSnapshot>,
    print_callback: Option<Py<PyAny>>,
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
    /// The positional arguments passed to the function.
    #[pyo3(get)]
    pub args: Py<PyTuple>,
    /// The keyword arguments passed to the function (key, value pairs).
    #[pyo3(get)]
    pub kwargs: Py<PyDict>,
    /// The unique identifier for this call
    #[pyo3(get)]
    pub call_id: u32,
}

/// Extract an external result (object or exception) from a dictionary.
///
/// Any dataclass return values are automatically registered in the `dc_registry` via `py_to_monty`
/// so they can be properly reconstructed on output.
/// Extracts an `ExternalResult` from a Python dict with a single key.
///
/// Accepts `return_value`, `exception`, or `future` (with value `...`).
/// The `call_id` is required for `future` results to track the pending call.
fn extract_external_result(
    py: Python<'_>,
    dict: &Bound<'_, PyDict>,
    error_msg: &'static str,
    dc_registry: &DcRegistry,
    call_id: u32,
) -> PyResult<ExternalResult> {
    if dict.len() != 1 {
        Err(PyTypeError::new_err(error_msg))
    } else if let Some(rv) = dict.get_item(intern!(py, "return_value"))? {
        // Return value provided
        Ok(py_to_monty(&rv, dc_registry)?.into())
    } else if let Some(exc) = dict.get_item(intern!(py, "exception"))? {
        // Exception provided
        let py_err = PyErr::from_value(exc.into_any());
        Ok(exc_py_to_monty(py, &py_err).into())
    } else if let Some(exc) = dict.get_item(intern!(py, "future"))? {
        if exc.eq(py.Ellipsis()).unwrap_or_default() {
            Ok(ExternalResult::Future(call_id))
        } else {
            Err(PyTypeError::new_err(
                "Value for the 'future' key must be Ellipsis (...)",
            ))
        }
    } else {
        // wrong key in kwargs
        Err(PyTypeError::new_err(error_msg))
    }
}

#[pymethods]
impl PyMontySnapshot {
    /// Resumes execution with either a return value or an exception.
    ///
    /// Exactly one of `return_value`, `exception` or `future` must be provided as a keyword argument.
    ///
    /// # Raises
    /// * `TypeError` if both arguments are provided, or neither
    /// * `RuntimeError` if the snapshot has already been resumed
    #[pyo3(signature = (**kwargs))]
    pub fn resume<'py>(&self, py: Python<'py>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Bound<'py, PyAny>> {
        const ARGS_ERROR: &str = "resume() accepts either return_value or exception, not both";

        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Snapshot is currently being resumed by another thread"))?;

        let snapshot = std::mem::replace(&mut *snapshot, EitherSnapshot::Done);
        let Some(kwargs) = kwargs else {
            return Err(PyTypeError::new_err(ARGS_ERROR));
        };
        let external_result = extract_external_result(py, kwargs, ARGS_ERROR, &self.dc_registry, self.call_id)?;

        // Build print writer before detaching - clone_ref needs py token
        let mut print_cb;
        let print_writer = match &self.print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb.clone_ref(py));
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };
        // wrap print_writer in SendWrapper so that it can be accessed inside the py.detach calls despite
        // no `Send` bound - py.detach() is overly restrictive to prevent `Bound` types going inside
        let mut print_writer = SendWrapper::new(print_writer);

        let progress = match snapshot {
            EitherSnapshot::NoLimitFn(call) => {
                let result = py.detach(|| call.resume(external_result, &mut print_writer));
                EitherProgress::NoLimit(result.map_err(|e| MontyError::new_err(py, e))?)
            }
            EitherSnapshot::NoLimitOs(call) => {
                let result = py.detach(|| call.resume(external_result, &mut print_writer));
                EitherProgress::NoLimit(result.map_err(|e| MontyError::new_err(py, e))?)
            }
            EitherSnapshot::LimitedFn(call) => {
                let result = py.detach(|| call.resume(external_result, &mut print_writer));
                EitherProgress::Limited(result.map_err(|e| MontyError::new_err(py, e))?)
            }
            EitherSnapshot::LimitedOs(call) => {
                let result = py.detach(|| call.resume(external_result, &mut print_writer));
                EitherProgress::Limited(result.map_err(|e| MontyError::new_err(py, e))?)
            }
            EitherSnapshot::Done => return Err(PyRuntimeError::new_err("Progress already resumed")),
        };

        let dc_registry = self.dc_registry.clone_ref(py);
        progress.progress_or_complete(
            py,
            self.script_name.clone(),
            self.print_callback.as_ref().map(|cb| cb.clone_ref(py)),
            dc_registry,
        )
    }

    /// Serializes the MontySnapshot instance to a binary format.
    ///
    /// The serialized data can be stored and later restored with `MontySnapshot.load()`.
    /// This allows suspending execution and resuming later, potentially in a different process.
    ///
    /// Note: The `print_callback` is not serialized and must be re-provided when resuming
    /// after loading.
    ///
    /// # Returns
    /// Bytes containing the serialized MontySnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if serialization fails.
    /// `RuntimeError` if the progress has already been resumed.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        #[derive(serde::Serialize)]
        struct SerializedSnapshot<'a> {
            snapshot: &'a EitherSnapshot,
            script_name: &'a str,
            is_os_function: bool,
            is_method_call: bool,
            function_name: &'a str,
            args: Vec<MontyObject>,
            kwargs: Vec<(MontyObject, MontyObject)>,
            call_id: u32,
        }

        let snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(&*snapshot, EitherSnapshot::Done) {
            return Err(PyRuntimeError::new_err(
                "Cannot dump progress that has already been resumed",
            ));
        }

        // Convert Python args to MontyObject
        let args: Vec<MontyObject> = self
            .args
            .bind(py)
            .iter()
            .map(|item| py_to_monty(&item, &self.dc_registry))
            .collect::<PyResult<_>>()?;

        // Convert Python kwargs to MontyObject pairs
        let kwargs: Vec<(MontyObject, MontyObject)> = self
            .kwargs
            .bind(py)
            .iter()
            .map(|(k, v)| Ok((py_to_monty(&k, &self.dc_registry)?, py_to_monty(&v, &self.dc_registry)?)))
            .collect::<PyResult<_>>()?;

        let serialized = SerializedSnapshot {
            snapshot: &snapshot,
            script_name: &self.script_name,
            is_os_function: self.is_os_function,
            is_method_call: self.is_method_call,
            function_name: &self.function_name,
            args,
            kwargs,
            call_id: self.call_id,
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Deserializes a MontySnapshot instance from binary format.
    ///
    /// Note: The `print_callback` is not preserved during serialization and must be
    /// re-provided as a keyword argument if print output is needed.
    ///
    /// # Arguments
    /// * `data` - The serialized MontySnapshot data from `dump()`
    /// * `print_callback` - Optional callback for print output
    /// * `dataclass_registry` - Optional list of dataclasses to register
    ///
    /// # Returns
    /// A new MontySnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if deserialization fails.
    #[staticmethod]
    #[pyo3(signature = (data, *, print_callback=None, dataclass_registry=None))]
    fn load(
        py: Python<'_>,
        data: &Bound<'_, PyBytes>,
        print_callback: Option<Py<PyAny>>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        #[derive(serde::Deserialize)]
        struct SerializedSnapshotOwned {
            snapshot: EitherSnapshot,
            script_name: String,
            is_os_function: bool,
            is_method_call: bool,
            function_name: String,
            args: Vec<MontyObject>,
            kwargs: Vec<(MontyObject, MontyObject)>,
            call_id: u32,
        }

        let bytes = data.as_bytes();

        let serialized: SerializedSnapshotOwned =
            postcard::from_bytes(bytes).map_err(|e| PyValueError::new_err(e.to_string()))?;

        let dc_registry = DcRegistry::from_list(py, dataclass_registry)?;

        // Convert MontyObject args to Python
        let args: Vec<Py<PyAny>> = serialized
            .args
            .iter()
            .map(|item| monty_to_py(py, item, &dc_registry))
            .collect::<PyResult<_>>()?;

        // Convert MontyObject kwargs to Python dict
        let kwargs_dict = PyDict::new(py);
        for (k, v) in &serialized.kwargs {
            kwargs_dict.set_item(monty_to_py(py, k, &dc_registry)?, monty_to_py(py, v, &dc_registry)?)?;
        }

        Ok(Self {
            snapshot: Mutex::new(serialized.snapshot),
            print_callback,
            dc_registry,
            script_name: serialized.script_name,
            is_os_function: serialized.is_os_function,
            is_method_call: serialized.is_method_call,
            function_name: serialized.function_name,
            args: PyTuple::new(py, args)?.unbind(),
            kwargs: kwargs_dict.unbind(),
            call_id: serialized.call_id,
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "MontySnapshot(script_name='{}', function_name='{}', args={}, kwargs={})",
            self.script_name,
            self.function_name,
            self.args.bind(py).repr()?,
            self.kwargs.bind(py).repr()?
        ))
    }
}

/// Holds a `ResolveFutures` for either resource tracker variant.
///
/// Used internally by `PyMontyFutureSnapshot` to store execution state when
/// awaiting resolution of pending async external calls.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherFutureSnapshot {
    NoLimit(ResolveFutures<PySignalTracker<NoLimitTracker>>),
    Limited(ResolveFutures<PySignalTracker<LimitedTracker>>),
    /// Sentinel indicating the snapshot has been consumed via `resume()`.
    Done,
}

#[pyclass(name = "MontyFutureSnapshot", module = "pydantic_monty", frozen)]
#[derive(Debug)]
pub struct PyMontyFutureSnapshot {
    snapshot: Mutex<EitherFutureSnapshot>,
    print_callback: Option<Py<PyAny>>,
    dc_registry: DcRegistry,

    /// Name of the script being executed
    #[pyo3(get)]
    pub script_name: String,
}

#[pymethods]
impl PyMontyFutureSnapshot {
    /// Resumes execution with results for one or more futures.
    #[pyo3(signature = (results))]
    pub fn resume<'py>(&self, py: Python<'py>, results: &Bound<'_, PyDict>) -> PyResult<Bound<'py, PyAny>> {
        const ARGS_ERROR: &str = "results values must be a dict with either 'return_value' or 'exception', not both";

        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Snapshot is currently being resumed by another thread"))?;

        let snapshot = std::mem::replace(&mut *snapshot, EitherFutureSnapshot::Done);

        let external_results = results
            .iter()
            .map(|(key, value)| {
                let call_id = key.extract::<u32>()?;
                let dict = value.cast::<PyDict>()?;
                let value = extract_external_result(py, dict, ARGS_ERROR, &self.dc_registry, call_id)?;
                Ok((call_id, value))
            })
            .collect::<PyResult<Vec<_>>>()?;

        // Build print writer before detaching - clone_ref needs py token
        let mut print_cb;
        let print_writer = match &self.print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb.clone_ref(py));
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };
        let mut print_writer = SendWrapper::new(print_writer);

        let progress = match snapshot {
            EitherFutureSnapshot::NoLimit(snapshot) => {
                let result = py.detach(|| snapshot.resume(external_results, &mut print_writer));
                EitherProgress::NoLimit(result.map_err(|e| MontyError::new_err(py, e))?)
            }
            EitherFutureSnapshot::Limited(snapshot) => {
                let result = py.detach(|| snapshot.resume(external_results, &mut print_writer));
                EitherProgress::Limited(result.map_err(|e| MontyError::new_err(py, e))?)
            }
            EitherFutureSnapshot::Done => return Err(PyRuntimeError::new_err("Progress already resumed")),
        };

        // Clone the Arc handle for the next snapshot/complete
        let dc_registry = self.dc_registry.clone_ref(py);
        progress.progress_or_complete(
            py,
            self.script_name.clone(),
            self.print_callback.as_ref().map(|cb| cb.clone_ref(py)),
            dc_registry,
        )
    }

    /// Returns the pending call IDs associated with the MontyFutureSnapshot instance.
    ///
    /// # Returns
    /// A slice of pending call IDs.
    #[getter]
    fn pending_call_ids<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        match &*snapshot {
            EitherFutureSnapshot::NoLimit(snapshot) => PyList::new(py, snapshot.pending_call_ids()),
            EitherFutureSnapshot::Limited(snapshot) => PyList::new(py, snapshot.pending_call_ids()),
            EitherFutureSnapshot::Done => Err(PyRuntimeError::new_err("MontyFutureSnapshot already resumed")),
        }
    }

    /// Serializes the MontyFutureSnapshot instance to a binary format.
    ///
    /// The serialized data can be stored and later restored with `MontyFutureSnapshot.load()`.
    /// This allows suspending execution and resuming later, potentially in a different process.
    ///
    /// Note: The `print_callback` is not serialized and must be re-provided when resuming
    /// after loading.
    ///
    /// # Returns
    /// Bytes containing the serialized MontyFutureSnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if serialization fails.
    /// `RuntimeError` if the progress has already been resumed.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        #[derive(serde::Serialize)]
        struct SerializedSnapshot<'a> {
            snapshot: &'a EitherFutureSnapshot,
            script_name: &'a str,
        }

        let snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(&*snapshot, EitherFutureSnapshot::Done) {
            return Err(PyRuntimeError::new_err(
                "Cannot dump progress that has already been resumed",
            ));
        }

        let serialized = SerializedSnapshot {
            snapshot: &snapshot,
            script_name: &self.script_name,
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Deserializes a MontyFutureSnapshot instance from binary format.
    ///
    /// Note: The `print_callback` is not preserved during serialization and must be
    /// re-provided as a keyword argument if print output is needed.
    ///
    /// # Arguments
    /// * `data` - The serialized MontyFutureSnapshot data from `dump()`
    /// * `print_callback` - Optional callback for print output
    /// * `dataclass_registry` - Optional list of dataclasses to register
    ///
    /// # Returns
    /// A new MontyFutureSnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if deserialization fails.
    #[staticmethod]
    #[pyo3(signature = (data, *, print_callback=None, dataclass_registry=None))]
    fn load(
        py: Python<'_>,
        data: &Bound<'_, PyBytes>,
        print_callback: Option<Py<PyAny>>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        #[derive(serde::Deserialize)]
        struct SerializedSnapshotOwned {
            snapshot: EitherFutureSnapshot,
            script_name: String,
        }

        let bytes = data.as_bytes();

        let serialized: SerializedSnapshotOwned =
            postcard::from_bytes(bytes).map_err(|e| PyValueError::new_err(e.to_string()))?;

        Ok(Self {
            snapshot: Mutex::new(serialized.snapshot),
            print_callback,
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            script_name: serialized.script_name,
        })
    }

    fn __repr__(&self) -> String {
        let snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        let pending_call_ids = match &*snapshot {
            EitherFutureSnapshot::NoLimit(snapshot) => snapshot.pending_call_ids(),
            EitherFutureSnapshot::Limited(snapshot) => snapshot.pending_call_ids(),
            EitherFutureSnapshot::Done => &[],
        };
        format!(
            "MontyFutureSnapshot(script_name='{}', pending_call_ids={pending_call_ids:?})",
            self.script_name,
        )
    }
}

#[pyclass(name = "MontyComplete", module = "pydantic_monty", frozen)]
pub struct PyMontyComplete {
    #[pyo3(get)]
    pub output: Py<PyAny>,
    // TODO we might want to add stats on execution here like time, allocations, etc.
}

impl PyMontyComplete {
    fn create<'py>(py: Python<'py>, output: &MontyObject, dc_registry: &DcRegistry) -> PyResult<Bound<'py, PyAny>> {
        let output = monty_to_py(py, output, dc_registry)?;
        let slf = Self { output };
        slf.into_bound_py_any(py)
    }
}

#[pymethods]
impl PyMontyComplete {
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("MontyComplete(output={})", self.output.bind(py).repr()?))
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

/// A `PrintWriter` implementation that calls a Python callback for each print output.
///
/// This struct holds a GIL-independent `Py<PyAny>` reference to the callback,
/// allowing it to be used across GIL release boundaries. The GIL is re-acquired
/// briefly for each callback invocation.
#[derive(Debug)]
pub struct CallbackStringPrint(Py<PyAny>);

impl CallbackStringPrint {
    /// Creates a new `CallbackStringPrint` from a borrowed Python callback.
    fn new(callback: &Bound<'_, PyAny>) -> Self {
        Self(callback.clone().unbind())
    }

    /// Creates a new `CallbackStringPrint` from an owned `Py<PyAny>`.
    fn from_py(callback: Py<PyAny>) -> Self {
        Self(callback)
    }
}

impl PrintWriterCallback for CallbackStringPrint {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        Python::attach(|py| {
            self.0.bind(py).call1(("stdout", output.as_ref()))?;
            Ok::<_, PyErr>(())
        })
        .map_err(|e| Python::attach(|py| exc_py_to_monty(py, &e)))
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        Python::attach(|py| {
            self.0.bind(py).call1(("stdout", end.to_string()))?;
            Ok::<_, PyErr>(())
        })
        .map_err(|e| Python::attach(|py| exc_py_to_monty(py, &e)))
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
