use std::{borrow::Cow, fmt::Write};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{
    ExternalResult, LimitedTracker, MontyException, MontyObject, MontyRun, NoLimitTracker, PrintWriter,
    ResourceTracker, RunProgress, Snapshot, StdPrint,
};
use pyo3::{
    exceptions::{PyKeyError, PyRuntimeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyTuple},
    IntoPyObjectExt,
};

use crate::{
    convert::{monty_to_py, py_to_monty},
    exceptions::{exc_monty_to_py, exc_py_to_monty},
    external::ExternalFunctionRegistry,
    limits::{extract_limits, PySignalTracker},
};

/// A sandboxed Python interpreter instance.
///
/// Parses and compiles Python code on initialization, then can be run
/// multiple times with different input values. This separates the parsing
/// cost from execution, making repeated runs more efficient.
#[pyclass(name = "Monty", module = "monty")]
#[derive(Debug)]
pub struct PyMonty {
    /// The compiled code snapshot, ready to execute.
    runner: MontyRun,
    /// The artificial name of the python code "file"
    script_name: String,
    /// Names of input variables expected by the code.
    input_names: Vec<String>,
    /// Names of external functions the code can call.
    external_function_names: Vec<String>,
}

#[pymethods]
impl PyMonty {
    /// Creates a new Monty interpreter by parsing the given code.
    ///
    /// # Arguments
    /// * `code` - Python code to execute
    /// * `inputs` - List of input variable names available in the code
    /// * `external_functions` - List of external function names the code can call
    ///
    /// # Raises
    /// `SyntaxError` if the code cannot be parsed
    #[new]
    #[pyo3(signature = (code, *, script_name="main.py", inputs=None, external_functions=None))]
    fn new(
        py: Python<'_>,
        code: String,
        script_name: &str,
        inputs: Option<&Bound<'_, PyList>>,
        external_functions: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        let input_names = list_str(inputs, "inputs")?;
        let external_function_names = list_str(external_functions, "external_functions")?;

        // Create the snapshot (parses the code)
        let runner = MontyRun::new(code, script_name, input_names.clone(), external_function_names.clone())
            .map_err(|e| exc_monty_to_py(py, e))?;

        Ok(Self {
            runner,
            script_name: script_name.to_string(),
            input_names,
            external_function_names,
        })
    }

    /// Executes the code and returns the result.
    ///
    /// # Arguments
    /// * `inputs` - Dict of input variable values (must match names from `__init__`)
    /// * `limits` - Optional `ResourceLimits` configuration
    /// * `external_functions` - Dict of external function callbacks (must match names from `__init__`)
    ///
    /// # Returns
    /// The result of the last expression in the code
    ///
    /// # Raises
    /// Various Python exceptions matching what the code would raise
    #[pyo3(signature = (*, inputs=None, limits=None, external_functions=None, print_callback=None))]
    fn run(
        &self,
        py: Python<'_>,
        inputs: Option<&Bound<'_, PyDict>>,
        limits: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        // Extract input values in the order they were declared
        let input_values = self.extract_input_values(inputs)?;

        // Use SignalCheckingTracker when limits are provided, NoLimitTracker otherwise
        match (limits, print_callback) {
            (Some(limits), Some(callback)) => {
                let inner_tracker = LimitedTracker::new(extract_limits(limits)?);
                let tracker = PySignalTracker::new(inner_tracker);
                self.run_with_tracker(
                    py,
                    input_values,
                    tracker,
                    external_functions,
                    CallbackStringPrint(callback),
                )
            }
            (Some(limits), None) => {
                let inner_tracker = LimitedTracker::new(extract_limits(limits)?);
                let tracker = PySignalTracker::new(inner_tracker);
                self.run_with_tracker(py, input_values, tracker, external_functions, StdPrint)
            }
            (None, Some(callback)) => self.run_with_tracker(
                py,
                input_values,
                PySignalTracker::new(NoLimitTracker::default()),
                external_functions,
                CallbackStringPrint(callback),
            ),
            (None, None) => self.run_with_tracker(
                py,
                input_values,
                PySignalTracker::new(NoLimitTracker::default()),
                external_functions,
                StdPrint,
            ),
        }
    }

    #[pyo3(signature = (*, inputs=None, limits=None, print_callback=None))]
    fn start<'py>(
        &self,
        py: Python<'py>,
        inputs: Option<&Bound<'py, PyDict>>,
        limits: Option<&Bound<'py, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Extract input values in the order they were declared
        let input_values = self.extract_input_values(inputs)?;

        macro_rules! start {
            ($resource_tracker:expr, $print_output:expr) => {
                // Clone the runner since start() consumes it - allows reuse of the parsed code
                self.runner
                    .clone()
                    .start(input_values, $resource_tracker, &mut $print_output)
                    .map_err(|e| exc_monty_to_py(py, e))?
            };
        }

        // separate code paths due to generics
        let progress = match (limits, print_callback) {
            (Some(limits), Some(callback)) => EitherProgress::Limited(start!(
                LimitedTracker::new(extract_limits(limits)?),
                CallbackStringPrint(callback)
            )),
            (Some(limits), None) => {
                EitherProgress::Limited(start!(LimitedTracker::new(extract_limits(limits)?), StdPrint))
            }
            (None, Some(callback)) => {
                EitherProgress::NoLimit(start!(NoLimitTracker::default(), CallbackStringPrint(callback)))
            }
            (None, None) => EitherProgress::NoLimit(start!(NoLimitTracker::default(), StdPrint)),
        };
        progress.progress_or_complete(py, self.script_name.clone(), print_callback.map(|c| c.clone().unbind()))
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
        if !self.external_function_names.is_empty() {
            write!(s, ", external_functions={:?}", self.external_function_names).unwrap();
        }
        s.push(')');
        s
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
            external_function_names: self.external_function_names.clone(),
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Deserializes a Monty instance from binary format.
    ///
    /// # Arguments
    /// * `data` - The serialized Monty data from `dump()`
    ///
    /// # Returns
    /// A new Monty instance.
    ///
    /// # Raises
    /// `ValueError` if deserialization fails.
    #[staticmethod]
    fn load(data: &Bound<'_, PyBytes>) -> PyResult<Self> {
        let bytes = data.as_bytes();
        let serialized: SerializedMonty =
            postcard::from_bytes(bytes).map_err(|e| PyValueError::new_err(e.to_string()))?;

        Ok(Self {
            runner: serialized.runner,
            script_name: serialized.script_name,
            input_names: serialized.input_names,
            external_function_names: serialized.external_function_names,
        })
    }
}

impl PyMonty {
    /// Extracts input values from the dict in the order they were declared.
    ///
    /// Validates that all required inputs are provided and no extra inputs are given.
    fn extract_input_values(&self, inputs: Option<&Bound<'_, PyDict>>) -> PyResult<Vec<::monty::MontyObject>> {
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
                py_to_monty(&value)
            })
            .collect::<PyResult<_>>()
    }

    /// Runs code with a generic tracker.
    fn run_with_tracker(
        &self,
        py: Python<'_>,
        input_values: Vec<MontyObject>,
        tracker: impl ResourceTracker,
        external_functions: Option<&Bound<'_, PyDict>>,
        mut print_output: impl PrintWriter,
    ) -> PyResult<Py<PyAny>> {
        if self.external_function_names.is_empty() {
            match self.runner.run(input_values, tracker, &mut print_output) {
                Ok(v) => monty_to_py(py, &v),
                Err(err) => Err(exc_monty_to_py(py, err)),
            }
        } else {
            // Clone the runner since start() consumes it - allows reuse of the parsed code
            let progress = self
                .runner
                .clone()
                .start(input_values, tracker, &mut print_output)
                .map_err(|e| exc_monty_to_py(py, e))?;
            execute_progress(py, progress, external_functions, &mut print_output)
        }
    }
}

/// pyclass doesn't support generic types, hence hard coding the generics
#[derive(Debug)]
enum EitherProgress {
    NoLimit(RunProgress<NoLimitTracker>),
    Limited(RunProgress<LimitedTracker>),
}

impl EitherProgress {
    fn progress_or_complete(
        self,
        py: Python<'_>,
        script_name: String,
        print_callback: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (function_name, args, kwargs, snapshot) = match self {
            EitherProgress::NoLimit(p) => match p {
                RunProgress::Complete(result) => return PyMontyComplete::create(py, &result),
                RunProgress::FunctionCall {
                    function_name,
                    args,
                    kwargs,
                    state,
                } => (function_name, args, kwargs, EitherSnapshot::NoLimit(state)),
            },
            EitherProgress::Limited(p) => match p {
                RunProgress::Complete(result) => return PyMontyComplete::create(py, &result),
                RunProgress::FunctionCall {
                    function_name,
                    args,
                    kwargs,
                    state,
                } => (function_name, args, kwargs, EitherSnapshot::Limited(state)),
            },
        };

        let items: PyResult<Vec<Py<PyAny>>> = args.iter().map(|item| monty_to_py(py, item)).collect();

        let dict = PyDict::new(py);
        for (k, v) in &kwargs {
            dict.set_item(monty_to_py(py, k)?, monty_to_py(py, v)?)?;
        }

        let slf = PyMontySnapshot {
            snapshot,
            print_callback: print_callback.map(|callback| callback.clone_ref(py)),
            script_name,
            function_name,
            args: PyTuple::new(py, items?)?.unbind(),
            kwargs: dict.unbind(),
        };
        slf.into_bound_py_any(py)
    }
}

/// Runtime execution snapshot, parameterized by resource tracker type.
///
/// Used internally by `PyMontySnapshot` to store execution state.
/// The `Done` variant indicates the snapshot has been consumed.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherSnapshot {
    NoLimit(Snapshot<NoLimitTracker>),
    Limited(Snapshot<LimitedTracker>),
    /// Done is used when taking the snapshot to run it
    /// should only be done after execution is complete
    Done,
}

#[pyclass(name = "MontySnapshot", module = "monty")]
#[derive(Debug)]
pub struct PyMontySnapshot {
    snapshot: EitherSnapshot,
    print_callback: Option<Py<PyAny>>,

    /// Name of the script being executed
    #[pyo3(get)]
    pub script_name: String,

    /// The name of the function being called.
    #[pyo3(get)]
    pub function_name: String,
    /// The positional arguments passed to the function.
    #[pyo3(get)]
    pub args: Py<PyTuple>,
    /// The keyword arguments passed to the function (key, value pairs).
    #[pyo3(get)]
    pub kwargs: Py<PyDict>,
}

#[pymethods]
impl PyMontySnapshot {
    /// Resumes execution with either a return value or an exception.
    ///
    /// Exactly one of `return_value` or `exception` must be provided.
    ///
    /// # Arguments
    /// * `return_value` - The value to return from the external function call
    /// * `exception` - An exception to raise in the Monty interpreter
    ///
    /// # Raises
    /// * `TypeError` if both arguments are provided, or neither
    /// * `RuntimeError` if the snapshot has already been resumed
    #[pyo3(signature = (**kwargs))]
    pub fn resume<'py>(&mut self, py: Python<'py>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Bound<'py, PyAny>> {
        const ARGS_ERROR: &str = "resume() accepts either return_value or exception, not both";
        let Some(kwargs) = kwargs else {
            return Err(PyTypeError::new_err(ARGS_ERROR));
        };
        if kwargs.len() != 1 {
            return Err(PyTypeError::new_err(ARGS_ERROR));
        }
        let external_result: ExternalResult = if let Some(rv) = kwargs.get_item(intern!(py, "return_value"))? {
            // Return value provided
            py_to_monty(&rv)?.into()
        } else if let Some(exc) = kwargs.get_item(intern!(py, "exception"))? {
            // Exception provided
            let py_err = PyErr::from_value(exc.into_any());
            exc_py_to_monty(py, py_err).into()
        } else {
            // wrong key in kwargs
            return Err(PyTypeError::new_err(ARGS_ERROR));
        };

        let snapshot = std::mem::replace(&mut self.snapshot, EitherSnapshot::Done);
        let progress = match snapshot {
            EitherSnapshot::NoLimit(snapshot) => {
                let result = if let Some(print_callback) = &self.print_callback {
                    snapshot.run(external_result, &mut CallbackStringPrint(print_callback.bind(py)))
                } else {
                    snapshot.run(external_result, &mut StdPrint)
                };
                EitherProgress::NoLimit(result.map_err(|e| exc_monty_to_py(py, e))?)
            }
            EitherSnapshot::Limited(snapshot) => {
                let result = if let Some(print_callback) = &self.print_callback {
                    snapshot.run(external_result, &mut CallbackStringPrint(print_callback.bind(py)))
                } else {
                    snapshot.run(external_result, &mut StdPrint)
                };
                EitherProgress::Limited(result.map_err(|e| exc_monty_to_py(py, e))?)
            }
            EitherSnapshot::Done => return Err(PyRuntimeError::new_err("Progress already resumed")),
        };

        progress.progress_or_complete(py, self.script_name.clone(), self.print_callback.take())
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
        if matches!(self.snapshot, EitherSnapshot::Done) {
            return Err(PyRuntimeError::new_err(
                "Cannot dump progress that has already been resumed",
            ));
        }

        // Convert Python args to MontyObject
        let args: Vec<MontyObject> = self
            .args
            .bind(py)
            .iter()
            .map(|item| py_to_monty(&item))
            .collect::<PyResult<_>>()?;

        // Convert Python kwargs to MontyObject pairs
        let kwargs: Vec<(MontyObject, MontyObject)> = self
            .kwargs
            .bind(py)
            .iter()
            .map(|(k, v)| Ok((py_to_monty(&k)?, py_to_monty(&v)?)))
            .collect::<PyResult<_>>()?;

        let serialized = SerializedProgress {
            snapshot: &self.snapshot,
            script_name: &self.script_name,
            function_name: &self.function_name,
            args,
            kwargs,
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
    ///
    /// # Returns
    /// A new MontySnapshot instance.
    ///
    /// # Raises
    /// `ValueError` if deserialization fails.
    #[staticmethod]
    #[pyo3(signature = (data, *, print_callback=None))]
    fn load(py: Python<'_>, data: &Bound<'_, PyBytes>, print_callback: Option<Py<PyAny>>) -> PyResult<Self> {
        let bytes = data.as_bytes();
        let serialized: SerializedProgressOwned =
            postcard::from_bytes(bytes).map_err(|e| PyValueError::new_err(e.to_string()))?;

        // Convert MontyObject args to Python
        let args: Vec<Py<PyAny>> = serialized
            .args
            .iter()
            .map(|item| monty_to_py(py, item))
            .collect::<PyResult<_>>()?;

        // Convert MontyObject kwargs to Python dict
        let kwargs_dict = PyDict::new(py);
        for (k, v) in &serialized.kwargs {
            kwargs_dict.set_item(monty_to_py(py, k)?, monty_to_py(py, v)?)?;
        }

        Ok(Self {
            snapshot: serialized.snapshot,
            print_callback,
            script_name: serialized.script_name,
            function_name: serialized.function_name,
            args: PyTuple::new(py, args)?.unbind(),
            kwargs: kwargs_dict.unbind(),
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

#[pyclass(name = "MontyComplete", module = "monty")]
pub struct PyMontyComplete {
    #[pyo3(get)]
    pub output: Py<PyAny>,
    // TODO we might want to add stats on execution here like time, allocations, etc.
}

impl PyMontyComplete {
    fn create<'py>(py: Python<'py>, output: &MontyObject) -> PyResult<Bound<'py, PyAny>> {
        let output = monty_to_py(py, output)?;
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

/// Executes the `RunProgress` loop, handling external function calls.
///
/// Checks for pending Python signals (e.g., Ctrl+C) after execution completes.
fn execute_progress(
    py: Python<'_>,
    mut progress: RunProgress<impl ResourceTracker>,
    external_functions: Option<&Bound<'_, PyDict>>,
    print_output: &mut impl PrintWriter,
) -> PyResult<Py<PyAny>> {
    loop {
        match progress {
            RunProgress::Complete(result) => return monty_to_py(py, &result),
            RunProgress::FunctionCall {
                function_name,
                args,
                kwargs,
                state,
            } => {
                let registry = external_functions
                    .map(|d| ExternalFunctionRegistry::new(py, d))
                    .ok_or_else(|| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "External function '{function_name}' called but no external_functions provided"
                        ))
                    })?;

                let return_value = registry.call(&function_name, &args, &kwargs);

                progress = state
                    .run(return_value, print_output)
                    .map_err(|e| exc_monty_to_py(py, e))?;
            }
        }
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

#[derive(Debug)]
pub struct CallbackStringPrint<'py>(&'py Bound<'py, PyAny>);

impl<'py> CallbackStringPrint<'py> {
    fn write(&mut self, output: impl IntoPyObject<'py>) -> PyResult<()> {
        self.0.call1(("stdout", output))?;
        Ok(())
    }
}

impl PrintWriter for CallbackStringPrint<'_> {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.write(output).map_err(|e| exc_py_to_monty(self.0.py(), e))
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.write(end).map_err(|e| exc_py_to_monty(self.0.py(), e))
    }
}

/// Serialization wrapper for `PyMonty` that includes all fields needed for reconstruction.
#[derive(serde::Serialize, serde::Deserialize)]
struct SerializedMonty {
    runner: MontyRun,
    script_name: String,
    input_names: Vec<String>,
    external_function_names: Vec<String>,
}

/// Serialization wrapper for `PyMontySnapshot` that uses borrowed references for efficiency.
///
/// Used during `dump()` to avoid unnecessary cloning.
#[derive(serde::Serialize)]
struct SerializedProgress<'a> {
    snapshot: &'a EitherSnapshot,
    script_name: &'a str,
    function_name: &'a str,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
}

/// Owned version of `SerializedProgress` for deserialization.
///
/// Used during `load()` to own all the deserialized data.
#[derive(serde::Deserialize)]
struct SerializedProgressOwned {
    snapshot: EitherSnapshot,
    script_name: String,
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
}
