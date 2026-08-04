//! Exporter-free shared telemetry pipeline forwarding records to Node's event loop.

use std::{
    sync::{Arc, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use monty_pool::telemetry_adapter::{
    configure_telemetry_adapter, TelemetryAdapter, TelemetryAdapterHandle, TELEMETRY_ADAPTER_VERSION,
};
use napi::{
    bindgen_prelude::{FnArgs, Function},
    threadsafe_function::{ThreadsafeFunctionCallMode, UnknownReturnValue},
    Status,
};
use napi_derive::napi;
use opentelemetry::{
    logs::AnyValue,
    trace::{SpanId, Status as SpanStatus, TraceId},
    Array, KeyValue, Value,
};
use opentelemetry_sdk::{logs::SdkLogRecord, trace::SpanData};
use serde_json::{json, Map, Value as JsonValue};

struct JsBridge {
    send: Box<dyn Fn(String, ThreadsafeFunctionCallMode) -> Status + Send + Sync>,
}

static HANDLE: OnceLock<TelemetryAdapterHandle> = OnceLock::new();

/// Installs the versioned Node callback and shared exporter-free pipeline.
#[napi(js_name = "_installTelemetryAdapter")]
pub fn install_telemetry_adapter(
    version: u8,
    callback: Function<'_, FnArgs<(String,)>, UnknownReturnValue>,
) -> napi::Result<()> {
    if version != TELEMETRY_ADAPTER_VERSION {
        return Err(napi::Error::from_reason(format!(
            "unsupported Monty telemetry adapter version {version}; expected {TELEMETRY_ADAPTER_VERSION}"
        )));
    }
    let callback = callback
        .build_threadsafe_function()
        .weak::<true>()
        .max_queue_size::<1024>()
        .build()?;
    let bridge = Arc::new(JsBridge {
        send: Box::new(move |event, mode| callback.call(FnArgs::from((event,)), mode)),
    });
    let handle = configure_telemetry_adapter(bridge as Arc<dyn TelemetryAdapter>)
        .map_err(|err| napi::Error::from_reason(format!("failed to configure Monty telemetry: {err}")))?;
    HANDLE
        .set(handle)
        .map_err(|_| napi::Error::from_reason("Monty telemetry is already configured"))
}

/// Returns the installed handle used to construct coupled checkout context.
pub(crate) fn configured_adapter() -> Option<&'static TelemetryAdapterHandle> {
    HANDLE.get()
}

impl TelemetryAdapter for JsBridge {
    fn start_span(&self, data: &SpanData) -> bool {
        let parent_id = (data.parent_span_id != SpanId::INVALID).then(|| data.parent_span_id.to_string());
        self.emit(json!({
            "kind": "start",
            "traceId": data.span_context.trace_id().to_string(),
            "spanId": data.span_context.span_id().to_string(),
            "parentId": parent_id,
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
        let attributes = record
            .attributes_iter()
            .map(|(key, value)| (key.as_str().to_owned(), any_value(value)))
            .collect::<Map<_, _>>();
        self.emit(
            json!({
                "kind": "log",
                "traceId": trace_id.to_string(),
                "parentId": parent_span_id.to_string(),
                "level": record.severity_text().unwrap_or("INFO"),
                "timestamp": timestamp(record.timestamp().or_else(|| record.observed_timestamp()).unwrap_or_else(SystemTime::now)),
                "body": record.body().map(any_value),
                "attributes": attributes,
            }),
        )
    }

    fn disable_root(&self, trace_id: TraceId, root_span_id: SpanId) {
        let event = json!({
            "kind": "close",
            "traceId": trace_id.to_string(),
            "spanId": root_span_id.to_string(),
        })
        .to_string();
        let _ = (self.send)(event, ThreadsafeFunctionCallMode::Blocking);
    }
}

impl JsBridge {
    /// Queues one ordered record without blocking a Tokio worker thread.
    fn emit(&self, event: JsonValue) -> bool {
        (self.send)(event.to_string(), ThreadsafeFunctionCallMode::NonBlocking) == Status::Ok
    }
}

/// Converts OTel span attributes into their JSON bridge representation.
fn attributes(values: &[KeyValue]) -> JsonValue {
    JsonValue::Object(
        values
            .iter()
            .map(|attribute| (attribute.key.as_str().to_owned(), value_to_json(&attribute.value)))
            .collect(),
    )
}

/// Converts one OTel span value without stringifying supported arrays.
fn value_to_json(value: &Value) -> JsonValue {
    match value {
        Value::Bool(value) => JsonValue::Bool(*value),
        Value::I64(value) => (*value).into(),
        Value::F64(value) => json!(value),
        Value::String(value) => value.as_str().into(),
        Value::Array(value) => match value {
            Array::Bool(values) => json!(values),
            Array::I64(values) => json!(values),
            Array::F64(values) => json!(values),
            Array::String(values) => JsonValue::Array(values.iter().map(|value| value.as_str().into()).collect()),
            _ => json!(value.to_string()),
        },
        _ => json!(value.to_string()),
    }
}

/// Converts one recursive OTel log value into JSON.
fn any_value(value: &AnyValue) -> JsonValue {
    match value {
        AnyValue::Int(value) => (*value).into(),
        AnyValue::Double(value) => json!(value),
        AnyValue::String(value) => value.as_str().into(),
        AnyValue::Boolean(value) => (*value).into(),
        AnyValue::Bytes(value) => json!(value),
        AnyValue::ListAny(values) => JsonValue::Array(values.iter().map(any_value).collect()),
        AnyValue::Map(values) => JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.as_str().to_owned(), any_value(value)))
                .collect(),
        ),
        _ => json!(format!("{value:?}")),
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
            (
                -i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
                duration.subsec_nanos(),
            )
        }
    };
    json!({ "seconds": seconds.to_string(), "nanoseconds": nanoseconds })
}
