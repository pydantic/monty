//! Bridge from Monty's shared telemetry processor into an adapter installed by Python Logfire.

use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use monty_pool::telemetry_adapter::{
    TELEMETRY_ADAPTER_VERSION, TelemetryAdapter, TelemetryAdapterHandle, TelemetryContext, configure_telemetry_adapter,
};
use opentelemetry::{
    Array, KeyValue, Value,
    logs::AnyValue,
    trace::{SpanId, Status, TraceId},
};
use opentelemetry_sdk::{logs::SdkLogRecord, trace::SpanData};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyTuple},
};

/// Installed bridge and exporter-free Rust Logfire pipeline.
struct InstalledBridge {
    bridge: Arc<PythonBridge>,
    handle: TelemetryAdapterHandle,
}

/// Python callback implementing the host side of the shared adapter.
struct PythonBridge {
    adapter: Py<PyAny>,
    disabled: AtomicBool,
}

static BRIDGE: OnceLock<InstalledBridge> = OnceLock::new();

/// Installs the one-shot Python telemetry adapter and shared Rust pipeline.
#[pyfunction]
pub(crate) fn _install_telemetry_adapter(version: u8, adapter: Py<PyAny>) -> PyResult<()> {
    if version != TELEMETRY_ADAPTER_VERSION {
        return Err(PyValueError::new_err(format!(
            "unsupported Monty telemetry adapter version {version}; expected {TELEMETRY_ADAPTER_VERSION}"
        )));
    }
    let bridge = Arc::new(PythonBridge {
        adapter,
        disabled: AtomicBool::new(false),
    });
    let handle = configure_telemetry_adapter(Arc::clone(&bridge) as Arc<dyn TelemetryAdapter>)
        .map_err(|err| PyRuntimeError::new_err(format!("failed to configure Monty telemetry: {err}")))?;
    BRIDGE
        .set(InstalledBridge { bridge, handle })
        .map_err(|_| PyRuntimeError::new_err("Monty telemetry is already configured"))
}

/// Captures serializable distributed context while Python contextvars are active.
pub(crate) fn capture_telemetry_context(py: Python<'_>) -> Option<TelemetryContext> {
    let installed = BRIDGE.get()?;
    if installed.bridge.disabled.load(Ordering::Relaxed) {
        return None;
    }
    let value = match installed.bridge.adapter.bind(py).call_method0("capture_context") {
        Ok(value) => value,
        Err(err) => {
            installed.bridge.disabled.store(true, Ordering::Relaxed);
            err.write_unraisable(py, Some(installed.bridge.adapter.bind(py)));
            return None;
        }
    };
    let context = (|| {
        if value.is_none() {
            Ok(installed.handle.unparented_context())
        } else {
            let value = value.cast::<PyTuple>()?;
            let trace_id: String = value.get_item(0)?.extract()?;
            let span_id: String = value.get_item(1)?.extract()?;
            let trace_flags: u8 = value.get_item(2)?.extract()?;
            let trace_state: String = value.get_item(3)?.extract()?;
            installed
                .handle
                .context(&trace_id, &span_id, trace_flags, &trace_state)
                .map_err(PyValueError::new_err)
        }
    })();
    match context {
        Ok(context) => Some(context),
        Err(err) => {
            installed.bridge.disabled.store(true, Ordering::Relaxed);
            err.write_unraisable(py, Some(installed.bridge.adapter.bind(py)));
            None
        }
    }
}

impl TelemetryAdapter for PythonBridge {
    fn start_span(&self, data: &SpanData) -> bool {
        self.call(|py, adapter| {
            let (message_template, attributes) = span_attributes(py, &data.attributes)?;
            let parent_id = (data.parent_span_id != SpanId::INVALID).then(|| data.parent_span_id.to_string());
            adapter.call_method1(
                "start_span",
                (
                    data.span_context.trace_id().to_string(),
                    data.span_context.span_id().to_string(),
                    parent_id,
                    data.span_context.trace_flags().to_u8(),
                    data.span_context.trace_state().header(),
                    data.name.as_ref(),
                    message_template.unwrap_or_else(|| data.name.to_string()),
                    timestamp(data.start_time),
                    attributes,
                ),
            )?;
            Ok(())
        })
    }

    fn end_span(&self, data: &SpanData) -> bool {
        self.call(|py, adapter| {
            let (_, attributes) = span_attributes(py, &data.attributes)?;
            let (status, description) = match &data.status {
                Status::Unset => ("unset", None),
                Status::Ok => ("ok", None),
                Status::Error { description } => ("error", Some(description.as_ref())),
            };
            adapter.call_method1(
                "end_span",
                (
                    data.span_context.span_id().to_string(),
                    timestamp(data.end_time),
                    status,
                    description,
                    attributes,
                ),
            )?;
            Ok(())
        })
    }

    fn emit_log(&self, parent_span_id: SpanId, record: &SdkLogRecord) -> bool {
        self.call(|py, adapter| {
            let (message_template, attributes) = log_attributes(py, record)?;
            adapter.call_method1(
                "log",
                (
                    parent_span_id.to_string(),
                    record.severity_text().unwrap_or("INFO"),
                    message_template.unwrap_or_else(|| record.body().map_or_else(String::new, any_value_string)),
                    timestamp(
                        record
                            .timestamp()
                            .or_else(|| record.observed_timestamp())
                            .unwrap_or_else(SystemTime::now),
                    ),
                    attributes,
                ),
            )?;
            Ok(())
        })
    }

    fn disable_root(&self, trace_id: TraceId, root_span_id: SpanId) {
        let _ = self.call(|_, adapter| {
            adapter.call_method1("disable_root", (trace_id.to_string(), root_span_id.to_string()))?;
            Ok(())
        });
    }
}

impl PythonBridge {
    /// Invokes the adapter without allowing failures to affect sandbox execution.
    fn call(&self, f: impl FnOnce(Python<'_>, &Bound<'_, PyAny>) -> PyResult<()>) -> bool {
        if self.disabled.load(Ordering::Relaxed) {
            return false;
        }
        Python::attach(|py| {
            let adapter = self.adapter.bind(py);
            if let Err(err) = f(py, adapter) {
                self.disabled.store(true, Ordering::Relaxed);
                err.write_unraisable(py, Some(adapter));
                false
            } else {
                true
            }
        })
    }
}

/// Builds Python attributes while extracting fields regenerated by Python Logfire.
fn span_attributes<'py>(py: Python<'py>, attributes: &[KeyValue]) -> PyResult<(Option<String>, Bound<'py, PyDict>)> {
    let dict = PyDict::new(py);
    let mut message_template = None;
    for attribute in attributes {
        match attribute.key.as_str() {
            "logfire.msg_template" => message_template = Some(attribute.value.to_string()),
            "logfire.msg" | "logfire.json_schema" => {}
            key => set_value(&dict, key, &attribute.value)?,
        }
    }
    Ok((message_template, dict))
}

/// Builds Python log attributes while extracting fields regenerated by Python Logfire.
fn log_attributes<'py>(py: Python<'py>, record: &SdkLogRecord) -> PyResult<(Option<String>, Bound<'py, PyDict>)> {
    let dict = PyDict::new(py);
    let mut message_template = None;
    for (key, value) in record.attributes_iter() {
        match key.as_str() {
            "logfire.msg_template" => message_template = Some(any_value_string(value)),
            "logfire.msg" | "logfire.json_schema" => {}
            key => set_any_value(&dict, key, value)?,
        }
    }
    Ok((message_template, dict))
}

/// Inserts one OTel span attribute into a Python dictionary.
fn set_value(dict: &Bound<'_, PyDict>, key: &str, value: &Value) -> PyResult<()> {
    match value {
        Value::Bool(value) => dict.set_item(key, value),
        Value::I64(value) => dict.set_item(key, value),
        Value::F64(value) => dict.set_item(key, value),
        Value::String(value) => dict.set_item(key, value.as_str()),
        Value::Array(array) => match array {
            Array::Bool(values) => dict.set_item(key, PyList::new(dict.py(), values)?),
            Array::I64(values) => dict.set_item(key, PyList::new(dict.py(), values)?),
            Array::F64(values) => dict.set_item(key, PyList::new(dict.py(), values)?),
            Array::String(values) => dict.set_item(
                key,
                PyList::new(dict.py(), values.iter().map(opentelemetry::StringValue::as_str))?,
            ),
            _ => dict.set_item(key, value.to_string()),
        },
        _ => dict.set_item(key, value.to_string()),
    }
}

/// Inserts one OTel log attribute into a Python dictionary.
fn set_any_value(dict: &Bound<'_, PyDict>, key: &str, value: &AnyValue) -> PyResult<()> {
    match value {
        AnyValue::Int(value) => dict.set_item(key, value),
        AnyValue::Double(value) => dict.set_item(key, value),
        AnyValue::String(value) => dict.set_item(key, value.as_str()),
        AnyValue::Boolean(value) => dict.set_item(key, value),
        AnyValue::Bytes(value) => dict.set_item(key, PyBytes::new(dict.py(), value)),
        AnyValue::ListAny(_) | AnyValue::Map(_) => dict.set_item(key, format!("{value:?}")),
        _ => dict.set_item(key, format!("{value:?}")),
    }
}

/// Renders a log body when no explicit message template was recorded.
fn any_value_string(value: &AnyValue) -> String {
    if let AnyValue::String(value) = value {
        value.to_string()
    } else {
        format!("{value:?}")
    }
}

/// Converts a system timestamp without losing subsecond precision.
fn timestamp(time: SystemTime) -> (i64, u32) {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => (
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
            duration.subsec_nanos(),
        ),
        Err(err) => {
            let duration = err.duration();
            let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
            let nanoseconds = duration.subsec_nanos();
            if nanoseconds == 0 {
                (-seconds, 0)
            } else {
                (seconds.saturating_neg().saturating_sub(1), 1_000_000_000 - nanoseconds)
            }
        }
    }
}
