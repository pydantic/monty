//! Python context restoration at host callback boundaries.

// PyO3 does not yet expose safe wrappers for entering and exiting contextvars contexts.
#![allow(unsafe_code)]

use pyo3::{ffi, prelude::*, types::PyDict};

use crate::telemetry;

pub(crate) const CALLBACK_SPAN_KEY: &str = "pydantic_monty.callback_span";

/// Runs before callback exceptions are converted into sandbox exception values.
pub(crate) fn call<T>(py: Python<'_>, callback: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
    let manager = (|| {
        // An ambient caller span is not a substitute when Monty tracing is disabled.
        let span = py
            .import("opentelemetry.context")?
            .call_method1("get_value", (CALLBACK_SPAN_KEY,))?;
        if span.is_none() {
            return Ok(None);
        }
        let kwargs = PyDict::new(py);
        kwargs.set_item("end_on_exit", false)?;
        let manager = py
            .import("opentelemetry.trace")?
            .call_method("use_span", (span,), Some(&kwargs))?;
        manager.call_method0("__enter__")?;
        Ok::<_, PyErr>(Some(manager))
    })()
    .ok()
    .flatten();

    let result = callback();
    if let Some(manager) = manager {
        // Telemetry must neither replace the callback result nor cause it to be retried.
        let _ = match &result {
            Ok(_) => manager.call_method1("__exit__", (py.None(), py.None(), py.None())),
            Err(err) => manager.call_method1("__exit__", (err.get_type(py), err.value(py), err.traceback(py))),
        };
    }
    result
}

#[derive(Debug)]
pub(crate) struct CallbackContext(Py<PyAny>);

impl CallbackContext {
    pub(crate) fn capture(py: Python<'_>) -> PyResult<Self> {
        // SAFETY: Python is attached; the API returns a new owned context reference.
        unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyContext_CopyCurrent()) }.map(|context| Self(context.unbind()))
    }

    pub(crate) fn enter<'py>(&self, py: Python<'py>, native: &opentelemetry::Context) -> PyResult<CallbackGuard<'py>> {
        // Each invocation gets its own copy: callbacks may re-enter Monty or run concurrently.
        // SAFETY: self.0 is a context created by PyContext_CopyCurrent, and Python is attached.
        let context = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyContext_Copy(self.0.as_ptr())) }?;
        // SAFETY: context is a fresh Python context and cannot already be entered.
        if unsafe { ffi::PyContext_Enter(context.as_ptr()) } != 0 {
            return Err(PyErr::fetch(py));
        }
        let mut guard = CallbackGuard { context, otel: None };
        let parent = telemetry::callback_context(py, native).or_else(|| {
            // Preserve caller context, but do not inherit an outer callback's private span selection.
            py.import("opentelemetry.context")
                .ok()?
                .call_method1("set_value", (CALLBACK_SPAN_KEY, py.None()))
                .ok()
                .map(Bound::unbind)
        });
        if let Some(parent) = parent {
            // Telemetry failure must not prevent execution of the user's callback.
            guard.otel = (|| {
                let module = py.import("opentelemetry.context")?;
                let token = module.call_method1("attach", (parent,))?;
                Ok::<_, PyErr>((module.getattr("detach")?.unbind(), token.unbind()))
            })()
            .ok();
        }
        Ok(guard)
    }
}

pub(crate) struct CallbackGuard<'py> {
    context: Bound<'py, PyAny>,
    otel: Option<(Py<PyAny>, Py<PyAny>)>,
}

impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        let py = self.context.py();
        if let Some((detach, token)) = &self.otel {
            let _ = detach.bind(py).call1((token,));
        }
        // SAFETY: this guard exits the context it entered on this thread, while Python is attached.
        if unsafe { ffi::PyContext_Exit(self.context.as_ptr()) } != 0 {
            PyErr::fetch(py).write_unraisable(py, Some(&self.context));
        }
    }
}
