//! Statically linked Logfire pipeline forwarding telemetry to Node's event loop.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use monty_pool::telemetry::{
    configure_telemetry_adapter_with_host_metrics, Measurement, MetricKind, MetricValue, TelemetryAdapter,
    TelemetryAdapterHandle,
};
use napi::{
    bindgen_prelude::{spawn, FnArgs, Function},
    threadsafe_function::ThreadsafeFunctionCallMode,
    Status,
};
use napi_derive::napi;
use opentelemetry::{
    logs::AnyValue,
    trace::{SpanId, Status as SpanStatus, TraceContextExt, TraceId},
    Array, KeyValue, Value,
};
use opentelemetry_sdk::{logs::SdkLogRecord, trace::SpanData};
use serde_json::{json, Map, Value as JsonValue};
use tokio::{task::spawn_blocking, time::sleep};

type SendEvent = Arc<dyn Fn(String, ThreadsafeFunctionCallMode) -> Status + Send + Sync>;
type SendAndWait = Arc<dyn Fn(String) -> Option<bool> + Send + Sync>;

struct JsBridge {
    send: SendEvent,
    send_and_wait: SendAndWait,
    send_metrics: SendEvent,
    flush_metrics: SendAndWait,
    /// Queue overflow disables tracing rather than leaving gaps in open spans.
    tracing_disabled: AtomicBool,
    /// Whether the metric callback can no longer receive measurements.
    metrics_disabled: AtomicBool,
    delivery: RwLock<DeliveryState>,
}

struct DeliveryState {
    cleanup_requested: bool,
}

struct InstalledBridge {
    bridge: Arc<JsBridge>,
    handle: TelemetryAdapterHandle,
}

static BRIDGE: OnceLock<InstalledBridge> = OnceLock::new();

/// Maximum JSON budget for one attribute crossing the Node callback queue.
const VALUE_SIZE_LIMIT: usize = 64 * 1024;
/// Maximum nesting accepted from recursive OTel log values.
const VALUE_DEPTH_LIMIT: usize = 64;

/// Installs the Node callbacks and process-global Rust telemetry pipeline.
#[napi(js_name = "_installTelemetry")]
pub fn install_telemetry(
    callback: Function<'_, FnArgs<(String,)>, bool>,
    metrics_callback: Function<'_, FnArgs<(String,)>, bool>,
) -> napi::Result<()> {
    let callback = Arc::new(
        callback
            .build_threadsafe_function()
            .weak::<true>()
            .max_queue_size::<1024>()
            .build()?,
    );
    let send_callback = Arc::clone(&callback);
    let send: SendEvent = Arc::new(move |event, mode| send_callback.call(FnArgs::from((event,)), mode));
    let wait_callback = Arc::clone(&callback);
    let send_and_wait: SendAndWait = Arc::new(move |event| {
        let (acknowledge, acknowledged) = mpsc::sync_channel(1);
        let status = wait_callback.call_with_return_value(
            FnArgs::from((event,)),
            ThreadsafeFunctionCallMode::Blocking,
            move |result, _| {
                let _ = acknowledge.send(result.unwrap_or(false));
                Ok(())
            },
        );
        (status == Status::Ok).then(|| acknowledged.recv().ok()).flatten()
    });
    let metrics_callback = Arc::new(
        metrics_callback
            .build_threadsafe_function()
            .weak::<true>()
            .max_queue_size::<1024>()
            .build()?,
    );
    let send_metrics_callback = Arc::clone(&metrics_callback);
    let send_metrics: SendEvent = Arc::new(move |event, mode| send_metrics_callback.call(FnArgs::from((event,)), mode));
    let flush_metrics_callback = Arc::clone(&metrics_callback);
    let flush_metrics: SendAndWait = Arc::new(move |event| {
        let (acknowledge, acknowledged) = mpsc::sync_channel(1);
        let status = flush_metrics_callback.call_with_return_value(
            FnArgs::from((event,)),
            ThreadsafeFunctionCallMode::Blocking,
            move |result, _| {
                let _ = acknowledge.send(result.unwrap_or(false));
                Ok(())
            },
        );
        (status == Status::Ok).then(|| acknowledged.recv().ok()).flatten()
    });
    let bridge = Arc::new(JsBridge {
        send,
        send_and_wait,
        send_metrics,
        flush_metrics,
        tracing_disabled: AtomicBool::new(false),
        metrics_disabled: AtomicBool::new(false),
        delivery: RwLock::new(DeliveryState {
            cleanup_requested: false,
        }),
    });
    let handle = configure_telemetry_adapter_with_host_metrics(Arc::clone(&bridge) as Arc<dyn TelemetryAdapter>)
        .map_err(|err| napi::Error::from_reason(format!("failed to configure Monty telemetry: {err}")))?;
    BRIDGE
        .set(InstalledBridge { bridge, handle })
        .map_err(|_| napi::Error::from_reason("Monty telemetry is already configured"))
}

/// Waits until queued telemetry has reached the Node event loop.
#[napi(js_name = "_flushTelemetry")]
pub async fn flush_telemetry() -> napi::Result<()> {
    if let Some(installed) = BRIDGE.get() {
        let bridge = Arc::clone(&installed.bridge);
        spawn_blocking(move || {
            let event = json!({ "kind": "flush" }).to_string();
            let _ = (bridge.send_and_wait)(event.clone());
            let _ = (bridge.flush_metrics)(event);
        })
        .await
        .map_err(|err| napi::Error::from_reason(format!("failed to join Monty telemetry flush: {err}")))?;
    }
    Ok(())
}

/// Returns the installed handle used to configure pool metrics.
pub(crate) fn configured_adapter() -> Option<&'static TelemetryAdapterHandle> {
    BRIDGE.get().map(|installed| &installed.handle)
}

/// Returns the installed handle while span and log delivery remains available.
pub(crate) fn configured_tracing_adapter() -> Option<&'static TelemetryAdapterHandle> {
    let installed = BRIDGE.get()?;
    (!installed.bridge.tracing_disabled.load(Ordering::Relaxed)).then_some(&installed.handle)
}

impl TelemetryAdapter for JsBridge {
    fn start_span(&self, data: &SpanData) -> bool {
        let parent_id = (data.parent_span_id != SpanId::INVALID).then(|| data.parent_span_id.to_string());
        self.start_span_event(json!({
            "kind": "start",
            "traceId": data.span_context.trace_id().to_string(),
            "spanId": data.span_context.span_id().to_string(),
            "parentId": parent_id,
            "parentIsRemote": false,
            "traceFlags": data.span_context.trace_flags().to_u8(),
            "traceState": data.span_context.trace_state().header(),
            "name": data.name,
            "timestamp": timestamp(data.start_time),
            "attributes": attributes(&data.attributes),
        }))
    }

    fn start_span_with_parent(&self, data: &SpanData, parent: &opentelemetry::Context) -> bool {
        let parent_id = (data.parent_span_id != SpanId::INVALID).then(|| data.parent_span_id.to_string());
        self.start_span_event(json!({
            "kind": "start",
            "traceId": data.span_context.trace_id().to_string(),
            "spanId": data.span_context.span_id().to_string(),
            "parentId": parent_id,
            "parentIsRemote": parent.span().span_context().is_remote(),
            "traceFlags": data.span_context.trace_flags().to_u8(),
            "traceState": data.span_context.trace_state().header(),
            "name": data.name,
            "timestamp": timestamp(data.start_time),
            "attributes": attributes(&data.attributes),
        }))
    }

    fn end_span(&self, data: &SpanData) -> bool {
        let (status, description) = match &data.status {
            SpanStatus::Unset => ("unset", None),
            SpanStatus::Ok => ("ok", None),
            SpanStatus::Error { description } => ("error", Some(description.as_ref())),
        };
        self.emit(json!({
            "kind": "end",
            "traceId": data.span_context.trace_id().to_string(),
            "spanId": data.span_context.span_id().to_string(),
            "timestamp": timestamp(data.end_time),
            "status": status,
            "statusDescription": description,
            "attributes": attributes(&data.attributes),
        }))
    }

    fn emit_log(&self, parent_span_id: SpanId, record: &SdkLogRecord) -> bool {
        let trace_id = record
            .trace_context()
            .map_or(TraceId::INVALID, |context| context.trace_id);
        let mut any_cut = false;
        let mut attributes = record
            .attributes_iter()
            .map(|(key, value)| {
                let (value, cut) = any_value(value);
                any_cut |= cut;
                (key.as_str().to_owned(), value)
            })
            .collect::<Map<_, _>>();
        let body = record.body().map(|body| {
            let (body, cut) = any_value(body);
            any_cut |= cut;
            body
        });
        if any_cut {
            attributes.insert("length_limit_exceeded".to_owned(), JsonValue::Bool(true));
        }
        self.emit(
            json!({
                "kind": "log",
                "traceId": trace_id.to_string(),
                "parentId": parent_span_id.to_string(),
                "severityNumber": record.severity_number().map(|severity| severity as u8),
                "severityText": record.severity_text().unwrap_or("INFO"),
                "timestamp": timestamp(record.timestamp().or_else(|| record.observed_timestamp()).unwrap_or_else(SystemTime::now)),
                "body": body,
                "attributes": attributes,
            }),
        )
    }

    fn record_metric(&self, measurement: &Measurement<'_>) {
        if self.metrics_disabled.load(Ordering::Relaxed) {
            return;
        }
        let kind = match measurement.kind {
            MetricKind::Counter => "counter",
            MetricKind::UpDownCounter => "upDownCounter",
            MetricKind::Histogram => "histogram",
        };
        let value = match measurement.value {
            MetricValue::I64(value) => json!(value),
            MetricValue::F64(value) => json!(value),
        };
        let event = json!({
            "kind": "metric",
            "instrumentKind": kind,
            "name": measurement.name,
            "unit": measurement.unit,
            "description": measurement.description,
            "value": value,
            "attributes": attributes(measurement.attributes),
        })
        .to_string();
        if (self.send_metrics)(event, ThreadsafeFunctionCallMode::NonBlocking) != Status::Ok {
            self.metrics_disabled.store(true, Ordering::Relaxed);
        }
    }

    fn disable_root(&self, trace_id: TraceId, root_span_id: SpanId) {
        let mut delivery = write_lock(&self.delivery);
        let all = self.tracing_disabled.load(Ordering::Relaxed);
        if all && delivery.cleanup_requested {
            return;
        }
        delivery.cleanup_requested |= all;
        let event = json!({
            "kind": "close",
            "traceId": trace_id.to_string(),
            "spanId": root_span_id.to_string(),
            "all": all,
        })
        .to_string();
        let cleanup_send = Arc::clone(&self.send);
        // A single async retry task neither blocks a pool worker nor grows
        // native cleanup state while the JS event queue drains.
        drop(spawn(async move {
            while cleanup_send(event.clone(), ThreadsafeFunctionCallMode::NonBlocking) == Status::QueueFull {
                sleep(Duration::from_millis(10)).await;
            }
        }));
    }
}

impl JsBridge {
    /// Starts a span synchronously so the host can report delivery failure.
    fn start_span_event(&self, event: JsonValue) -> bool {
        let _delivery = read_lock(&self.delivery);
        if self.tracing_disabled.load(Ordering::Relaxed) {
            return false;
        }
        if let Some(enabled) = (self.send_and_wait)(event.to_string()) {
            enabled
        } else {
            self.tracing_disabled.store(true, Ordering::Relaxed);
            false
        }
    }

    /// Queues one ordered record without blocking a Tokio worker thread.
    fn emit(&self, event: JsonValue) -> bool {
        let _delivery = read_lock(&self.delivery);
        if self.tracing_disabled.load(Ordering::Relaxed) {
            false
        } else {
            let sent = (self.send)(event.to_string(), ThreadsafeFunctionCallMode::NonBlocking) == Status::Ok;
            if !sent {
                self.tracing_disabled.store(true, Ordering::Relaxed);
            }
            sent
        }
    }
}

/// Reads delivery state while recovering from an unrelated panic.
fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Writes delivery state while recovering from an unrelated panic.
fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// Converts OTel span attributes into their JSON bridge representation.
fn attributes(values: &[KeyValue]) -> JsonValue {
    let mut cut = false;
    let mut attributes = values
        .iter()
        .map(|attribute| {
            let (value, value_cut) = value_to_json(&attribute.value);
            cut |= value_cut;
            (attribute.key.as_str().to_owned(), value)
        })
        .collect::<Map<_, _>>();
    if cut {
        attributes.insert("length_limit_exceeded".to_owned(), JsonValue::Bool(true));
    }
    JsonValue::Object(attributes)
}

/// Converts one OTel span value without stringifying supported arrays.
fn value_to_json(value: &Value) -> (JsonValue, bool) {
    let mut budget = JsonBudget::new();
    let value = value_to_json_budget(value, &mut budget);
    (value, budget.cut)
}

/// Converts a span value while sharing one allocation budget across children.
fn value_to_json_budget(value: &Value, budget: &mut JsonBudget) -> JsonValue {
    match value {
        Value::Bool(value) => {
            budget.consume(5);
            JsonValue::Bool(*value)
        }
        Value::I64(value) => {
            budget.consume(20);
            (*value).into()
        }
        Value::F64(value) => {
            budget.consume(24);
            json!(value)
        }
        Value::String(value) => JsonValue::String(budget.string(value.as_str())),
        Value::Array(value) => match value {
            Array::Bool(values) => budget.array(values, |value, budget| {
                budget.consume(5);
                JsonValue::Bool(*value)
            }),
            Array::I64(values) => budget.array(values, |value, budget| {
                budget.consume(20);
                (*value).into()
            }),
            Array::F64(values) => budget.array(values, |value, budget| {
                budget.consume(24);
                json!(value)
            }),
            Array::String(values) => {
                budget.array(values, |value, budget| JsonValue::String(budget.string(value.as_str())))
            }
            _ => budget.unsupported("<unsupported array>"),
        },
        _ => budget.unsupported("<unsupported value>"),
    }
}

/// Converts one recursive OTel log value into bounded JSON.
fn any_value(value: &AnyValue) -> (JsonValue, bool) {
    let mut budget = JsonBudget::new();
    let value = any_value_budget(value, &mut budget, 0);
    (value, budget.cut)
}

/// Converts a recursive log value while sharing one allocation budget.
fn any_value_budget(value: &AnyValue, budget: &mut JsonBudget, depth: usize) -> JsonValue {
    if depth >= VALUE_DEPTH_LIMIT {
        return budget.unsupported("<depth limit reached>");
    }
    match value {
        AnyValue::Int(value) => {
            budget.consume(20);
            (*value).into()
        }
        AnyValue::Double(value) => {
            budget.consume(24);
            json!(value)
        }
        AnyValue::String(value) => JsonValue::String(budget.string(value.as_str())),
        AnyValue::Boolean(value) => {
            budget.consume(5);
            (*value).into()
        }
        AnyValue::Bytes(value) => {
            // Each byte becomes up to three decimal digits plus a comma.
            let keep = value.len().min(budget.remaining / 4);
            budget.consume(keep * 4);
            budget.cut |= keep < value.len();
            json!(&value[..keep])
        }
        AnyValue::ListAny(values) => budget.array(values, |value, budget| any_value_budget(value, budget, depth + 1)),
        AnyValue::Map(values) => {
            budget.consume(2); // `{}`
            let mut output = Map::new();
            for (key, value) in values.iter() {
                if budget.exhausted() {
                    budget.cut = true;
                    break;
                }
                budget.consume(2); // `:` and a possible comma
                let key = budget.string(key.as_str());
                output.insert(key, any_value_budget(value, budget, depth + 1));
            }
            JsonValue::Object(output)
        }
        _ => budget.unsupported("<unsupported value>"),
    }
}

/// Remaining host-allocation budget while constructing one JSON value.
struct JsonBudget {
    remaining: usize,
    cut: bool,
}

impl JsonBudget {
    /// Starts one per-value bridge budget.
    const fn new() -> Self {
        Self {
            remaining: VALUE_SIZE_LIMIT,
            cut: false,
        }
    }

    /// Charges bytes and returns how many fit.
    fn consume(&mut self, requested: usize) -> usize {
        let consumed = requested.min(self.remaining);
        self.remaining -= consumed;
        self.cut |= consumed < requested;
        consumed
    }

    /// Copies a prefix whose escaped JSON string fits the remaining budget.
    fn string(&mut self, value: &str) -> String {
        if self.consume(2) < 2 {
            return String::new();
        }
        let mut end = 0;
        for (index, character) in value.char_indices() {
            let escaped_len = match character {
                '"' | '\\' | '\u{0008}' | '\t' | '\n' | '\u{000c}' | '\r' => 2,
                '\u{0000}'..='\u{001f}' => 6,
                _ => character.len_utf8(),
            };
            if escaped_len > self.remaining {
                self.cut = true;
                break;
            }
            self.consume(escaped_len);
            end = index + character.len_utf8();
        }
        self.cut |= end < value.len();
        value[..end].to_owned()
    }

    /// Converts array elements until their shared budget is exhausted.
    fn array<T>(&mut self, values: &[T], convert: impl Fn(&T, &mut Self) -> JsonValue) -> JsonValue {
        self.consume(2); // `[]`
        let mut output = Vec::new();
        for value in values {
            if self.exhausted() {
                self.cut = true;
                break;
            }
            self.consume(1);
            output.push(convert(value, self));
        }
        JsonValue::Array(output)
    }

    /// Marks an unsupported value and returns a bounded placeholder.
    fn unsupported(&mut self, placeholder: &str) -> JsonValue {
        self.cut = true;
        JsonValue::String(self.string(placeholder))
    }

    /// Whether another child cannot fit in this value.
    const fn exhausted(&self) -> bool {
        self.remaining == 0
    }
}

/// Converts a system timestamp without using lossy JS nanoseconds.
fn timestamp(time: SystemTime) -> JsonValue {
    let (seconds, nanoseconds) = match time.duration_since(UNIX_EPOCH) {
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
    };
    json!({ "seconds": seconds.to_string(), "nanoseconds": nanoseconds })
}
