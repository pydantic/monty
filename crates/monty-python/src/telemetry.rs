//! Bridge from Monty's Rust telemetry pipeline into Python OpenTelemetry.

use std::{
    collections::HashMap,
    mem,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use monty_pool::telemetry::{
    Measurement, MetricKind, MetricValue, Metrics, TelemetryAdapter, TelemetryAdapterHandle, TelemetryContext,
    configure_telemetry_adapter_with_host_metrics,
};
use opentelemetry::{
    Array, Context, KeyValue, Value,
    logs::AnyValue,
    trace::{SpanId, Status, TraceContextExt, TraceId},
};
use opentelemetry_sdk::{logs::SdkLogRecord, trace::SpanData};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};

/// Installed bridge and process-global Rust tracing pipeline.
struct InstalledBridge {
    bridge: Arc<PythonBridge>,
    handle: TelemetryAdapterHandle,
}

/// Standard Python OpenTelemetry objects and the state needed to call them.
struct PythonBridge {
    tracer: Option<Py<PyAny>>,
    meter: Option<Py<PyAny>>,
    logger: Option<Py<PyAny>>,
    helpers: PythonHelpers,
    spans: Mutex<HashMap<SpanKey, SpanState>>,
    instruments: Mutex<HashMap<&'static str, Py<PyAny>>>,
    /// Whether span delivery has failed.
    spans_disabled: AtomicBool,
    /// Whether log delivery has failed.
    logs_disabled: AtomicBool,
    /// Whether metric delivery has failed.
    metrics_disabled: AtomicBool,
}

/// Python OpenTelemetry API objects used by callbacks from Rust threads.
struct PythonHelpers {
    get_current_span: Py<PyAny>,
    set_span_in_context: Py<PyAny>,
    span_context: Py<PyAny>,
    non_recording_span: Py<PyAny>,
    trace_flags: Py<PyAny>,
    trace_state: Py<PyAny>,
    empty_context: Py<PyAny>,
    status_ok: Py<PyAny>,
    status_error: Py<PyAny>,
    severity_number: Py<PyAny>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct SpanKey {
    trace_id: TraceId,
    span_id: SpanId,
}

struct SpanState {
    span: Py<PyAny>,
    context: Py<PyAny>,
    root: SpanKey,
    initial_attributes: HashMap<String, Value>,
}

static BRIDGE: OnceLock<InstalledBridge> = OnceLock::new();

/// Installs standard Python OpenTelemetry components and the shared Rust pipeline.
#[pyfunction]
pub(crate) fn _install_telemetry(
    py: Python<'_>,
    tracer: Option<Py<PyAny>>,
    meter: Option<Py<PyAny>>,
    logger: Option<Py<PyAny>>,
) -> PyResult<()> {
    if BRIDGE.get().is_some() {
        return Err(PyRuntimeError::new_err("Monty telemetry is already configured"));
    }
    if tracer.is_none() && meter.is_none() && logger.is_none() {
        return Err(PyValueError::new_err(
            "at least one OpenTelemetry component is required",
        ));
    }
    let trace = py.import("opentelemetry.trace")?;
    let context = py.import("opentelemetry.context")?;
    let logs = py.import("opentelemetry._logs")?;
    let status_code = trace.getattr("StatusCode")?;
    let empty_context = context.getattr("Context")?.call0()?;
    let set_span_in_context = trace.getattr("set_span_in_context")?;
    let bridge = Arc::new(PythonBridge {
        tracer,
        meter,
        logger,
        helpers: PythonHelpers {
            get_current_span: trace.getattr("get_current_span")?.unbind(),
            set_span_in_context: set_span_in_context.unbind(),
            span_context: trace.getattr("SpanContext")?.unbind(),
            non_recording_span: trace.getattr("NonRecordingSpan")?.unbind(),
            trace_flags: trace.getattr("TraceFlags")?.unbind(),
            trace_state: trace.getattr("TraceState")?.unbind(),
            empty_context: empty_context.unbind(),
            status_ok: status_code.getattr("OK")?.unbind(),
            status_error: status_code.getattr("ERROR")?.unbind(),
            severity_number: logs.getattr("SeverityNumber")?.unbind(),
        },
        spans: Mutex::new(HashMap::new()),
        instruments: Mutex::new(HashMap::new()),
        spans_disabled: AtomicBool::new(false),
        logs_disabled: AtomicBool::new(false),
        metrics_disabled: AtomicBool::new(false),
    });
    let handle = configure_telemetry_adapter_with_host_metrics(Arc::clone(&bridge) as Arc<dyn TelemetryAdapter>)
        .map_err(|err| PyRuntimeError::new_err(format!("failed to configure Monty telemetry: {err}")))?;
    BRIDGE
        .set(InstalledBridge { bridge, handle })
        .map_err(|_| PyRuntimeError::new_err("Monty telemetry is already configured"))
}

/// The pool metrics handle when a meter was installed.
pub(crate) fn pool_metrics() -> Option<Metrics> {
    BRIDGE
        .get()
        .filter(|installed| installed.bridge.meter.is_some())
        .map(|installed| installed.handle.metrics())
}

/// Captures the current standard OpenTelemetry span before leaving Python.
pub(crate) fn capture_telemetry_context(py: Python<'_>) -> Option<TelemetryContext> {
    let installed = BRIDGE.get()?;
    let bridge = &installed.bridge;
    let logs_enabled = bridge.logger.is_some() && !bridge.logs_disabled.load(Ordering::Acquire);
    let Some(tracer) = &bridge.tracer else {
        return logs_enabled.then(|| installed.handle.unparented_context());
    };
    if bridge.spans_disabled.load(Ordering::Acquire) {
        return logs_enabled.then(|| installed.handle.unparented_context());
    }

    let result = (|| {
        let span = bridge.helpers.get_current_span.bind(py).call0()?;
        let span_context = span.call_method0("get_span_context")?;
        if !span_context.getattr("is_valid")?.extract()? {
            return Ok(installed.handle.unparented_context());
        }
        let trace_id: u128 = span_context.getattr("trace_id")?.extract()?;
        let span_id: u64 = span_context.getattr("span_id")?.extract()?;
        let trace_flags: u8 = span_context.getattr("trace_flags")?.extract()?;
        let trace_state: String = span_context
            .getattr("trace_state")?
            .call_method0("to_header")?
            .extract()?;
        let is_remote: bool = span_context.getattr("is_remote")?.extract()?;
        installed
            .handle
            .context_from_ids(trace_id.into(), span_id.into(), trace_flags, &trace_state, is_remote)
            .map_err(PyValueError::new_err)
    })();
    match result {
        Ok(context) => Some(context),
        Err(err) => {
            bridge.disable_spans(py, tracer.bind(py), err);
            logs_enabled.then(|| installed.handle.unparented_context())
        }
    }
}

impl TelemetryAdapter for PythonBridge {
    fn start_span(&self, data: &SpanData) -> bool {
        self.start_span_callback(data, false)
    }

    fn start_span_with_parent(&self, data: &SpanData, parent: &Context) -> bool {
        self.start_span_callback(data, parent.span().span_context().is_remote())
    }

    fn end_span(&self, data: &SpanData) -> bool {
        if self.tracer.is_none() {
            return true;
        }
        self.call_spans(|py| {
            let key = SpanKey {
                trace_id: data.span_context.trace_id(),
                span_id: data.span_context.span_id(),
            };
            let Some(state) = lock(&self.spans).remove(&key) else {
                return Ok(false);
            };
            let span = state.span.bind(py);
            let attributes = span_attribute_delta(py, &data.attributes, &state.initial_attributes)?;
            if !attributes.is_empty() {
                span.call_method1("set_attributes", (attributes,))?;
            }
            match &data.status {
                Status::Unset => {}
                Status::Ok => {
                    span.call_method1("set_status", (self.helpers.status_ok.bind(py),))?;
                }
                Status::Error { description } => {
                    span.call_method1("set_status", (self.helpers.status_error.bind(py), description.as_ref()))?;
                }
            }
            let kwargs = PyDict::new(py);
            kwargs.set_item("end_time", timestamp_ns(data.end_time))?;
            span.call_method("end", (), Some(&kwargs))?;
            Ok(true)
        })
        .unwrap_or_else(|| self.logs_enabled())
    }

    fn emit_log(&self, parent_span_id: SpanId, record: &SdkLogRecord) -> bool {
        let Some(logger) = &self.logger else {
            return true;
        };
        Python::attach(|py| {
            if self.logs_disabled.load(Ordering::Acquire) {
                return true;
            }
            let result = (|| {
                let trace_id = record
                    .trace_context()
                    .map_or(TraceId::INVALID, |context| context.trace_id);
                let parent = if self.tracer.is_some() && !self.spans_disabled.load(Ordering::Acquire) {
                    lock(&self.spans)
                        .get(&SpanKey {
                            trace_id,
                            span_id: parent_span_id,
                        })
                        .map(|state| state.context.clone_ref(py))
                } else {
                    Some(self.helpers.empty_context.clone_ref(py))
                };
                let Some(parent) = parent else {
                    return Ok(());
                };
                let kwargs = PyDict::new(py);
                kwargs.set_item(
                    "timestamp",
                    timestamp_ns(
                        record
                            .timestamp()
                            .or_else(|| record.observed_timestamp())
                            .unwrap_or_else(SystemTime::now),
                    ),
                )?;
                kwargs.set_item("context", parent)?;
                if let Some(severity) = record.severity_number() {
                    kwargs.set_item(
                        "severity_number",
                        self.helpers.severity_number.bind(py).call1((severity as u8,))?,
                    )?;
                }
                if let Some(severity) = record.severity_text() {
                    kwargs.set_item("severity_text", severity)?;
                }
                if let Some(body) = record.body() {
                    kwargs.set_item("body", any_value_to_py(py, body)?)?;
                }
                kwargs.set_item("attributes", log_attributes(py, record)?)?;
                logger.bind(py).call_method("emit", (), Some(&kwargs))?;
                Ok::<_, PyErr>(())
            })();
            if let Err(err) = result
                && !self.logs_disabled.swap(true, Ordering::AcqRel)
            {
                err.write_unraisable(py, Some(logger.bind(py)));
            }
            true
        })
    }

    fn disable_root(&self, trace_id: TraceId, root_span_id: SpanId) {
        let root = SpanKey {
            trace_id,
            span_id: root_span_id,
        };
        // Python finalizers may re-enter telemetry, so release the mutex before decrefing spans.
        let discarded = {
            let mut spans = lock(&self.spans);
            let (retained, discarded): (HashMap<_, _>, HashMap<_, _>) = mem::take(&mut *spans)
                .into_iter()
                .partition(|(_, state)| state.root != root);
            *spans = retained;
            discarded
        };
        drop(discarded);
    }

    fn record_metric(&self, measurement: &Measurement<'_>) {
        if self.metrics_disabled.load(Ordering::Acquire) {
            return;
        }
        let Some(meter) = &self.meter else {
            return;
        };
        let _ = Python::try_attach(|py| {
            if self.metrics_disabled.load(Ordering::Acquire) {
                return;
            }
            let result = (|| {
                let instrument = if let Some(instrument) = lock(&self.instruments).get(measurement.name) {
                    instrument.clone_ref(py)
                } else {
                    let method = match measurement.kind {
                        MetricKind::Counter => "create_counter",
                        MetricKind::UpDownCounter => "create_up_down_counter",
                        MetricKind::Histogram => "create_histogram",
                    };
                    let created = meter
                        .bind(py)
                        .call_method1(method, (measurement.name, measurement.unit, measurement.description))?
                        .unbind();
                    let mut instruments = lock(&self.instruments);
                    if let Some(instrument) = instruments.get(measurement.name) {
                        instrument.clone_ref(py)
                    } else {
                        instruments.insert(measurement.name, created.clone_ref(py));
                        created
                    }
                };
                let attributes = attributes_to_py(py, measurement.attributes)?;
                let kwargs = PyDict::new(py);
                kwargs.set_item("context", self.helpers.empty_context.bind(py))?;
                let method = match measurement.kind {
                    MetricKind::Counter | MetricKind::UpDownCounter => "add",
                    MetricKind::Histogram => "record",
                };
                match measurement.value {
                    MetricValue::I64(value) => {
                        instrument
                            .bind(py)
                            .call_method(method, (value, attributes), Some(&kwargs))?;
                    }
                    MetricValue::F64(value) => {
                        instrument
                            .bind(py)
                            .call_method(method, (value, attributes), Some(&kwargs))?;
                    }
                }
                Ok::<_, PyErr>(())
            })();
            if let Err(err) = result {
                self.metrics_disabled.store(true, Ordering::Release);
                err.write_unraisable(py, Some(meter.bind(py)));
            }
        });
    }
}

impl PythonBridge {
    /// Starts one standard Python OTel span and retains it until its Rust span ends.
    fn start_span_callback(&self, data: &SpanData, parent_is_remote: bool) -> bool {
        let Some(tracer) = &self.tracer else {
            return true;
        };
        self.call_spans(|py| {
            let key = SpanKey {
                trace_id: data.span_context.trace_id(),
                span_id: data.span_context.span_id(),
            };
            let parent_key = SpanKey {
                trace_id: key.trace_id,
                span_id: data.parent_span_id,
            };
            let (parent_context, root) = if let Some(parent) = lock(&self.spans).get(&parent_key) {
                (parent.context.clone_ref(py), parent.root)
            } else {
                (self.external_parent_context(py, data, parent_is_remote)?, key)
            };
            let attributes = attributes_to_py(py, &data.attributes)?;
            let kwargs = PyDict::new(py);
            kwargs.set_item("context", &parent_context)?;
            kwargs.set_item("attributes", attributes)?;
            kwargs.set_item("start_time", timestamp_ns(data.start_time))?;
            let span = tracer
                .bind(py)
                .call_method("start_span", (data.name.as_ref(),), Some(&kwargs))?;
            let context = self
                .helpers
                .set_span_in_context
                .bind(py)
                .call1((&span, parent_context))?
                .unbind();
            let initial_attributes = data
                .attributes
                .iter()
                .map(|attribute| (attribute.key.as_str().to_owned(), attribute.value.clone()))
                .collect();
            let mut spans = lock(&self.spans);
            let replaced = if self.spans_disabled.load(Ordering::Acquire) {
                None
            } else {
                spans.insert(
                    key,
                    SpanState {
                        span: span.unbind(),
                        context,
                        root,
                        initial_attributes,
                    },
                )
            };
            // A replaced custom span may run a finalizer which re-enters Monty.
            drop(spans);
            drop(replaced);
            Ok(true)
        })
        .unwrap_or_else(|| self.logs_enabled())
    }

    /// Reconstructs an external parent from the propagated Rust span context.
    fn external_parent_context(&self, py: Python<'_>, data: &SpanData, parent_is_remote: bool) -> PyResult<Py<PyAny>> {
        if data.parent_span_id == SpanId::INVALID {
            return Ok(self.helpers.empty_context.clone_ref(py));
        }
        let flags = self
            .helpers
            .trace_flags
            .bind(py)
            .call1((data.span_context.trace_flags().to_u8(),))?;
        let state = self
            .helpers
            .trace_state
            .bind(py)
            .call_method1("from_header", (vec![data.span_context.trace_state().header()],))?;
        let context = self.helpers.span_context.bind(py).call1((
            trace_id_int(data.span_context.trace_id()),
            span_id_int(data.parent_span_id),
            parent_is_remote,
            flags,
            state,
        ))?;
        let span = self.helpers.non_recording_span.bind(py).call1((context,))?;
        Ok(self
            .helpers
            .set_span_in_context
            .bind(py)
            .call1((span, self.helpers.empty_context.bind(py)))?
            .unbind())
    }

    /// Invokes the tracer without allowing failures to affect Monty or logging.
    fn call_spans(&self, f: impl FnOnce(Python<'_>) -> PyResult<bool>) -> Option<bool> {
        Python::attach(|py| {
            if self.spans_disabled.load(Ordering::Acquire) {
                return None;
            }
            match f(py) {
                Ok(enabled) => Some(enabled),
                Err(err) => {
                    let tracer = self.tracer.as_ref().expect("tracer checked by caller");
                    self.disable_spans(py, tracer.bind(py), err);
                    None
                }
            }
        })
    }

    fn logs_enabled(&self) -> bool {
        self.logger.is_some() && !self.logs_disabled.load(Ordering::Acquire)
    }

    /// Permanently disables spans and discards every retained Python span.
    fn disable_spans(&self, py: Python<'_>, target: &Bound<'_, PyAny>, err: PyErr) {
        // Python finalizers may re-enter telemetry, so release the mutex before decrefing spans.
        let (first_failure, discarded) = {
            let mut spans = lock(&self.spans);
            (
                !self.spans_disabled.swap(true, Ordering::AcqRel),
                mem::take(&mut *spans),
            )
        };
        drop(discarded);
        if first_failure {
            err.write_unraisable(py, Some(target));
        }
    }
}

/// Builds standard Python OTel attributes.
fn attributes_to_py<'py>(py: Python<'py>, attributes: &[KeyValue]) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for attribute in attributes {
        set_value(&dict, attribute.key.as_str(), &attribute.value)?;
    }
    Ok(dict)
}

/// Builds only the attributes added or changed after span start.
fn span_attribute_delta<'py>(
    py: Python<'py>,
    attributes: &[KeyValue],
    initial: &HashMap<String, Value>,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for attribute in attributes {
        if initial.get(attribute.key.as_str()) != Some(&attribute.value) {
            set_value(&dict, attribute.key.as_str(), &attribute.value)?;
        }
    }
    Ok(dict)
}

/// Builds standard Python OTel log attributes.
fn log_attributes<'py>(py: Python<'py>, record: &SdkLogRecord) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (key, value) in record.attributes_iter() {
        dict.set_item(key.as_str(), any_value_to_py(py, value)?)?;
    }
    Ok(dict)
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

/// Converts a recursively typed OTel log value into native Python objects.
fn any_value_to_py(py: Python<'_>, value: &AnyValue) -> PyResult<Py<PyAny>> {
    match value {
        AnyValue::Int(value) => Ok(value.into_pyobject(py)?.into_any().unbind()),
        AnyValue::Double(value) => Ok(value.into_pyobject(py)?.into_any().unbind()),
        AnyValue::String(value) => Ok(value.as_str().into_pyobject(py)?.into_any().unbind()),
        AnyValue::Boolean(value) => Ok(value.into_pyobject(py)?.to_owned().into_any().unbind()),
        AnyValue::Bytes(value) => Ok(PyBytes::new(py, value).into_any().unbind()),
        AnyValue::ListAny(values) => {
            let output = PyList::empty(py);
            for value in values.iter() {
                output.append(any_value_to_py(py, value)?)?;
            }
            Ok(output.into_any().unbind())
        }
        AnyValue::Map(values) => {
            let output = PyDict::new(py);
            for (key, value) in values.iter() {
                output.set_item(key.as_str(), any_value_to_py(py, value)?)?;
            }
            Ok(output.into_any().unbind())
        }
        _ => Ok(format!("{value:?}").into_pyobject(py)?.into_any().unbind()),
    }
}

/// Converts an OTel trace ID to the integer representation used by Python.
fn trace_id_int(value: TraceId) -> u128 {
    u128::from_be_bytes(value.to_bytes())
}

/// Converts an OTel span ID to the integer representation used by Python.
fn span_id_int(value: SpanId) -> u64 {
    u64::from_be_bytes(value.to_bytes())
}

/// Locks bridge state while recovering from an unrelated callback panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Converts a system timestamp to the nanoseconds expected by Python OTel.
fn timestamp_ns(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
        Err(err) => -i128::try_from(err.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}
