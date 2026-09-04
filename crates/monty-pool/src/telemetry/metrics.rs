//! Aggregate metrics for pool health and turn latency — what the spans in
//! [`tracing`](crate::telemetry::tracing) cannot answer: is the fleet
//! saturated, how often do workers die, how much sandbox time does a typical
//! run cost.
//!
//! Recorded for *every* checkout, not only those a host gave a
//! [`TelemetryContext`](crate::telemetry::TelemetryContext) to: an aggregate
//! covering only traced sessions would mislead. Rust hosts record into Logfire
//! instruments directly. A foreign adapter can instead receive each raw
//! measurement so its host metrics SDK controls aggregation.
//!
//! **No value the sandbox controls may become an attribute.** Every attribute
//! here is a closed set fixed by this crate, because one time series per value
//! is a way to exhaust a metrics backend — and a script can mint values freely
//! (calling `f_1()`, `f_2()`, … or raising a class per iteration). So a called
//! function's name is never recorded, not even when the host resolved it: a
//! host whose lookup is a callable resolves *any* name. Only the fixed os-call
//! names in [`os_call`] look like an exception, and they come from the
//! protocol's oneof rather than from the sandbox.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, PoisonError, RwLock},
    time::{Duration, Instant},
};

use logfire::{ExponentialHistogram, Logfire};
use monty_proto::{pb, pb::os_call::Call};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, UpDownCounter},
};

use super::TelemetryAdapter;

/// Live workers, whatever they are doing.
static LIVE_WORKERS: Instrument = Instrument {
    kind: MetricKind::UpDownCounter,
    name: "monty.pool.workers.live",
    unit: "{worker}",
    description: "Workers the pool is keeping alive: pooled, checked out, or being spawned.",
};

/// Pooled workers immediately available for checkout.
static IDLE_WORKERS: Instrument = Instrument {
    kind: MetricKind::UpDownCounter,
    name: "monty.pool.workers.idle",
    unit: "{worker}",
    description: "Workers immediately available for checkout.",
};

/// Checked-out workers blocked waiting for the host.
static SUSPENDED_WORKERS: Instrument = Instrument {
    kind: MetricKind::UpDownCounter,
    name: "monty.pool.workers.suspended",
    unit: "{worker}",
    description: "Workers blocked waiting for the host to answer a suspension.",
};

/// Time spent waiting for a worker to check out.
static CHECKOUT_WAIT: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.pool.checkout.wait",
    unit: "s",
    description: "Time spent acquiring a worker for a checkout.",
};

/// Workers leaving the pool, by why.
static WORKER_TERMINATED: Instrument = Instrument {
    kind: MetricKind::Counter,
    name: "monty.pool.worker.terminated",
    unit: "{worker}",
    description: "Workers discarded by the pool, by reason.",
};

/// Checkout lifetime.
static SESSION_DURATION: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.pool.session.duration",
    unit: "s",
    description: "Lifetime of a checked-out session.",
};

/// Wall time of one execution turn.
static RUN_DURATION: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.run.duration",
    unit: "s",
    description: "Wall time of one feed, including time spent waiting on the host.",
};

/// Sandbox time of one execution turn. Subtracting it from [`RUN_DURATION`]
/// leaves host and transport overhead, primarily suspension handling.
static RUN_EXECUTION: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.run.execution_time",
    unit: "s",
    description: "Sandbox execution time of one feed, excluding host round-trips.",
};

/// Wall time of one housekeeping turn.
static TURN_DURATION: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.turn.duration",
    unit: "s",
    description: "Wall time of one non-execution turn, by kind.",
};

/// Suspensions the sandbox raised.
static SUSPENSIONS: Instrument = Instrument {
    kind: MetricKind::Counter,
    name: "monty.run.suspensions",
    unit: "{suspension}",
    description: "Suspensions the sandbox raised for the host to answer, by kind.",
};

/// Round-trip time of one suspension, whoever answers it.
static EXT_CALL: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.ext.call.duration",
    unit: "s",
    description: "Time the host took to answer a suspension, by kind.",
};

/// Session dump size.
static SNAPSHOT_BYTES: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.snapshot.bytes",
    unit: "By",
    description: "Size of a session dump.",
};

/// Sandbox output volume.
static PRINT_BYTES: Instrument = Instrument {
    kind: MetricKind::Counter,
    name: "monty.print.bytes",
    unit: "By",
    description: "Bytes the sandbox printed, by stream.",
};

/// Protocol frame size.
static FRAME_BYTES: Instrument = Instrument {
    kind: MetricKind::Histogram,
    name: "monty.wire.frame.bytes",
    unit: "By",
    description: "Size of one protocol frame, by direction.",
};

/// The constant half of an instrument, shared by every measurement it records.
struct Instrument {
    kind: MetricKind,
    name: &'static str,
    unit: &'static str,
    description: &'static str,
}

/// Records pool measurements into a Rust or foreign-host metrics SDK.
///
/// Put on [`PoolConfig::metrics`](crate::PoolConfig::metrics); cheap to clone
/// (one `Arc`). Foreign-language bindings obtain it from
/// [`TelemetryAdapterHandle::metrics`](crate::telemetry::TelemetryAdapterHandle::metrics),
/// while Rust hosts construct one with [`Metrics::for_logfire`].
#[derive(Clone)]
pub struct Metrics(Arc<Sink>);

/// Where raw measurements are recorded.
enum Sink {
    /// Pushed one at a time to the foreign host that owns aggregation.
    Adapter(Arc<dyn TelemetryAdapter>),
    /// Recorded directly into a Rust host's configured instruments.
    Logfire(Instruments),
}

impl Metrics {
    /// Sends each measurement to a foreign host's telemetry adapter.
    pub(super) fn for_adapter(adapter: Arc<dyn TelemetryAdapter>) -> Self {
        Self(Arc::new(Sink::Adapter(adapter)))
    }

    /// Builds Monty's instruments on a configured `Logfire` meter.
    ///
    /// Create one and clone it per pool: the worker counters then sum over all
    /// pools and sessions that record through the handle.
    #[must_use]
    pub fn for_logfire(logfire: Logfire) -> Self {
        Self(Arc::new(Sink::Logfire(Instruments::new(logfire))))
    }

    /// Adjusts the number of live workers across all pools using this handle.
    pub(crate) fn live_workers(&self, delta: i64) {
        self.record(&LIVE_WORKERS, MetricValue::I64(delta), &[]);
    }

    /// Adjusts the number of workers immediately available for checkout.
    pub(crate) fn idle_workers(&self, delta: i64) {
        self.record(&IDLE_WORKERS, MetricValue::I64(delta), &[]);
    }

    /// Adjusts the number of workers waiting for the host to resume them.
    fn suspended_workers(&self, delta: i64) {
        self.record(&SUSPENDED_WORKERS, MetricValue::I64(delta), &[]);
    }

    /// Time [`Pool::checkout`](crate::Pool::checkout) spent obtaining a worker.
    ///
    /// The saturation signal: a non-zero `waited` tail means `max_processes`
    /// is below what the workload needs, and `exhausted` is already a
    /// user-visible failure.
    pub(crate) fn checkout_wait(&self, elapsed: Duration, outcome: &'static str) {
        self.record(
            &CHECKOUT_WAIT,
            MetricValue::seconds(elapsed),
            &[KeyValue::new("outcome", outcome)],
        );
    }

    /// One worker leaving the pool, by why it left. `crash` versus `oom`
    /// separates "our bug" from "the sandboxed code asked for too much".
    pub(crate) fn worker_terminated(&self, reason: &'static str) {
        self.record(
            &WORKER_TERMINATED,
            MetricValue::I64(1),
            &[KeyValue::new("reason", reason)],
        );
    }

    /// Lifetime of one checkout, from `Configure` to `finish` (or to the drop
    /// that abandoned it). With the checkout rate this sizes the pool.
    pub(crate) fn session_duration(&self, elapsed: Duration, outcome: &'static str) {
        self.record(
            &SESSION_DURATION,
            MetricValue::seconds(elapsed),
            &[KeyValue::new("outcome", outcome)],
        );
    }

    /// Records one measurement into its configured host.
    fn record(&self, instrument: &Instrument, value: MetricValue, attributes: &[KeyValue]) {
        match self.0.as_ref() {
            Sink::Adapter(adapter) => adapter.record_metric(&Measurement {
                kind: instrument.kind,
                name: instrument.name,
                unit: instrument.unit,
                description: instrument.description,
                value,
                attributes,
            }),
            Sink::Logfire(instruments) => instruments.record(instrument, value, attributes),
        }
    }
}

impl fmt::Debug for Metrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Metrics")
    }
}

/// One raw measurement and the metadata needed to create its instrument.
pub struct Measurement<'a> {
    /// Which kind of instrument records this measurement.
    pub kind: MetricKind,
    /// Dotted instrument name, e.g. `monty.pool.checkout.wait`.
    pub name: &'static str,
    /// UCUM unit: `s`, `By`, `1`, or a `{thing}` annotation for counts.
    pub unit: &'static str,
    /// One-line description of what the instrument measures.
    pub description: &'static str,
    /// The measured value.
    pub value: MetricValue,
    /// Low-cardinality dimensions to record it under.
    pub attributes: &'a [KeyValue],
}

/// The kind of host instrument a definition builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricKind {
    /// Monotonic sum: the value is an increment.
    Counter,
    /// Non-monotonic sum: the value adjusts a current count.
    UpDownCounter,
    /// Distribution: the value is one sample.
    Histogram,
}

/// A measured value, integral for counts and byte sizes, floating for
/// durations and ratios.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MetricValue {
    /// An integral count.
    I64(i64),
    /// A duration in seconds, or a ratio.
    F64(f64),
}

impl MetricValue {
    /// A duration as fractional seconds, the unit OTel durations use.
    fn seconds(duration: Duration) -> Self {
        Self::F64(duration.as_secs_f64())
    }

    /// A byte count, saturating rather than wrapping on absurd sizes.
    fn bytes(len: usize) -> Self {
        Self::count(len)
    }

    /// A count of things, saturating rather than wrapping.
    fn count(n: usize) -> Self {
        Self::I64(i64::try_from(n).unwrap_or(i64::MAX))
    }

    /// The value as a histogram sample.
    fn as_f64(self) -> f64 {
        match self {
            Self::I64(value) => value as f64,
            Self::F64(value) => value,
        }
    }

    /// The value as an up/down-counter adjustment. Only durations are `F64`,
    /// and no up/down counter records one, so the saturating cast never runs.
    #[expect(clippy::cast_possible_truncation, reason = "float→int casts saturate")]
    fn as_i64(self) -> i64 {
        match self {
            Self::I64(value) => value,
            Self::F64(value) => value as i64,
        }
    }

    /// The value as a counter increment; a negative one would be a bug in a
    /// caller, and saturates to zero rather than wrapping into a huge count.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "float→int casts saturate, including negatives to zero"
    )]
    fn as_u64(self) -> u64 {
        match self {
            Self::I64(value) => u64::try_from(value).unwrap_or(0),
            Self::F64(value) => value as u64,
        }
    }
}

/// The `outcome` attribute for an operation that either worked or did not.
pub(crate) const fn outcome(ok: bool) -> &'static str {
    if ok { "ok" } else { "error" }
}

/// Instruments built from a Rust host's meter, on first use of each.
///
/// Lazy rather than a registered list built up front: an instrument then needs
/// no bookkeeping beyond its own [`Instrument`], so one that only a rare code
/// path records can never be forgotten and silently dropped. The read lock is
/// the steady state — building happens at most once per instrument per handle.
struct Instruments {
    logfire: Logfire,
    built: RwLock<HashMap<&'static str, Handle>>,
}

impl Instruments {
    fn new(logfire: Logfire) -> Self {
        Self {
            logfire,
            built: RwLock::new(HashMap::new()),
        }
    }

    /// Records into `instrument`'s handle, building it if this is its first use.
    fn record(&self, instrument: &Instrument, value: MetricValue, attributes: &[KeyValue]) {
        {
            let built = self.built.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(handle) = built.get(instrument.name) {
                handle.record(value, attributes);
                return;
            }
        }
        // built under the write lock, not before it: two handles for one
        // exponential histogram would share a scale registration, and dropping
        // the loser would deregister the winner's scale
        let mut built = self.built.write().unwrap_or_else(PoisonError::into_inner);
        built
            .entry(instrument.name)
            .or_insert_with(|| self.build(instrument))
            .record(value, attributes);
    }

    /// Creates one instrument on the host's meter.
    fn build(&self, instrument: &Instrument) -> Handle {
        let metrics = self.logfire.metrics();
        match instrument.kind {
            MetricKind::Counter => Handle::Counter(
                metrics
                    .u64_counter(instrument.name)
                    .with_unit(instrument.unit)
                    .with_description(instrument.description)
                    .build(),
            ),
            MetricKind::UpDownCounter => Handle::UpDownCounter(
                metrics
                    .i64_up_down_counter(instrument.name)
                    .with_unit(instrument.unit)
                    .with_description(instrument.description)
                    .build(),
            ),
            // exponential, not the SDK's default buckets: those are scaled for
            // seconds-long requests and would put every monty turn in the
            // first bucket
            MetricKind::Histogram => Handle::Histogram(
                metrics
                    .f64_exponential_histogram(instrument.name, MAX_HISTOGRAM_SCALE)
                    .with_unit(instrument.unit)
                    .with_description(instrument.description)
                    .build(),
            ),
        }
    }
}

/// Upper bound on exponential histogram resolution; the SDK downscales from
/// here as a distribution widens, so this only says "as fine as OTel allows".
const MAX_HISTOGRAM_SCALE: i8 = 20;

/// One built instrument, typed by the kind that produced it.
enum Handle {
    Counter(Counter<u64>),
    UpDownCounter(UpDownCounter<i64>),
    Histogram(ExponentialHistogram<f64>),
}

impl Handle {
    fn record(&self, value: MetricValue, attributes: &[KeyValue]) {
        match self {
            Self::Counter(counter) => counter.add(value.as_u64(), attributes),
            Self::UpDownCounter(counter) => counter.add(value.as_i64(), attributes),
            Self::Histogram(histogram) => histogram.record(value.as_f64(), attributes),
        }
    }
}

/// Records one worker's turns, mirroring the protocol the way
/// [`crate::telemetry::tracing::Recorder`] mirrors it for spans.
///
/// A separate state machine rather than a branch inside that recorder, because
/// metrics must also run for untraced checkouts. Lives on the
/// [`Worker`](crate::worker::Worker), which sees every request and event.
pub(crate) struct TurnMetrics {
    metrics: Metrics,
    /// When the in-flight feed started; `None` between feeds, and for a feed
    /// restored mid-suspension (whose start this process never saw).
    feed: Option<Instant>,
    /// Whether a feed is running, including one restored mid-suspension.
    run_active: bool,
    /// The suspension the feed is blocked on, timing the host round-trip.
    /// While this is set, the worker counts towards [`SUSPENDED_WORKERS`].
    pending: Option<Suspension>,
    /// The in-flight housekeeping turn: what to label it, and when it started.
    turn: Option<(&'static str, Instant)>,
    /// Cumulative sandbox execution time as last reported, so each run
    /// contributes its own delta instead of the session total.
    reported_micros: u64,
}

impl TurnMetrics {
    /// Creates the per-worker recorder, which outlives the checkouts it serves.
    pub(crate) const fn new(metrics: Metrics) -> Self {
        Self {
            metrics,
            feed: None,
            run_active: false,
            pending: None,
            turn: None,
            reported_micros: 0,
        }
    }

    /// Records the size of one frame that reached the wire.
    pub(crate) fn frame(&self, direction: &'static str, len: usize) {
        self.metrics.record(
            &FRAME_BYTES,
            MetricValue::bytes(len),
            &[KeyValue::new("direction", direction)],
        );
    }

    /// Starts timing one turn; called once the request is on the wire.
    ///
    /// The resume family opens nothing — it *answers* the open suspension, so
    /// it closes that instead, which is what makes the recorded duration the
    /// host round-trip.
    pub(crate) fn begin_turn(&mut self, request: &pb::ParentRequest) {
        let now = Instant::now();
        match &request.kind {
            // a new session on this worker; its execution clock restarts, so
            // the delta ratchet has to as well
            Some(pb::parent_request::Kind::Configure(_)) => {
                self.feed = None;
                self.run_active = false;
                self.abandon_pending();
                self.turn = Some(("configure", now));
                self.reported_micros = 0;
            }
            Some(pb::parent_request::Kind::Feed(_)) => {
                self.abandon_pending();
                self.feed = Some(now);
                self.run_active = true;
            }
            Some(pb::parent_request::Kind::Load(_)) => {
                // a load adopts the dumped session's clock, which may be ahead
                // of or behind this worker's; the first reply re-bases it
                self.run_active = false;
                self.reported_micros = 0;
                self.turn = Some(("load", now));
            }
            Some(pb::parent_request::Kind::Dump(_)) => self.turn = Some(("dump", now)),
            Some(pb::parent_request::Kind::InstallDependencies(_)) => {
                self.turn = Some(("install_dependencies", now));
            }
            Some(pb::parent_request::Kind::Reset(_)) => {
                self.feed = None;
                self.run_active = false;
                self.abandon_pending();
                self.turn = Some(("reset", now));
            }
            // no reply is awaited, so nothing is left open to close
            Some(pb::parent_request::Kind::Shutdown(_)) => {
                self.feed = None;
                self.run_active = false;
                self.abandon_pending();
                self.turn = None;
            }
            Some(pb::parent_request::Kind::ResumeCall(r)) => self.close_suspension(ext_result(r.result.as_ref())),
            Some(pb::parent_request::Kind::ResumeNameLookup(r)) => {
                let outcome = match r.kind {
                    Some(pb::resume_name_lookup::Kind::Value(_)) => "value",
                    Some(pb::resume_name_lookup::Kind::Undefined(_)) => "undefined",
                    Some(pb::resume_name_lookup::Kind::Error(_)) => "error",
                    None => "missing",
                };
                self.close_suspension(outcome);
            }
            Some(pb::parent_request::Kind::ResumeFutures(_)) => self.close_suspension("resolved"),
            Some(pb::parent_request::Kind::AbortFeed(_)) => self.close_suspension("aborted"),
            None => {}
        }
    }

    /// Records one event from the worker: a suspension opens the pending
    /// round-trip, a turn-ending event closes the run or housekeeping turn.
    pub(crate) fn event(&mut self, event: &pb::ChildEvent) {
        // a load's reply stamps the restored session's cumulative clock, spent
        // in another process: re-base the delta ratchet so the next run
        // records only its own cost
        if matches!(self.turn, Some(("load", _))) {
            self.reported_micros = self.reported_micros.max(event.total_execution_micros);
        }
        match &event.kind {
            Some(pb::child_event::Kind::Print(p)) => self.metrics.record(
                &PRINT_BYTES,
                MetricValue::bytes(p.text.len()),
                &[KeyValue::new("stream", print_stream(p.stream))],
            ),
            Some(pb::child_event::Kind::FunctionCall(_)) => self.suspend(SuspensionKind::FunctionCall),
            Some(pb::child_event::Kind::OsCall(c)) => self.suspend(SuspensionKind::OsCall(os_call(c.call.as_ref()))),
            Some(pb::child_event::Kind::NameLookup(_)) => self.suspend(SuspensionKind::NameLookup),
            Some(pb::child_event::Kind::ResolveFutures(_)) => self.suspend(SuspensionKind::ResolveFutures),
            Some(pb::child_event::Kind::Complete(_)) => self.end_run("complete", event),
            Some(pb::child_event::Kind::Error(_)) => {
                // an error answering an open housekeeping turn (`Dump`,
                // `Load`, `InstallDependencies`, ...) is that turn's outcome,
                // not a run's — no run happened, so recording an execution
                // sample would fake one. A failed dump even leaves the feed
                // suspended and resumable.
                if self.turn.is_some() {
                    self.end_turn("error");
                } else {
                    self.end_run("error", event);
                }
            }
            Some(pb::child_event::Kind::TypingError(_)) => self.end_run("typing_error", event),
            Some(pb::child_event::Kind::DumpResult(d)) => {
                self.metrics.record(
                    &SNAPSHOT_BYTES,
                    MetricValue::bytes(d.state.len()),
                    &[KeyValue::new("op", "dump")],
                );
                self.end_turn("ok");
            }
            Some(pb::child_event::Kind::Ok(_)) => self.end_turn("ok"),
            // the worker is about to exit; the pool counts the termination
            Some(pb::child_event::Kind::FatalError(_)) => self.end_terminal("fatal_error", event),
            Some(pb::child_event::Kind::Shutdown(_)) => self.end_terminal("shutdown", event),
            None => {}
        }
    }

    /// Counts one suspension and starts timing the host's answer, during which
    /// this worker is suspended.
    fn suspend(&mut self, kind: SuspensionKind) {
        // a suspension answering `Load` is a restored feed re-raising it: the
        // load itself is done, and the host round-trip that follows must not
        // be billed to the load turn
        if matches!(self.turn, Some(("load", _))) {
            self.end_turn("ok");
        }
        self.run_active = true;
        self.metrics.record(
            &SUSPENSIONS,
            MetricValue::I64(1),
            &[KeyValue::new("kind", kind.label())],
        );
        // a suspension raised while one is open would be a protocol violation
        // the checkout rejects; releasing first keeps the count honest anyway
        self.abandon_pending();
        self.pending = Some(Suspension {
            start: Instant::now(),
            kind,
        });
        self.metrics.suspended_workers(1);
    }

    /// Records the round-trip the answering resume just closed, under its kind
    /// and outcome alone — never the name of what was called, which the
    /// sandboxed code chooses (see the module docs).
    fn close_suspension(&mut self, outcome: &'static str) {
        let Some(suspension) = self.take_pending() else {
            return;
        };
        let elapsed = MetricValue::seconds(suspension.start.elapsed());
        let mut attributes = vec![
            KeyValue::new("kind", suspension.kind.label()),
            KeyValue::new("outcome", outcome),
        ];
        // only the os call names what it did, and only from the protocol's own
        // fixed set — see the cardinality rule in the module docs
        if let SuspensionKind::OsCall(function) = suspension.kind {
            attributes.push(KeyValue::new("function", function));
        }
        self.metrics.record(&EXT_CALL, elapsed, &attributes);
    }

    /// Drops the open suspension without recording a round-trip, for the turns
    /// that end one without answering it.
    fn abandon_pending(&mut self) {
        let _ = self.take_pending();
    }

    /// Takes the open suspension, releasing this worker's claim on
    /// [`SUSPENDED_WORKERS`]. The single place `pending` is cleared, so the count
    /// cannot drift from the state it describes.
    fn take_pending(&mut self) -> Option<Suspension> {
        let suspension = self.pending.take();
        if suspension.is_some() {
            self.metrics.suspended_workers(-1);
        }
        suspension
    }

    /// Ends an execution turn: its wall time, and the sandbox time it consumed.
    ///
    /// A feed restored mid-suspension has no start instant in this process, so
    /// it contributes execution time but no wall time.
    fn end_run(&mut self, outcome: &'static str, event: &pb::ChildEvent) {
        self.run_active = false;
        self.abandon_pending();
        self.end_turn(outcome);
        if let Some(start) = self.feed.take() {
            self.metrics.record(
                &RUN_DURATION,
                MetricValue::seconds(start.elapsed()),
                &[KeyValue::new("outcome", outcome)],
            );
        }
        // the reported total is cumulative for the session (and never rewinds,
        // even from a worker misreporting it), so this run's cost is the delta
        let total = event.total_execution_micros;
        let delta = total.saturating_sub(self.reported_micros);
        self.reported_micros = self.reported_micros.max(total);
        self.metrics
            .record(&RUN_EXECUTION, MetricValue::seconds(Duration::from_micros(delta)), &[]);
    }

    /// Ends a terminal event as a run or housekeeping turn.
    fn end_terminal(&mut self, outcome: &'static str, event: &pb::ChildEvent) {
        if self.run_active {
            self.end_run(outcome, event);
        } else {
            self.end_turn(outcome);
        }
    }

    /// Ends the open housekeeping turn, if there is one.
    fn end_turn(&mut self, outcome: &'static str) {
        if let Some((turn, start)) = self.turn.take() {
            self.metrics.record(
                &TURN_DURATION,
                MetricValue::seconds(start.elapsed()),
                &[KeyValue::new("turn", turn), KeyValue::new("outcome", outcome)],
            );
        }
    }
}

impl Drop for TurnMetrics {
    /// A worker killed mid-suspension is no longer waiting on the host, and
    /// nothing else will close its round-trip.
    fn drop(&mut self) {
        self.abandon_pending();
    }
}

/// The suspension a feed is blocked on while the host answers it.
struct Suspension {
    start: Instant,
    kind: SuspensionKind,
}

/// One variant per suspension the protocol has: the four child events that
/// hand control to the host and wait for a `Resume*`.
///
/// Only the OS call carries anything, and only values this crate chose. What
/// the others are *for* — the called function, the looked-up name — is named
/// by the sandboxed code, so none of it is kept (see the module docs).
enum SuspensionKind {
    FunctionCall,
    /// Carries the call's fixed name, which is the protocol's, not the
    /// sandbox's.
    OsCall(&'static str),
    NameLookup,
    ResolveFutures,
}

impl SuspensionKind {
    /// Value of the `kind` attribute this suspension is recorded under.
    const fn label(&self) -> &'static str {
        match self {
            Self::FunctionCall => "function",
            Self::OsCall(_) => "os",
            Self::NameLookup => "name_lookup",
            Self::ResolveFutures => "futures",
        }
    }
}

/// Classifies the host's answer to a call suspension.
fn ext_result(result: Option<&pb::ExtFunctionResult>) -> &'static str {
    match result.and_then(|result| result.kind.as_ref()) {
        Some(pb::ext_function_result::Kind::ReturnValue(_)) => "value",
        Some(pb::ext_function_result::Kind::Error(_)) => "error",
        Some(pb::ext_function_result::Kind::Future(_)) => "future",
        Some(pb::ext_function_result::Kind::NotFound(_)) => "not_found",
        Some(pb::ext_function_result::Kind::NotHandled(_)) => "not_handled",
        None => "missing",
    }
}

/// The fixed name of an OS call — the protocol's own, so it is safe as an
/// attribute where anything the sandbox names is not.
///
/// The names match the `os call {function}` spans in
/// [`tracing`](crate::telemetry::tracing); keep the two lists in step.
fn os_call(call: Option<&Call>) -> &'static str {
    match call {
        Some(Call::Exists(_)) => "exists",
        Some(Call::IsFile(_)) => "is_file",
        Some(Call::IsDir(_)) => "is_dir",
        Some(Call::IsSymlink(_)) => "is_symlink",
        Some(Call::ReadText(_)) => "read_text",
        Some(Call::ReadBytes(_)) => "read_bytes",
        Some(Call::Stat(_)) => "stat",
        Some(Call::Iterdir(_)) => "iterdir",
        Some(Call::Resolve(_)) => "resolve",
        Some(Call::Absolute(_)) => "absolute",
        Some(Call::Unlink(_)) => "unlink",
        Some(Call::Rmdir(_)) => "rmdir",
        Some(Call::WriteText(_)) => "write_text",
        Some(Call::AppendText(_)) => "append_text",
        Some(Call::WriteBytes(_)) => "write_bytes",
        Some(Call::AppendBytes(_)) => "append_bytes",
        Some(Call::Open(_)) => "open",
        Some(Call::Mkdir(_)) => "mkdir",
        Some(Call::Rename(_)) => "rename",
        Some(Call::Getenv(_)) => "getenv",
        Some(Call::GetEnviron(_)) => "get_environ",
        Some(Call::DateToday(_)) => "date_today",
        Some(Call::DateTimeNow(_)) => "date_time_now",
        None => "unknown",
    }
}

/// The name of a `PrintStream` enum value.
fn print_stream(stream: i32) -> &'static str {
    match pb::PrintStream::try_from(stream) {
        Ok(pb::PrintStream::Stdout) => "stdout",
        Ok(pb::PrintStream::Stderr) => "stderr",
        _ => "unspecified",
    }
}
// tests live here rather than in `tests/` because `TurnMetrics` is
// crate-private: recording is a side effect of the worker, not part of the
// pool's public API. `tests/metrics.rs` covers the pool-level instruments,
// which a public `Pool` does emit.
#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
        time::Duration,
    };

    use logfire::{Logfire, config::MetricsOptions};
    use monty_proto::{WireFunctionCall, pb, pb::os_call::Call};
    use monty_types::MontyObject;
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporter, PeriodicReader,
        data::{AggregatedMetrics, MetricData},
    };

    use super::{Metrics, TurnMetrics};

    /// A cumulative aggregate exported from the test's Logfire provider.
    struct Capture {
        logfire: Logfire,
        exporter: InMemoryMetricExporter,
    }

    /// One exported metric point, flattened for focused state-machine assertions.
    struct Recorded {
        name: String,
        value: Aggregate,
        attributes: Vec<(String, String)>,
    }

    /// The aggregate shapes produced by Monty's instruments.
    enum Aggregate {
        I64(i64),
        U64(u64),
        Histogram {
            count: usize,
            sum: f64,
            min: Option<f64>,
            max: Option<f64>,
        },
    }

    impl Capture {
        /// The attributes of every time series recorded under `name`.
        fn attributes(&self, name: &str) -> Vec<Vec<(String, String)>> {
            let mut series = self.select(name, |recorded| {
                let mut attributes = recorded.attributes.clone();
                attributes.sort();
                attributes
            });
            series.sort();
            series
        }

        /// The current value of an integral up/down counter.
        fn i64_sum(&self, name: &str) -> i64 {
            self.select(name, |recorded| match recorded.value {
                Aggregate::I64(value) => value,
                _ => panic!("{name} was not an i64 sum"),
            })
            .into_iter()
            .sum()
        }

        /// Total value across a monotonic counter's attribute series.
        fn u64_sum(&self, name: &str) -> u64 {
            self.select(name, |recorded| match recorded.value {
                Aggregate::U64(value) => value,
                _ => panic!("{name} was not a u64 sum"),
            })
            .into_iter()
            .sum()
        }

        /// Histogram aggregates under `name`.
        fn histograms(&self, name: &str) -> Vec<(usize, f64, Option<f64>, Option<f64>)> {
            self.select(name, |recorded| match recorded.value {
                Aggregate::Histogram { count, sum, min, max } => (count, sum, min, max),
                _ => panic!("{name} was not a histogram"),
            })
        }

        /// Whether the latest collection includes `name`.
        fn has(&self, name: &str) -> bool {
            self.records().iter().any(|recorded| recorded.name == name)
        }

        /// Selects every point from the latest cumulative collection.
        fn select<T>(&self, name: &str, map: impl Fn(&Recorded) -> T) -> Vec<T> {
            self.records()
                .iter()
                .filter(|recorded| recorded.name == name)
                .map(map)
                .collect()
        }

        /// Flushes and flattens the provider's latest cumulative collection.
        fn records(&self) -> Vec<Recorded> {
            self.exporter.reset();
            self.logfire.force_flush().unwrap();
            let exported = self.exporter.get_finished_metrics().unwrap();
            let mut records = Vec::new();
            for resource in &exported {
                for scope in resource.scope_metrics() {
                    for metric in scope.metrics() {
                        flatten(metric.name(), metric.data(), &mut records);
                    }
                }
            }
            records
        }
    }

    /// Adds each supported aggregate data point to the flattened capture.
    fn flatten(name: &str, data: &AggregatedMetrics, records: &mut Vec<Recorded>) {
        match data {
            AggregatedMetrics::I64(MetricData::Sum(sum)) => {
                for point in sum.data_points() {
                    records.push(Recorded {
                        name: name.to_owned(),
                        value: Aggregate::I64(point.value()),
                        attributes: attributes(point.attributes()),
                    });
                }
            }
            AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                for point in sum.data_points() {
                    records.push(Recorded {
                        name: name.to_owned(),
                        value: Aggregate::U64(point.value()),
                        attributes: attributes(point.attributes()),
                    });
                }
            }
            AggregatedMetrics::F64(MetricData::ExponentialHistogram(histogram)) => {
                for point in histogram.data_points() {
                    records.push(Recorded {
                        name: name.to_owned(),
                        value: Aggregate::Histogram {
                            count: point.count(),
                            sum: point.sum(),
                            min: point.min(),
                            max: point.max(),
                        },
                        attributes: attributes(point.attributes()),
                    });
                }
            }
            other => panic!("unexpected Monty metric aggregate: {other:?}"),
        }
    }

    /// Flattens OTel attributes to their stable string representation.
    fn attributes<'a>(values: impl Iterator<Item = &'a KeyValue>) -> Vec<(String, String)> {
        values.map(|kv| (kv.key.to_string(), kv.value.to_string())).collect()
    }

    /// A recorder writing into a fresh local Logfire provider.
    fn recorder() -> (TurnMetrics, Arc<Capture>) {
        let exporter = InMemoryMetricExporter::default();
        let logfire = logfire::configure()
            .local()
            .send_to_logfire(false)
            .with_metrics(Some(
                MetricsOptions::default().with_additional_reader(PeriodicReader::builder(exporter.clone()).build()),
            ))
            .finish()
            .unwrap();
        let metrics = TurnMetrics::new(Metrics::for_logfire(logfire.clone()));
        (metrics, Arc::new(Capture { logfire, exporter }))
    }

    fn request(kind: pb::parent_request::Kind) -> pb::ParentRequest {
        pb::ParentRequest {
            kind: Some(kind),
            trace_parent: None,
        }
    }

    fn event(kind: pb::child_event::Kind) -> pb::ChildEvent {
        pb::ChildEvent {
            kind: Some(kind),
            total_execution_micros: 0,
            max_duration_micros: None,
            max_suspensions: None,
            restored_script_name: None,
        }
    }

    fn feed() -> pb::ParentRequest {
        request(pb::parent_request::Kind::Feed(pb::Feed {
            code: "double(2)".to_owned(),
            inputs: vec![],
            skip_type_check: false,
        }))
    }

    fn call_event(function_name: &str) -> pb::ChildEvent {
        event(pb::child_event::Kind::FunctionCall(WireFunctionCall {
            function_name: function_name.to_owned(),
            args: vec![],
            kwargs: vec![],
            call_id: 1,
            object_id: None,
        }))
    }

    fn resume_call(kind: pb::ext_function_result::Kind) -> pb::ParentRequest {
        request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: 1,
            result: Some(pb::ExtFunctionResult { kind: Some(kind) }),
        }))
    }

    fn attribute<'a>(attributes: &'a [(String, String)], key: &str) -> Option<&'a str> {
        attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.as_str())
    }

    /// Asserts two small metric values agree despite floating-point addition.
    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    /// Applies [`assert_close`] to two optional metric bounds.
    fn assert_optional_close(actual: Option<f64>, expected: Option<f64>) {
        match (actual, expected) {
            (Some(actual), Some(expected)) => assert_close(actual, expected),
            (None, None) => {}
            _ => panic!("{actual:?} != {expected:?}"),
        }
    }

    /// One feed with one host call: the suspension is counted, the round-trip
    /// timed on its own, and the run recorded when the completion arrives.
    #[test]
    fn a_feed_records_its_run_and_its_round_trip() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&feed());
        metrics.event(&call_event("double"));
        metrics.begin_turn(&resume_call(pb::ext_function_result::Kind::ReturnValue(
            MontyObject::Int(4).into(),
        )));
        metrics.event(&event(pb::child_event::Kind::Complete(pb::Complete {
            value: Some(MontyObject::Int(4).into()),
        })));

        assert_eq!(
            capture.attributes("monty.run.suspensions"),
            [[("kind".to_owned(), "function".to_owned())]]
        );
        assert_eq!(
            capture.attributes("monty.ext.call.duration"),
            [[
                ("kind".to_owned(), "function".to_owned()),
                ("outcome".to_owned(), "value".to_owned())
            ]]
        );
        assert_eq!(
            capture.attributes("monty.run.duration"),
            [[("outcome".to_owned(), "complete".to_owned())]]
        );
        assert_eq!(capture.histograms("monty.run.execution_time")[0].0, 1);
        assert_eq!(capture.u64_sum("monty.run.suspensions"), 1);
    }

    /// The called name is chosen by the sandboxed code, so it must never reach
    /// an attribute — under *any* outcome, since a script can mint one name per
    /// call and each distinct value would cost the host a time series.
    ///
    /// `error` is the outcome that makes this more than theory: calling a
    /// method a host object does not have raises `AttributeError` there, which
    /// comes back as `error` rather than `not_found`. And `value` is no safer,
    /// because a host whose lookup is a callable resolves every name.
    #[test]
    fn sandbox_chosen_names_never_reach_attributes() {
        let (mut metrics, capture) = recorder();
        let outcomes = [
            pb::ext_function_result::Kind::NotFound("attacker_chosen".to_owned()),
            pb::ext_function_result::Kind::Error(pb::RaisedException {
                exc_type: "AttributeError".to_owned(),
                message: None,
                traceback: vec![],
                data: None,
            }),
            pb::ext_function_result::Kind::ReturnValue(MontyObject::Int(1).into()),
        ];
        metrics.begin_turn(&feed());
        for (index, kind) in outcomes.into_iter().enumerate() {
            metrics.event(&call_event(&format!("attacker_chosen_{index}")));
            metrics.begin_turn(&resume_call(kind));
        }

        for attributes in capture.attributes("monty.ext.call.duration") {
            let keys: Vec<&str> = attributes.iter().map(|(key, _)| key.as_str()).collect();
            assert_eq!(keys, ["kind", "outcome"], "{attributes:?}");
        }
        let outcomes: Vec<_> = capture
            .attributes("monty.ext.call.duration")
            .iter()
            .map(|attributes| attribute(attributes, "outcome").unwrap().to_owned())
            .collect();
        assert_eq!(outcomes, ["error", "not_found", "value"]);
    }

    /// An os call is the one suspension that names what it did — from the
    /// protocol's fixed set, so it costs a bounded number of series.
    #[test]
    fn os_calls_carry_their_protocol_name() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&feed());
        metrics.event(&event(pb::child_event::Kind::OsCall(pb::OsCall {
            call_id: 1,
            call: Some(Call::ReadText("/mnt/f.txt".to_owned())),
        })));
        metrics.begin_turn(&resume_call(pb::ext_function_result::Kind::ReturnValue(
            MontyObject::String("hello".to_owned()).into(),
        )));

        assert_eq!(
            capture.attributes("monty.ext.call.duration"),
            [[
                ("function".to_owned(), "read_text".to_owned()),
                ("kind".to_owned(), "os".to_owned()),
                ("outcome".to_owned(), "value".to_owned())
            ]]
        );
    }

    /// The worker reports its execution clock cumulatively, so each run
    /// contributes the delta rather than the session total.
    #[test]
    fn execution_time_is_the_delta_of_a_cumulative_clock() {
        let (mut metrics, capture) = recorder();
        for total in [100, 250] {
            metrics.begin_turn(&feed());
            metrics.event(&pb::ChildEvent {
                kind: Some(pb::child_event::Kind::Complete(pb::Complete { value: None })),
                total_execution_micros: total,
                max_duration_micros: None,
                max_suspensions: None,
                restored_script_name: None,
            });
        }

        let execution = capture.histograms("monty.run.execution_time");
        let [(count, sum, min, max)] = execution.as_slice() else {
            panic!("expected one execution-time series")
        };
        assert_eq!(*count, 2);
        assert_close(*sum, Duration::from_micros(250).as_secs_f64());
        assert_optional_close(*min, Some(Duration::from_micros(100).as_secs_f64()));
        assert_optional_close(*max, Some(Duration::from_micros(150).as_secs_f64()));
    }

    /// A raised exception ends the run and shows up as its outcome. The class
    /// itself is not recorded anywhere: the sandbox names it.
    #[test]
    fn an_exception_ends_the_run_without_describing_itself() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&feed());
        metrics.event(&event(pb::child_event::Kind::Error(pb::Error {
            exception: Some(pb::RaisedException {
                exc_type: "MyCustomError".to_owned(),
                message: None,
                traceback: vec![],
                data: None,
            }),
        })));

        assert_eq!(
            capture.attributes("monty.run.duration"),
            [[("outcome".to_owned(), "error".to_owned())]]
        );
    }

    /// Terminal events close the operation that received them before checkout
    /// teardown drops the worker-side recorder.
    #[test]
    fn terminal_events_end_runs_and_housekeeping_turns() {
        let terminal_events = [
            (
                pb::child_event::Kind::FatalError(pb::FatalError {
                    message: "worker failed".to_owned(),
                }),
                "fatal_error",
            ),
            (
                pb::child_event::Kind::Shutdown(pb::ShutdownDump { dump: None }),
                "shutdown",
            ),
        ];
        for (kind, outcome) in terminal_events {
            let (mut metrics, capture) = recorder();
            metrics.begin_turn(&feed());
            metrics.event(&event(kind));
            assert_eq!(
                capture.attributes("monty.run.duration"),
                [[("outcome".to_owned(), outcome.to_owned())]]
            );
            assert_eq!(capture.histograms("monty.run.execution_time")[0].0, 1);
        }

        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&request(pb::parent_request::Kind::InstallDependencies(
            pb::InstallDependencies { requirements: vec![] },
        )));
        metrics.event(&event(pb::child_event::Kind::Shutdown(pb::ShutdownDump { dump: None })));
        assert_eq!(
            capture.attributes("monty.turn.duration"),
            [[
                ("outcome".to_owned(), "shutdown".to_owned()),
                ("turn".to_owned(), "install_dependencies".to_owned())
            ]]
        );
        assert!(!capture.has("monty.run.execution_time"));
    }

    /// A restored session's cumulative clock was spent in another process, so
    /// the load's reply re-bases the ratchet and the next run records only
    /// its own delta — not the whole restored history.
    #[test]
    fn a_load_rebases_the_execution_clock() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&request(pb::parent_request::Kind::Load(pb::Load { state: vec![] })));
        metrics.event(&pb::ChildEvent {
            kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
            total_execution_micros: 10_000_000,
            max_duration_micros: None,
            max_suspensions: None,
            restored_script_name: Some("dumped.py".to_owned()),
        });
        metrics.begin_turn(&feed());
        metrics.event(&pb::ChildEvent {
            kind: Some(pb::child_event::Kind::Complete(pb::Complete { value: None })),
            total_execution_micros: 10_000_100,
            max_duration_micros: None,
            max_suspensions: None,
            restored_script_name: None,
        });

        let execution = capture.histograms("monty.run.execution_time");
        assert_eq!(execution[0].0, 1);
        assert_close(execution[0].1, Duration::from_micros(100).as_secs_f64());
    }

    /// A feed restored mid-suspension re-raises the suspension as the load's
    /// reply: that ends the load turn — the host round-trip that follows is
    /// not the load's cost — and the eventual completion contributes only the
    /// post-resume execution delta, with no wall time (the feed's start was
    /// never seen in this process).
    #[test]
    fn a_restored_suspension_closes_the_load_turn() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&request(pb::parent_request::Kind::Load(pb::Load { state: vec![] })));
        metrics.event(&pb::ChildEvent {
            kind: Some(pb::child_event::Kind::NameLookup(pb::NameLookup {
                name: "value".to_owned(),
                object_id: None,
            })),
            total_execution_micros: 10_000_000,
            max_duration_micros: None,
            max_suspensions: None,
            restored_script_name: None,
        });
        let turns = capture.attributes("monty.turn.duration");
        assert_eq!(
            turns,
            [[
                ("outcome".to_owned(), "ok".to_owned()),
                ("turn".to_owned(), "load".to_owned())
            ]]
        );

        metrics.begin_turn(&request(pb::parent_request::Kind::ResumeNameLookup(
            pb::ResumeNameLookup {
                kind: Some(pb::resume_name_lookup::Kind::Value(MontyObject::Int(1).into())),
            },
        )));
        metrics.event(&pb::ChildEvent {
            kind: Some(pb::child_event::Kind::Complete(pb::Complete { value: None })),
            total_execution_micros: 10_000_050,
            max_duration_micros: None,
            max_suspensions: None,
            restored_script_name: None,
        });
        let execution = capture.histograms("monty.run.execution_time");
        assert_eq!(execution[0].0, 1);
        assert_close(execution[0].1, Duration::from_micros(50).as_secs_f64());
        assert!(!capture.has("monty.run.duration"));
    }

    /// An error answering a housekeeping turn is that turn's outcome; no run
    /// happened, so no run instruments may record.
    #[test]
    fn a_failed_housekeeping_turn_is_not_a_run() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&request(pb::parent_request::Kind::InstallDependencies(
            pb::InstallDependencies {
                requirements: vec!["pydantic".to_owned()],
            },
        )));
        metrics.event(&event(pb::child_event::Kind::Error(pb::Error {
            exception: Some(pb::RaisedException {
                exc_type: "ValueError".to_owned(),
                message: None,
                traceback: vec![],
                data: None,
            }),
        })));

        assert_eq!(
            capture.attributes("monty.turn.duration"),
            [[
                ("outcome".to_owned(), "error".to_owned()),
                ("turn".to_owned(), "install_dependencies".to_owned())
            ]]
        );
        assert!(!capture.has("monty.run.duration"));
        assert!(!capture.has("monty.run.execution_time"));
    }

    /// Worker adjustments commute across concurrent users of one `Metrics`.
    #[test]
    fn worker_adjustments_are_commutative_under_concurrency() {
        const THREADS: usize = 8;
        const ITERATIONS: usize = 100;

        let (_, capture) = recorder();
        let metrics = Metrics::for_logfire(capture.logfire.clone());
        let barrier = Arc::new(Barrier::new(THREADS));
        let threads: Vec<_> = (0..THREADS)
            .map(|_| {
                let metrics = metrics.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..ITERATIONS {
                        metrics.live_workers(1);
                        metrics.idle_workers(1);
                        metrics.idle_workers(-1);
                        metrics.live_workers(-1);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        assert_eq!(capture.i64_sum("monty.pool.workers.live"), 0);
        assert_eq!(capture.i64_sum("monty.pool.workers.idle"), 0);
    }

    /// The suspended-worker count rises while the host owns a suspension and
    /// falls however it ends: answered, aborted, or abandoned with a dead worker.
    #[test]
    fn suspended_workers_tracks_host_round_trips() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&feed());
        metrics.event(&call_event("double"));
        assert_eq!(capture.i64_sum("monty.pool.workers.suspended"), 1);
        metrics.begin_turn(&resume_call(pb::ext_function_result::Kind::ReturnValue(
            MontyObject::Int(4).into(),
        )));
        assert_eq!(capture.i64_sum("monty.pool.workers.suspended"), 0);

        metrics.event(&call_event("double"));
        metrics.begin_turn(&request(pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
            exception: None,
        })));
        assert_eq!(capture.i64_sum("monty.pool.workers.suspended"), 0);
        assert!(capture.attributes("monty.ext.call.duration").contains(&vec![
            ("kind".to_owned(), "function".to_owned()),
            ("outcome".to_owned(), "aborted".to_owned()),
        ]));
        metrics.event(&event(pb::child_event::Kind::Error(pb::Error { exception: None })));

        // a worker dropped mid-suspension is no longer waiting on anyone
        metrics.begin_turn(&feed());
        metrics.event(&call_event("double"));
        assert_eq!(capture.i64_sum("monty.pool.workers.suspended"), 1);
        drop(metrics);
        assert_eq!(capture.i64_sum("monty.pool.workers.suspended"), 0);
    }

    /// A dump is a housekeeping turn: it reports its own size and duration and
    /// leaves the feed it interrupted open.
    #[test]
    fn a_dump_reports_its_size_without_ending_the_run() {
        let (mut metrics, capture) = recorder();
        metrics.begin_turn(&feed());
        metrics.begin_turn(&request(pb::parent_request::Kind::Dump(pb::Dump {})));
        metrics.event(&event(pb::child_event::Kind::DumpResult(pb::DumpResult {
            state: vec![0; 32],
        })));

        let snapshots = capture.histograms("monty.snapshot.bytes");
        assert_eq!(snapshots[0].0, 1);
        assert_close(snapshots[0].1, 32.0);
        let turns = capture.attributes("monty.turn.duration");
        assert_eq!(attribute(&turns[0], "turn"), Some("dump"));
        assert!(!capture.has("monty.run.duration"));
    }

    /// A Rust host records into instruments of its own rather than through an
    /// adapter: the measurements have to reach its meter provider, and the
    /// duration histograms have to come out exponential (the SDK's default
    /// buckets would put every monty turn in the first one).
    #[test]
    fn a_logfire_host_records_into_its_own_meter() {
        let exporter = InMemoryMetricExporter::default();
        let logfire = logfire::configure()
            .local()
            .send_to_logfire(false)
            .with_metrics(Some(
                MetricsOptions::default().with_additional_reader(PeriodicReader::builder(exporter.clone()).build()),
            ))
            .finish()
            .unwrap();
        let mut metrics = TurnMetrics::new(Metrics::for_logfire(logfire.clone()));

        metrics.begin_turn(&feed());
        metrics.event(&call_event("double"));
        metrics.begin_turn(&resume_call(pb::ext_function_result::Kind::ReturnValue(
            MontyObject::Int(4).into(),
        )));
        metrics.event(&event(pb::child_event::Kind::Complete(pb::Complete { value: None })));
        logfire.force_flush().unwrap();

        let exported = exporter.get_finished_metrics().unwrap();
        let mut found = Vec::new();
        for resource in &exported {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    found.push((metric.name().to_owned(), is_exponential(metric.data())));
                }
            }
        }
        found.sort();
        assert_eq!(
            found,
            [
                ("monty.ext.call.duration".to_owned(), true),
                ("monty.pool.workers.suspended".to_owned(), false),
                ("monty.run.duration".to_owned(), true),
                ("monty.run.execution_time".to_owned(), true),
                ("monty.run.suspensions".to_owned(), false),
            ]
        );
    }

    /// Whether an exported metric used base-2 exponential bucketing.
    fn is_exponential(data: &AggregatedMetrics) -> bool {
        match data {
            AggregatedMetrics::F64(data) => matches!(data, MetricData::ExponentialHistogram(_)),
            AggregatedMetrics::U64(data) => matches!(data, MetricData::ExponentialHistogram(_)),
            AggregatedMetrics::I64(data) => matches!(data, MetricData::ExponentialHistogram(_)),
        }
    }
}
