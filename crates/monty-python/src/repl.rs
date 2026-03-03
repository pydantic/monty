use std::sync::{Mutex, PoisonError};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{LimitedTracker, MontyObject, MontyRepl as CoreMontyRepl, NoLimitTracker, PrintWriter};
use pyo3::{
    exceptions::{PyKeyError, PyRuntimeError, PyTypeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};
use send_wrapper::SendWrapper;

use crate::{
    convert::{monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    exceptions::MontyError,
    limits::{PySignalTracker, extract_limits},
    monty_cls::{CallbackStringPrint, list_str},
};

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
            EitherRepl::NoLimit(repl) => repl.feed_run(code, vec![], &mut print_writer),
            EitherRepl::Limited(repl) => repl.feed_run(code, vec![], &mut print_writer),
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

        let inputs: Vec<(String, MontyObject)> = input_names.into_iter().zip(input_values).collect();

        if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            let print_writer = SendWrapper::new(&mut print_writer);
            let (repl, output) = py
                .detach(move || {
                    let mut repl = CoreMontyRepl::new(&script_name, tracker);
                    let output = repl.feed_run(&code, inputs, print_writer.take())?;
                    Ok((repl, output))
                })
                .map_err(|e| MontyError::new_err(py, e))?;
            Ok((EitherRepl::Limited(repl), output))
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            let print_writer = SendWrapper::new(&mut print_writer);
            let (repl, output) = py
                .detach(move || {
                    let mut repl = CoreMontyRepl::new(&script_name, tracker);
                    let output = repl.feed_run(&code, inputs, print_writer.take())?;
                    Ok((repl, output))
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
