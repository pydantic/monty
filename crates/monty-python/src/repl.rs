use std::sync::{Mutex, PoisonError};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{LimitedTracker, MontyRepl as CoreMontyRepl, NoLimitTracker, PrintWriter};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};

use crate::{
    convert::monty_to_py,
    dataclass::DcRegistry,
    exceptions::MontyError,
    limits::{PySignalTracker, extract_limits},
    monty_cls::CallbackStringPrint,
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

/// Stateful no-replay REPL session.
///
/// Create with `MontyRepl()` then call `feed()` to execute snippets
/// incrementally against persistent heap and namespace state.
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
    /// Creates an empty REPL session ready to receive snippets via `feed()`.
    ///
    /// No code is parsed or executed at construction time — all execution
    /// is driven through `feed()`.
    #[new]
    #[pyo3(signature = (*, script_name="main.py", limits=None, print_callback=None, dataclass_registry=None))]
    fn new(
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
        print_callback: Option<&Bound<'_, PyAny>>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        let dc_registry = DcRegistry::from_list(py, dataclass_registry)?;
        let print_callback = print_callback.map(|c| c.clone().unbind());
        let script_name = script_name.to_string();

        let repl = if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            EitherRepl::Limited(CoreMontyRepl::new(&script_name, tracker))
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            EitherRepl::NoLimit(CoreMontyRepl::new(&script_name, tracker))
        };

        Ok(Self {
            repl: Mutex::new(repl),
            print_callback,
            dc_registry,
            script_name,
        })
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
