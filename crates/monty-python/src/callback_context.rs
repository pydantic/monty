//! OpenTelemetry activation at host callback boundaries.

use pyo3::{prelude::*, types::PyDict};

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

/// Activates the suspension's span without replacing the calling task's Python context.
pub(crate) fn enter<'py>(py: Python<'py>, native: &opentelemetry::Context) -> CallbackGuard<'py> {
    let parent = telemetry::callback_context(py, native).or_else(|| {
        // Do not inherit an outer callback's private span selection when tracing is disabled.
        let module = py.import("opentelemetry.context").ok()?;
        if module.call_method1("get_value", (CALLBACK_SPAN_KEY,)).ok()?.is_none() {
            None
        } else {
            module
                .call_method1("set_value", (CALLBACK_SPAN_KEY, py.None()))
                .ok()
                .map(Bound::unbind)
        }
    });
    let otel = parent.and_then(|parent| {
        // Telemetry failure must not prevent execution of the user's callback.
        (|| {
            let module = py.import("opentelemetry.context")?;
            let token = module.call_method1("attach", (parent,))?;
            Ok::<_, PyErr>((module.getattr("detach")?.unbind(), token.unbind()))
        })()
        .ok()
    });
    CallbackGuard { py, otel }
}

/// Restores only Monty's temporary OTel activation; callback context-variable changes persist.
pub(crate) struct CallbackGuard<'py> {
    py: Python<'py>,
    otel: Option<(Py<PyAny>, Py<PyAny>)>,
}

impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        if let Some((detach, token)) = &self.otel {
            let _ = detach.bind(self.py).call1((token,));
        }
    }
}
