//! Telemetry for the pool: spans describing what each session did, and
//! aggregate metrics over the fleet running them.
//!
//! This root is the host-facing surface both are delivered through — the
//! [`TelemetryAdapter`] bridge for a host whose SDK is in another language,
//! and [`TelemetryContext::for_logfire`] / [`Metrics::for_logfire`] for a Rust
//! host that already owns one. The recorders behind it are internal: the
//! `tracing` submodule mirrors the protocol into spans, `metrics` into
//! instruments, and `tracing_json` encodes span attributes the way the Logfire
//! SDKs do.

pub(crate) mod metrics;
pub(crate) mod tracing;
mod tracing_json;

use std::{
    collections::{HashMap, HashSet},
    fmt,
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use logfire::{ConfigureError, Logfire, config::AdvancedOptions};
pub use metrics::{Measurement, MetricKind, MetricValue, Metrics};
use opentelemetry::{
    Context, InstrumentationScope,
    trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState},
};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    logs::{LogProcessor, SdkLogRecord},
    trace::{Span, SpanData, SpanProcessor},
};

/// Current host-adapter protocol version.
pub const TELEMETRY_ADAPTER_VERSION: u8 = 1;

/// Configured exporter-free pipeline shared by all checkouts using one adapter.
pub struct TelemetryAdapterHandle {
    /// Retains the isolated provider and its processors for the handle's lifetime.
    logfire: Logfire,
    /// The one recorder every pool built from this handle shares, so the
    /// worker counters total over all of them. Reaches the adapter directly
    /// rather than through the span pipeline.
    metrics: Metrics,
}

/// Distributed parent context and isolated recorder for one checkout root.
pub struct TelemetryContext {
    parent: Option<SpanContext>,
    logfire: Logfire,
}

impl TelemetryAdapterHandle {
    /// Validates host W3C fields and couples them to this adapter pipeline.
    pub fn context(
        &self,
        trace_id: &str,
        span_id: &str,
        trace_flags: u8,
        trace_state: &str,
    ) -> Result<TelemetryContext, String> {
        let trace_id = TraceId::from_hex(trace_id).map_err(|err| format!("invalid trace ID: {err}"))?;
        let span_id = SpanId::from_hex(span_id).map_err(|err| format!("invalid span ID: {err}"))?;
        let trace_state = TraceState::from_str(trace_state).map_err(|err| format!("invalid trace state: {err}"))?;
        if trace_id == TraceId::INVALID || span_id == SpanId::INVALID {
            return Err("trace and span IDs must be non-zero".to_owned());
        }
        Ok(TelemetryContext {
            parent: Some(SpanContext::new(
                trace_id,
                span_id,
                TraceFlags::new(trace_flags),
                true,
                trace_state,
            )),
            logfire: self.logfire.clone(),
        })
    }

    /// Creates adapter context when no host span is active.
    #[must_use]
    pub fn unparented_context(&self) -> TelemetryContext {
        TelemetryContext {
            parent: None,
            logfire: self.logfire.clone(),
        }
    }

    /// Metrics handle for [`PoolConfig::metrics`](crate::PoolConfig::metrics).
    ///
    /// Unlike spans, metrics do not travel per checkout: they are aggregates
    /// covering untraced sessions and worker lifecycle events that belong to
    /// no session at all. Every call returns the same shared recorder, so the
    /// worker counters sum over all pools configured from this handle.
    #[must_use]
    pub fn metrics(&self) -> Metrics {
        self.metrics.clone()
    }
}

impl TelemetryContext {
    /// Records the pool's spans straight into `logfire`: a Rust host already
    /// owns a configured SDK, so it needs no [`TelemetryAdapter`] bridge.
    /// Unparented, like [`TelemetryAdapterHandle::unparented_context`] — an
    /// in-process ambient span attaches through `tracing`, not W3C fields.
    #[must_use]
    pub fn for_logfire(logfire: Logfire) -> Self {
        Self { parent: None, logfire }
    }
}

impl TelemetryContext {
    /// Returns the isolated recorder installed while processing this checkout.
    pub(crate) fn logfire(&self) -> Logfire {
        self.logfire.clone()
    }

    /// Converts the propagated span into the remote OTel parent context.
    pub(crate) fn into_parent(self) -> Option<Context> {
        self.parent
            .map(|parent| Context::new().with_remote_span_context(parent))
    }
}

/// Receives Monty records without owning exporters or credentials.
/// Returning false disables the affected Monty root and calls `disable_root`.
pub trait TelemetryAdapter: Send + Sync + 'static {
    /// Starts a reconstructed host span keyed by its Rust OTel span ID.
    fn start_span(&self, span: &SpanData) -> bool;
    /// Applies final data and ends a reconstructed host span.
    fn end_span(&self, span: &SpanData) -> bool;
    /// Emits a log under the reconstructed span identified by `parent_span_id`.
    fn emit_log(&self, parent_span_id: SpanId, record: &SdkLogRecord) -> bool;
    /// Discards host-side state after delivery for one Monty root becomes unreliable.
    fn disable_root(&self, trace_id: TraceId, root_span_id: SpanId);
    /// Records one metric measurement, creating the instrument named by
    /// [`Measurement::name`] on first sight.
    ///
    /// Defaults to dropping the measurement, so an adapter written against an
    /// earlier version of this trait keeps compiling and keeps working — which
    /// is why adding metrics did not bump [`TELEMETRY_ADAPTER_VERSION`].
    /// Metrics carry no span context and cannot disable a root, so unlike the
    /// methods above this one reports nothing back.
    fn record_metric(&self, measurement: &Measurement<'_>) {
        let _ = measurement;
    }
}

/// Configures the exporter-free Rust pipeline shared by language adapters.
pub fn configure_telemetry_adapter(
    adapter: Arc<dyn TelemetryAdapter>,
) -> Result<TelemetryAdapterHandle, ConfigureError> {
    let processor = AdapterProcessor::new(Arc::clone(&adapter));
    let logfire = logfire::configure()
        .local()
        .send_to_logfire(false)
        .with_console(None)
        .with_install_panic_handler(false)
        .with_additional_span_processor(processor.clone())
        .with_advanced_options(AdvancedOptions::default().with_log_processor(processor))
        .finish()?;
    Ok(TelemetryAdapterHandle {
        logfire,
        metrics: Metrics::new(adapter),
    })
}

/// Shared processor state for span ancestry and root-level disablement.
#[derive(Clone)]
struct AdapterProcessor(Arc<ProcessorState>);

/// Span ancestry shared by the span and log processor callbacks.
struct ProcessorState {
    adapter: Arc<dyn TelemetryAdapter>,
    routing: Mutex<RoutingState>,
}

/// Mutable ancestry and delivery state shared by processor callbacks.
struct RoutingState {
    spans: HashMap<SpanKey, SpanKey>,
    disabled: HashSet<SpanKey>,
}

/// Identifies an OTel span without assuming span IDs are process-global.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct SpanKey {
    trace_id: TraceId,
    span_id: SpanId,
}

impl AdapterProcessor {
    /// Creates the shared span/log processor around one language adapter.
    fn new(adapter: Arc<dyn TelemetryAdapter>) -> Self {
        Self(Arc::new(ProcessorState {
            adapter,
            routing: Mutex::new(RoutingState {
                spans: HashMap::new(),
                disabled: HashSet::new(),
            }),
        }))
    }

    /// Stops one Monty root after its ordered event stream becomes unreliable.
    fn disable(&self, root: SpanKey) {
        if lock(&self.0.routing).disabled.insert(root) {
            self.0.adapter.disable_root(root.trace_id, root.span_id);
        }
    }
}

impl fmt::Debug for AdapterProcessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AdapterProcessor")
    }
}

impl SpanProcessor for AdapterProcessor {
    fn on_start(&self, span: &mut Span, parent: &Context) {
        let Some(data) = span.exported_data() else { return };
        let span_key = SpanKey {
            trace_id: data.span_context.trace_id(),
            span_id: data.span_context.span_id(),
        };
        let parent_key = SpanKey {
            trace_id: span_key.trace_id,
            span_id: parent.span().span_context().span_id(),
        };
        let (root, disabled) = {
            let mut routing = lock(&self.0.routing);
            let root = routing.spans.get(&parent_key).copied().unwrap_or(span_key);
            routing.spans.insert(span_key, root);
            (root, routing.disabled.contains(&root))
        };
        if !disabled && !self.0.adapter.start_span(&data) {
            self.disable(root);
        }
    }

    fn on_end(&self, span: SpanData) {
        let span_key = SpanKey {
            trace_id: span.span_context.trace_id(),
            span_id: span.span_context.span_id(),
        };
        let (root, disabled) = {
            let mut routing = lock(&self.0.routing);
            let Some(root) = routing.spans.remove(&span_key) else {
                return;
            };
            (root, routing.disabled.contains(&root))
        };
        if !disabled && !self.0.adapter.end_span(&span) {
            self.disable(root);
        }
        if root == span_key {
            lock(&self.0.routing).disabled.remove(&root);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
}

impl LogProcessor for AdapterProcessor {
    fn emit(&self, record: &mut SdkLogRecord, _instrumentation: &InstrumentationScope) {
        let Some(context) = record.trace_context() else { return };
        let parent = SpanKey {
            trace_id: context.trace_id,
            span_id: context.span_id,
        };
        let (root, disabled) = {
            let routing = lock(&self.0.routing);
            let Some(root) = routing.spans.get(&parent).copied() else {
                return;
            };
            (root, routing.disabled.contains(&root))
        };
        if disabled {
            return;
        }
        if !self.0.adapter.emit_log(context.span_id, record) {
            self.disable(root);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
}

/// Locks processor state while recovering from an unrelated panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
