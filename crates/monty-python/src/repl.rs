use std::sync::{Mutex, PoisonError};

// Use `::monty` to refer to the external crate (not the pymodule)
use ::monty::{
    ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRepl as CoreMontyRepl, NameLookupResult,
    NoLimitTracker, PrintWriter, ReplProgress, ReplStartError, ResourceTracker,
};
use monty::ExcType;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyTuple},
};
use send_wrapper::SendWrapper;

use crate::{
    convert::{get_docstring, monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    exceptions::{MontyError, exc_py_to_monty},
    external::{ExternalFunctionRegistry, dispatch_method_call},
    limits::{PySignalTracker, extract_limits},
    monty_cls::CallbackStringPrint,
};

/// Runtime REPL session holder for pyclass interoperability.
///
/// PyO3 classes cannot be generic, so this enum stores REPL sessions for both
/// resource tracker variants.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum EitherRepl {
    NoLimit(CoreMontyRepl<PySignalTracker<NoLimitTracker>>),
    Limited(CoreMontyRepl<PySignalTracker<LimitedTracker>>),
}

/// Stateful no-replay REPL session.
///
/// Create with `MontyRepl()` then call `feed_run()` to execute snippets
/// incrementally against persistent heap and namespace state.
///
/// Uses `Mutex` for the inner REPL because `CoreMontyRepl` contains a `Heap`
/// with `Cell<usize>` (not `Sync`), and PyO3 requires `Send + Sync` for all
/// pyclass types. The mutex also prevents concurrent `feed_run()` calls.
#[pyclass(name = "MontyRepl", module = "pydantic_monty", frozen)]
#[derive(Debug)]
pub struct PyMontyRepl {
    repl: Mutex<Option<EitherRepl>>,
    dc_registry: DcRegistry,

    /// Name of the script being executed.
    #[pyo3(get)]
    pub script_name: String,
}

#[pymethods]
impl PyMontyRepl {
    /// Creates an empty REPL session ready to receive snippets via `feed_run()`.
    ///
    /// No code is parsed or executed at construction time — all execution
    /// is driven through `feed_run()`.
    #[new]
    #[pyo3(signature = (*, script_name="main.py", limits=None, dataclass_registry=None))]
    fn new(
        py: Python<'_>,
        script_name: &str,
        limits: Option<&Bound<'_, PyDict>>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        let dc_registry = DcRegistry::from_list(py, dataclass_registry)?;
        let script_name = script_name.to_string();

        let repl = if let Some(limits) = limits {
            let tracker = PySignalTracker::new(LimitedTracker::new(extract_limits(limits)?));
            EitherRepl::Limited(CoreMontyRepl::new(&script_name, tracker))
        } else {
            let tracker = PySignalTracker::new(NoLimitTracker);
            EitherRepl::NoLimit(CoreMontyRepl::new(&script_name, tracker))
        };

        Ok(Self {
            repl: Mutex::new(Some(repl)),
            dc_registry,
            script_name,
        })
    }

    /// Registers a dataclass type for proper isinstance() support on output.
    fn register_dataclass(&self, cls: &Bound<'_, pyo3::types::PyType>) -> PyResult<()> {
        self.dc_registry.insert(cls)
    }

    /// Feeds and executes a single incremental REPL snippet.
    ///
    /// The snippet is compiled against existing session state and executed once
    /// without replaying previously fed snippets.
    ///
    /// When `external_functions` is provided, external function calls and name
    /// lookups are dispatched to the provided callables — matching the behavior
    /// of `Monty.run(external_functions=...)`.
    #[pyo3(signature = (code, *, inputs=None, external_functions=None, print_callback=None, os=None))]
    fn feed_run<'py>(
        &self,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        external_functions: Option<&Bound<'_, PyDict>>,
        print_callback: Option<Py<PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input_values = extract_repl_inputs(inputs, &self.dc_registry)?;

        let mut print_cb;
        let mut print_writer = match print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb);
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        if external_functions.is_some() || os.is_some() {
            return self.feed_run_with_externals(py, code, input_values, external_functions, os, print_writer);
        }

        let mut guard = self
            .repl
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;
        let repl = guard
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;

        let output = match repl {
            EitherRepl::NoLimit(repl) => repl.feed_run(code, input_values, print_writer.reborrow()),
            EitherRepl::Limited(repl) => repl.feed_run(code, input_values, print_writer.reborrow()),
        }
        .map_err(|e| MontyError::new_err(py, e))?;

        Ok(monty_to_py(py, &output, &self.dc_registry)?.into_bound(py))
    }

    /// Starts executing an incremental snippet, yielding snapshots for external calls.
    ///
    /// Unlike `feed_run()`, which handles external function dispatch internally via a loop,
    /// `feed_start()` returns a snapshot object whenever the code needs an external function
    /// call, OS call, name lookup, or future resolution. The caller then provides the result
    /// via `snapshot.resume(...)`, which returns the next snapshot or `MontyComplete`.
    ///
    /// This enables the same iterative start/resume pattern used by `Monty.start()`,
    /// including support for async external functions via `FutureSnapshot`.
    #[pyo3(signature = (code, *, inputs=None, print_callback=None))]
    fn feed_start<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        code: &str,
        inputs: Option<&Bound<'_, PyDict>>,
        print_callback: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.get();
        let input_values = extract_repl_inputs(inputs, &this.dc_registry)?;

        let mut print_cb;
        let print_writer = match &print_callback {
            Some(cb) => {
                print_cb = CallbackStringPrint::from_py(cb.clone_ref(py));
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };
        let mut print_output = SendWrapper::new(print_writer);

        let repl = this.take_repl()?;
        let repl_owner: Py<Self> = slf.clone().unbind();

        let code_owned = code.to_owned();
        let inputs_owned = input_values;
        let dc_registry = this.dc_registry.clone_ref(py);
        let script_name = this.script_name.clone();

        match repl {
            EitherRepl::NoLimit(repl) => {
                let progress = py
                    .detach(|| repl.feed_start(&code_owned, inputs_owned, print_output.reborrow()))
                    .map_err(|e| this.restore_repl_from_start_error(py, *e))?;
                let either = crate::monty_cls::EitherProgress::ReplNoLimit(progress, repl_owner);
                either.progress_or_complete(py, script_name, print_callback, dc_registry)
            }
            EitherRepl::Limited(repl) => {
                let progress = py
                    .detach(|| repl.feed_start(&code_owned, inputs_owned, print_output.reborrow()))
                    .map_err(|e| this.restore_repl_from_start_error(py, *e))?;
                let either = crate::monty_cls::EitherProgress::ReplLimited(progress, repl_owner);
                either.progress_or_complete(py, script_name, print_callback, dc_registry)
            }
        }
    }

    /// Serializes this REPL session to bytes.
    fn dump<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        #[derive(serde::Serialize)]
        struct SerializedRepl<'a> {
            repl: &'a EitherRepl,
            script_name: &'a str,
        }

        let guard = self.repl.lock().unwrap_or_else(PoisonError::into_inner);
        let repl = guard
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;

        let serialized = SerializedRepl {
            repl,
            script_name: &self.script_name,
        };
        let bytes = postcard::to_allocvec(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Restores a REPL session from `dump()` bytes.
    #[staticmethod]
    #[pyo3(signature = (data, *, dataclass_registry=None))]
    fn load(
        py: Python<'_>,
        data: &Bound<'_, PyBytes>,
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
            repl: Mutex::new(Some(serialized.repl)),
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
            script_name: serialized.script_name,
        })
    }

    fn __repr__(&self) -> String {
        format!("MontyRepl(script_name='{}')", self.script_name)
    }
}

impl PyMontyRepl {
    /// Executes a REPL snippet with external function and OS call support.
    ///
    /// Uses the iterative `feed_start` / resume loop to handle external function
    /// calls and name lookups, matching the same dispatch logic as `Monty.run()`.
    ///
    /// `feed_start` consumes the REPL, so we temporarily take it out of the mutex
    /// (leaving `None`) and restore it on both success and error paths.
    fn feed_run_with_externals<'py>(
        &self,
        py: Python<'py>,
        code: &str,
        input_values: Vec<(String, MontyObject)>,
        external_functions: Option<&Bound<'_, PyDict>>,
        os: Option<&Bound<'_, PyAny>>,
        mut print_writer: PrintWriter<'_>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut print_output = SendWrapper::new(&mut print_writer);

        let repl = self.take_repl()?;

        let result = match repl {
            EitherRepl::NoLimit(repl) => {
                self.feed_start_loop(py, repl, code, input_values, external_functions, os, &mut print_output)
            }
            EitherRepl::Limited(repl) => {
                self.feed_start_loop(py, repl, code, input_values, external_functions, os, &mut print_output)
            }
        };

        // On error, the REPL is already restored inside `restore_repl_from_start_error`.
        match result {
            Ok((output, restored_repl)) => {
                self.put_repl(restored_repl);
                Ok(monty_to_py(py, &output, &self.dc_registry)?.into_bound(py))
            }
            Err(err) => Err(err),
        }
    }

    /// Runs the feed_start / resume loop for a specific resource tracker type.
    ///
    /// Returns the output value and the restored REPL enum variant, or a Python error.
    #[expect(clippy::too_many_arguments)]
    fn feed_start_loop<T: ResourceTracker + Send>(
        &self,
        py: Python<'_>,
        repl: CoreMontyRepl<T>,
        code: &str,
        input_values: Vec<(String, MontyObject)>,
        external_functions: Option<&Bound<'_, PyDict>>,
        os: Option<&Bound<'_, PyAny>>,
        print_output: &mut SendWrapper<&mut PrintWriter<'_>>,
    ) -> PyResult<(MontyObject, EitherRepl)>
    where
        EitherRepl: FromCoreRepl<T>,
    {
        let code_owned = code.to_owned();
        let mut progress = py
            .detach(|| repl.feed_start(&code_owned, input_values, print_output.reborrow()))
            .map_err(|e| self.restore_repl_from_start_error(py, *e))?;

        loop {
            match progress {
                ReplProgress::Complete { repl, value } => {
                    return Ok((value, EitherRepl::from_core(repl)));
                }
                ReplProgress::FunctionCall(call) => {
                    let return_value = if call.method_call {
                        dispatch_method_call(py, &call.function_name, &call.args, &call.kwargs, &self.dc_registry)
                    } else if let Some(ext_fns) = external_functions {
                        let registry = ExternalFunctionRegistry::new(py, ext_fns, &self.dc_registry);
                        registry.call(&call.function_name, &call.args, &call.kwargs)
                    } else {
                        let msg = format!(
                            "External function '{}' called but no external_functions provided",
                            call.function_name
                        );
                        self.put_repl(EitherRepl::from_core(call.into_repl()));
                        return Err(PyRuntimeError::new_err(msg));
                    };

                    progress = py
                        .detach(|| call.resume(return_value, print_output.reborrow()))
                        .map_err(|e| self.restore_repl_from_start_error(py, *e))?;
                }
                ReplProgress::NameLookup(lookup) => {
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

                    progress = py
                        .detach(|| lookup.resume(result, print_output.reborrow()))
                        .map_err(|e| self.restore_repl_from_start_error(py, *e))?;
                }
                ReplProgress::OsCall(call) => {
                    let result: ExtFunctionResult = if let Some(os_callback) = os {
                        let py_args: Vec<Py<PyAny>> = call
                            .args
                            .iter()
                            .map(|arg| monty_to_py(py, arg, &self.dc_registry))
                            .collect::<PyResult<_>>()?;
                        let py_args_tuple = PyTuple::new(py, py_args)?;

                        let py_kwargs = PyDict::new(py);
                        for (k, v) in &call.kwargs {
                            py_kwargs.set_item(
                                monty_to_py(py, k, &self.dc_registry)?,
                                monty_to_py(py, v, &self.dc_registry)?,
                            )?;
                        }

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
                        .detach(|| call.resume(result, print_output.reborrow()))
                        .map_err(|e| self.restore_repl_from_start_error(py, *e))?;
                }
                ReplProgress::ResolveFutures(state) => {
                    self.put_repl(EitherRepl::from_core(state.into_repl()));
                    return Err(PyRuntimeError::new_err(
                        "async futures not supported with `MontyRepl.feed_run`",
                    ));
                }
            }
        }
    }

    /// Takes the REPL out of the mutex for `feed_start` (which consumes self),
    /// leaving `None` until the REPL is restored via `put_repl`.
    pub(crate) fn take_repl(&self) -> PyResult<EitherRepl> {
        let mut guard = self
            .repl
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("REPL session is currently executing another snippet"))?;
        guard
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("REPL session is currently executing another snippet"))
    }

    /// Creates an empty REPL owner for snapshot deserialization.
    ///
    /// The REPL mutex starts as `None` — the real REPL state lives inside the
    /// deserialized snapshot and will be restored via `put_repl` when the
    /// snapshot is resumed to completion.
    pub(crate) fn empty_owner(script_name: String, dc_registry: DcRegistry) -> Self {
        Self {
            repl: Mutex::new(None),
            dc_registry,
            script_name,
        }
    }

    /// Restores a REPL into the mutex after `feed_start` completes successfully.
    pub(crate) fn put_repl(&self, repl: EitherRepl) {
        let mut guard = self.repl.lock().unwrap_or_else(PoisonError::into_inner);
        *guard = Some(repl);
    }

    /// Extracts the REPL from a `ReplStartError`, restores it into `self.repl`,
    /// and returns the Python exception.
    fn restore_repl_from_start_error<T: ResourceTracker>(&self, py: Python<'_>, err: ReplStartError<T>) -> PyErr
    where
        EitherRepl: FromCoreRepl<T>,
    {
        self.put_repl(EitherRepl::from_core(err.repl));
        MontyError::new_err(py, err.error)
    }
}

/// Converts a Python dict of `{name: value}` pairs into the `Vec<(String, MontyObject)>`
/// format expected by the core REPL's `feed_run` and `feed_start`.
fn extract_repl_inputs(
    inputs: Option<&Bound<'_, PyDict>>,
    dc_registry: &DcRegistry,
) -> PyResult<Vec<(String, MontyObject)>> {
    let Some(inputs) = inputs else {
        return Ok(vec![]);
    };
    inputs
        .iter()
        .map(|(key, value)| {
            let name = key.extract::<String>()?;
            let obj = py_to_monty(&value, dc_registry)?;
            Ok((name, obj))
        })
        .collect::<PyResult<_>>()
}

/// Helper trait to convert a typed `CoreMontyRepl<T>` back into the
/// type-erased `EitherRepl` enum.
pub(crate) trait FromCoreRepl<T: ResourceTracker> {
    /// Wraps a core REPL into the appropriate `EitherRepl` variant.
    fn from_core(repl: CoreMontyRepl<T>) -> Self;
}

impl FromCoreRepl<PySignalTracker<NoLimitTracker>> for EitherRepl {
    fn from_core(repl: CoreMontyRepl<PySignalTracker<NoLimitTracker>>) -> Self {
        Self::NoLimit(repl)
    }
}

impl FromCoreRepl<PySignalTracker<LimitedTracker>> for EitherRepl {
    fn from_core(repl: CoreMontyRepl<PySignalTracker<LimitedTracker>>) -> Self {
        Self::Limited(repl)
    }
}
