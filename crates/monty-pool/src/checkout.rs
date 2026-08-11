//! A checked-out worker: one REPL session, driven turn by turn.

use std::{
    borrow::Cow,
    future::{Future, ready},
    path::Path,
    pin::Pin,
    process::ExitStatus,
    sync::Arc,
    time::Duration,
};

use monty_fs::{MountCallOutcome, MountMode, MountRoot, MountTable, OverlayState};
use monty_proto::{FrameError, PROTOCOL_VERSION, exceeds_max_value_depth, pb, validate_requirement};
use monty_types::{
    AssertMessageAnnotations, ExcType, MONTY_VERSION, MontyException, MontyObject, OsFunctionCall, PrintStream,
    ResourceLimits, TypeCheckingConfig,
};
use tokio::{task::spawn_blocking, time::timeout};

use crate::{
    CrashCause, PoolError,
    pool::{CapacityGuard, PoolInner},
    worker::Worker,
};

/// Arguments for the REPL session a checkout creates — mirrors
/// `MontyRepl`'s constructor surface.
#[derive(Debug, Clone)]
pub struct ReplConfig {
    /// Script name used in tracebacks and type-check diagnostics.
    pub script_name: String,
    /// Sandbox resource limits enforced inside the worker. `None` means
    /// unlimited (except monty's standard recursion-depth default).
    pub limits: Option<ResourceLimits>,
    /// Type-check every fed snippet before executing it.
    pub type_check: bool,
    /// Stub declarations made available to type checking.
    pub type_check_stubs: Option<String>,
    /// How the worker renders typing diagnostics. Chosen here rather than on
    /// the raised error because the structured diagnostics never leave the
    /// worker — only the rendered text crosses the wire.
    pub type_check_config: TypeCheckingConfig,
    /// Give failed `assert` statements pytest-style introspected messages
    /// (see `limitations/assert.md`). On by default with a 120-byte
    /// operand-repr truncation; `MaxBytes` customizes the truncation.
    pub assert_message_annotations: AssertMessageAnnotations,
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self {
            script_name: "main.py".to_owned(),
            limits: None,
            type_check: false,
            type_check_stubs: None,
            type_check_config: TypeCheckingConfig::default(),
            assert_message_annotations: AssertMessageAnnotations::default(),
        }
    }
}

/// A host directory mounted into the sandbox for one feed. Mounts are handled
/// entirely on the parent: the checkout services covered filesystem OS calls
/// from the host path itself (so mounts work even when the worker runs on a
/// remote machine). Every OS call still surfaces as a [`TurnEvent::OsCall`];
/// mounts are consulted only when the caller asks, via
/// [`Checkout::resume_from_mounts`].
#[derive(Debug, Clone)]
pub struct MountSpec {
    /// The host directory, opened when this spec was built and shared by every
    /// feed that reuses it. The path is never resolved again, so sandbox code
    /// cannot make it name a different directory between feeds.
    root: MountRoot,
    /// Access mode.
    pub mode: MountSpecMode,
    /// Cap on total bytes written through this mount.
    pub write_bytes_limit: Option<u64>,
    /// Aggregate budget for retained overlay data and transient results.
    pub memory_usage_limit: u64,
}

impl MountSpec {
    /// Opens `host_path` and creates mount configuration with the default
    /// 100 MB memory budget and no cumulative write limit.
    ///
    /// Build this once and reuse it; each call resolves the path afresh. The
    /// open is blocking filesystem I/O, so an async caller opening a directory
    /// that may stall (NFS, FUSE) should either build the spec before entering
    /// the runtime or open the [`MountRoot`] under `spawn_blocking` and pass it
    /// to [`Self::from_root`]. Feeds never reopen it, so this cost is paid once
    /// per mount rather than once per feed.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Runtime`] if the virtual path is not absolute, or
    /// the host path cannot be opened as a directory.
    pub fn new(virtual_path: &str, host_path: impl AsRef<Path>, mode: MountSpecMode) -> Result<Self, PoolError> {
        let root = MountRoot::open(virtual_path, host_path).map_err(|err| PoolError::Runtime(err.into_exception()))?;
        Ok(Self::from_root(root, mode))
    }

    /// Creates mount configuration from an already-opened [`MountRoot`], for
    /// hosts that open it themselves to map failures their own way.
    #[must_use]
    pub fn from_root(root: MountRoot, mode: MountSpecMode) -> Self {
        Self {
            root,
            mode,
            write_bytes_limit: None,
            memory_usage_limit: monty_fs::DEFAULT_MEMORY_USAGE_LIMIT,
        }
    }

    /// Returns the normalized virtual path this mount answers on.
    #[must_use]
    pub fn virtual_path(&self) -> &str {
        self.root.virtual_path()
    }

    /// Returns the host directory path. Diagnostics only — operations run
    /// against the descriptor, not this path.
    #[must_use]
    pub fn host_path(&self) -> &Path {
        self.root.host_path()
    }
}

/// Access mode for a [`MountSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountSpecMode {
    ReadOnly,
    ReadWrite,
    /// Copy-on-write overlay in parent memory; writes are discarded when the
    /// feed ends.
    Overlay,
}

/// How a protocol turn ended: a suspension that needs an answer from the
/// caller, or completion of the fed snippet.
#[derive(Debug)]
pub enum TurnEvent {
    /// The sandbox called an external function — answer with
    /// [`Checkout::resume`]. When `method_call` is true this is a dataclass
    /// method call and the instance is the first argument.
    FunctionCall {
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        method_call: bool,
    },
    /// The sandbox performed an OS operation (e.g. `"Path.read_text"`).
    /// Answer it from this feed's mounts with
    /// [`Checkout::resume_from_mounts`], or directly with
    /// [`Checkout::resume`]. A caller with no handler should resume with
    /// [`ResumeValue::NotHandled`]; the sandbox then raises the call's own
    /// no-handler default.
    OsCall {
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
    },
    /// The sandbox read an undefined name — answer with
    /// [`Checkout::resume_name_lookup`].
    NameLookup { name: String },
    /// Every sandbox task is blocked on external futures — answer with
    /// [`Checkout::resume_futures`].
    ResolveFutures { pending_call_ids: Vec<u32> },
    /// The fed snippet completed with this value; the session is ready for
    /// the next [`Checkout::feed`].
    Complete(MontyObject),
}

/// The caller's answer to a [`TurnEvent::FunctionCall`] or
/// [`TurnEvent::OsCall`].
#[derive(Debug)]
pub enum ResumeValue {
    /// The call returned this value.
    Return(MontyObject),
    /// The call raised this exception.
    Error(MontyException),
    /// The call is asynchronous: register an external future and continue
    /// other tasks; resolve later via [`Checkout::resume_futures`].
    Future,
    /// No handler exists for the called name — the sandbox raises
    /// `NameError`.
    NotFound,
    /// No handler accepted this OS call — the sandbox raises the call's own
    /// no-handler default (`PermissionError` naming the path for filesystem
    /// calls, `RuntimeError` for the rest). Only valid answering a
    /// [`TurnEvent::OsCall`].
    NotHandled,
}

/// The (boxed) future an [`OnPrint`] callback returns for one fragment; the
/// turn awaits it before reading on, so a slow sink backpressures the worker.
pub type PrintFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Callback receiving sandbox `print()` output streamed during a turn.
///
/// The callback returns a future so genuinely async sinks (a JS callback, a
/// socket) can be awaited per fragment; synchronous sinks wrap themselves
/// with [`on_print_sync`]. The future must be `'static`, so it captures owned
/// copies of whatever it needs (including the text, if consumed async).
pub type OnPrint<'a> = &'a mut (dyn FnMut(PrintStream, &str) -> PrintFuture + Send);

/// Callback for the events a [`Checkout::turn_raw`] streams before the
/// turn-ender — `Print`s today. Returns a future for the same reason
/// [`OnPrint`] does: a slow sink backpressures the worker.
pub type OnRawEvent<'a> = &'a mut (dyn FnMut(&pb::ChildEvent) -> PrintFuture + Send);

/// Adapts a synchronous print sink to the [`OnPrint`] callback shape.
///
/// ```rust,no_run
/// # use monty_pool::on_print_sync;
/// let mut on_print = on_print_sync(|_stream, text| print!("{text}"));
/// // session.feed("print('hi')", vec![], vec![], false, &mut on_print).await?;
/// ```
pub fn on_print_sync<F>(mut sink: F) -> impl FnMut(PrintStream, &str) -> PrintFuture + Send
where
    F: FnMut(PrintStream, &str) + Send,
{
    move |stream, text| {
        sink(stream, text);
        Box::pin(ready(()))
    }
}

/// One worker dedicated to one REPL session.
///
/// Obtained from [`crate::Pool::checkout`]. [`Checkout::finish`] returns the
/// worker to the pool; dropping without finishing kills the worker instead —
/// mid-execution state cannot be trusted back into the pool.
///
/// # Cancellation
///
/// Turn futures (`feed`, `resume*`, `dump`, ...) are **not** resumable after
/// being dropped mid-flight: the request may already be on the wire (or a
/// mount's host I/O abandoned mid-service), so the protocol state is
/// unknowable. The checkout notices on its next call,
/// discards the worker, and fails with [`PoolError::Protocol`]; `finish` on
/// such a session likewise discards the worker rather than returning it.
pub struct Checkout {
    /// `None` after `finish()` or after the worker was discarded on error.
    worker: Option<Worker>,
    pool: Arc<PoolInner>,
    /// The suspension awaiting an answer, when mid-feed.
    pending: Option<Pending>,
    /// Set while a turn's I/O is in flight; still set on the next call only
    /// if the previous turn future was cancelled mid-I/O (see the type docs).
    turn_in_flight: bool,
    /// The session's `max_duration` budget for the parent-side backstop, when
    /// configured. Set from the config on `create`; for `restore`d sessions it
    /// is adopted from the timing fields on the worker's first reply (the limits
    /// travel inside the opaque dump).
    duration_budget: Option<Duration>,
    /// Cumulative sandbox execution time as last reported by the worker —
    /// the child's clock is the single source of truth (it runs only while
    /// the interpreter executes, never during suspensions or between feeds,
    /// and survives dump/load). Monotonic max across turns so a compromised
    /// worker cannot rewind the parent's view of its consumed budget.
    reported_execution: Duration,
    /// The deadline armed for the most recent turn, surfaced by
    /// [`PoolError::Timeout`] when it fires.
    armed_deadline: Option<Duration>,
    /// The script name a `restore` adopted, captured from the worker's `Load`
    /// reply (the name travels inside the opaque dump, so the parent learns it
    /// only by the worker echoing it). Reset at the start of each `restore` and
    /// taken by `restore` to return; unset for non-restore turns.
    restored_script_name: Option<String>,
    /// Parent-side mount table for the in-flight feed, built from the
    /// [`MountSpec`]s passed to [`Checkout::feed`] / [`Checkout::restore`].
    /// Consulted only by [`Checkout::resume_from_mounts`]. Dropped when the
    /// feed ends so overlay writes never leak into the next feed.
    feed_mounts: Option<MountTable>,
}

/// Which kind of suspension is awaiting an answer.
enum Pending {
    /// FunctionCall or OsCall; carries the call id and name (the name feeds
    /// `ResumeValue::NotFound`'s NameError).
    Call {
        call_id: u32,
        function_name: String,
        /// The typed OS call, retained so [`Checkout::resume_from_mounts`] can
        /// offer it to this feed's mount table after the caller has seen it.
        /// `None` for an external function call, which also gates
        /// [`ResumeValue::NotHandled`] — only an OS call can resolve that way.
        os_call: Option<Box<OsFunctionCall>>,
    },
    NameLookup,
    Futures,
}

impl Checkout {
    /// Sends `Configure` on a fresh worker (the worker materializes the repl
    /// lazily on the first feed, or restores one via `load_snapshot` instead).
    pub(crate) async fn create(worker: Worker, pool: Arc<PoolInner>, repl: &ReplConfig) -> Result<Self, PoolError> {
        let request = request(pb::parent_request::Kind::Configure(pb::Configure {
            script_name: repl.script_name.clone(),
            limits: repl.limits.as_ref().map(Into::into),
            type_check: repl.type_check,
            type_check_stubs: repl.type_check_stubs.clone(),
            type_check_format: pb::TypeCheckFormat::from(repl.type_check_config.format).into(),
            type_check_color: repl.type_check_config.color,
            assert_message_annotations: Some(repl.assert_message_annotations.max_bytes()),
            // What the child actually checks: it rejects a version outside the
            // range it serves with a `FatalError`. Relevant whenever the worker
            // is not the binary this crate ships — a system-packaged `monty`,
            // or a remote worker reached over a socket.
            protocol_version: PROTOCOL_VERSION,
            // Diagnostic only, so a rejection can report both builds.
            monty_version: MONTY_VERSION.to_owned(),
        }));
        let mut this = Self {
            worker: Some(worker),
            pool,
            pending: None,
            turn_in_flight: false,
            duration_budget: repl.limits.as_ref().and_then(|limits| limits.max_duration),
            reported_execution: Duration::ZERO,
            armed_deadline: None,
            restored_script_name: None,
            feed_mounts: None,
        };
        let mut no_print = on_print_sync(|_, _| {});
        let deadline = this.pool.config.request_timeout;
        match this.request_turn(&request, deadline, &mut no_print).await? {
            ControlEvent::Ok => Ok(this),
            other => Err(this.protocol_violation(format!("unexpected reply to Configure: {other:?}"))),
        }
    }

    /// Restores a dumped session into this checkout's freshly configured (but
    /// not-yet-fed) worker, returning the re-announced suspension event when the
    /// dump was taken mid-feed (`None` for an idle, between-feeds dump).
    ///
    /// This is the low-level restore both `session.load_session` (idle dumps) and
    /// `session.load_snapshot` (suspended dumps) drive: the caller inspects the
    /// returned `Option` to tell which kind of dump it was and reject a
    /// mismatch. Only valid before the worker has been fed (the child rejects a
    /// `Load` once a repl exists).
    ///
    /// `mounts` re-establish a suspended feed's mounts, which are never part of
    /// the dump (they are host configuration the parent services itself). Pass
    /// the same mounts the original feed used, so the resumed feed's covered
    /// calls can still be answered by [`Checkout::resume_from_mounts`]. A dump
    /// taken mid-OS-call re-announces the call in full, so the returned event
    /// is that same [`TurnEvent::OsCall`] — restoring never answers it here.
    /// The session's resource budget is taken from the dump, so the prior
    /// `Configure` limits are dropped here and re-adopted from the worker's
    /// reply.
    ///
    /// Returns the re-announced suspension (`Some` — a suspended dump) or `None`
    /// (an idle dump), paired with the worker's adopted script name (the dump's,
    /// not the `Configure` one), which the parent surfaces in restored snapshots.
    pub async fn restore(
        &mut self,
        state: Vec<u8>,
        mounts: Vec<MountSpec>,
        on_print: OnPrint<'_>,
    ) -> Result<(Option<TurnEvent>, Option<String>), PoolError> {
        self.ensure_ready()?;
        let feed_mounts = Self::build_feed_mounts(mounts);
        // the dump carries its own limits/consumed time/script name — forget
        // what the worker's Configure established and re-adopt from the reply
        self.pending = None;
        self.duration_budget = None;
        self.reported_execution = Duration::ZERO;
        self.restored_script_name = None;
        self.feed_mounts = feed_mounts;
        let request = request(pb::parent_request::Kind::Load(pb::Load { state }));
        let event = match self
            .request_turn(&request, self.pool.config.request_timeout, on_print)
            .await?
        {
            ControlEvent::Ok => None,
            ControlEvent::Turn(event) => Some(event),
            other @ ControlEvent::Dump(_) => {
                return Err(self.protocol_violation(format!("unexpected reply to Load: {other:?}")));
            }
        };
        Ok((event, self.restored_script_name.take()))
    }

    /// Executes one snippet against the session. Inputs become sandbox
    /// globals; mounts apply to this feed only and are serviced by the parent
    /// (an invalid host path fails here, before any frame is sent, as a
    /// session-preserving [`PoolError::Runtime`]). Returns the first
    /// suspension (or completion); `print()` output streams to `on_print`
    /// throughout.
    ///
    /// # Errors
    /// [`PoolError::Runtime`] / [`PoolError::Typing`] leave the session
    /// usable; all other errors mean the worker was discarded.
    pub async fn feed(
        &mut self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        mounts: Vec<MountSpec>,
        skip_type_check: bool,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        if self.pending.is_some() {
            return Err(PoolError::Protocol(
                "feed called while a suspension is awaiting an answer".into(),
            ));
        }
        ensure_sendable(inputs.iter().map(|(_, value)| value))?;
        self.feed_mounts = Self::build_feed_mounts(mounts);
        let request = request(pb::parent_request::Kind::Feed(pb::Feed {
            code: code.to_owned(),
            inputs: inputs
                .into_iter()
                .map(|(name, value)| pb::NamedValue {
                    name,
                    value: Some(value.into()),
                })
                .collect(),
            skip_type_check,
        }));
        self.expect_turn(&request, on_print).await
    }

    /// Answers a [`TurnEvent::FunctionCall`] or [`TurnEvent::OsCall`].
    pub async fn resume(&mut self, value: ResumeValue, on_print: OnPrint<'_>) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        let Some(Pending::Call {
            call_id,
            function_name,
            os_call,
        }) = &self.pending
        else {
            return Err(PoolError::Protocol("no suspended call to resume".into()));
        };
        let (call_id, function_name, is_os_call) = (*call_id, function_name.clone(), os_call.is_some());
        if matches!(value, ResumeValue::NotHandled) && !is_os_call {
            return Err(PoolError::Protocol(
                "NotHandled is only valid answering an OS call".into(),
            ));
        }
        if let ResumeValue::Return(obj) = &value {
            ensure_sendable([obj])?;
        }
        let result = match value {
            ResumeValue::Return(obj) => pb::ext_function_result::Kind::ReturnValue(obj.into()),
            ResumeValue::Error(exc) => pb::ext_function_result::Kind::Error((&exc).into()),
            ResumeValue::Future => pb::ext_function_result::Kind::Future(call_id),
            ResumeValue::NotFound => pb::ext_function_result::Kind::NotFound(function_name),
            ResumeValue::NotHandled => pb::ext_function_result::Kind::NotHandled(pb::Unit {}),
        };
        let request = request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id,
            result: Some(pb::ExtFunctionResult { kind: Some(result) }),
        }));
        // `pending` is deliberately NOT cleared here: an oversize answer is
        // rejected by `Worker::send` before any bytes reach the child, and
        // `request_turn` surfaces it with the suspension still answerable.
        // Every turn-ending reply overwrites `pending` anyway.
        self.expect_turn(&request, on_print).await
    }

    /// Answers a pending [`TurnEvent::OsCall`] from this feed's mounts, when
    /// they cover it.
    ///
    /// `Ok(None)` means no mount covers the call (or the feed has none): the
    /// suspension is left intact for the caller to answer itself, typically via
    /// its own `os` handler and then [`Checkout::resume`]. `Ok(Some(event))`
    /// means a mount serviced the call — including servicing it into an error
    /// such as `PermissionError` — and the feed ran on to `event`.
    ///
    /// This is how mounts are reached now that every OS call surfaces: an
    /// auto-answering driver tries mounts first and falls back to its handler,
    /// while a caller driving suspensions by hand can ignore mounts entirely.
    /// Path containment inside covered calls is enforced by the [`MountTable`].
    ///
    /// Covered calls perform real host filesystem I/O, serviced on tokio's
    /// blocking pool so a stalled volume cannot pin a runtime worker. Dropping
    /// this future mid-servicing abandons the feed's mount state and is
    /// treated exactly like a cancellation mid-turn: the worker is discarded
    /// on the next call.
    pub async fn resume_from_mounts(&mut self, on_print: OnPrint<'_>) -> Result<Option<TurnEvent>, PoolError> {
        self.ensure_ready()?;
        let Some(Pending::Call { os_call, .. }) = &mut self.pending else {
            return Err(PoolError::Protocol("no suspended call to resume".into()));
        };
        let Some(call) = os_call.take() else {
            return Err(PoolError::Protocol(
                "resume_from_mounts is only valid answering an OS call".into(),
            ));
        };
        // The call is *moved* into the table so a covered write's payload
        // reaches overlay storage without a copy; an uncovered call comes back
        // unchanged and is put back for the caller to answer.
        let outcome = match self.feed_mounts.take() {
            Some(mut mounts) => {
                // Table and call move into the blocking task and back out.
                // `turn_in_flight` spans the await: a caller cancelling here
                // abandons both, condemning the worker via the same mechanism
                // as a cancellation mid-turn-I/O (see `request_turn`).
                self.turn_in_flight = true;
                let (mounts, outcome) = self
                    .run_blocking(move || {
                        let outcome = mounts.handle_os_call(*call);
                        (mounts, outcome)
                    })
                    .await?;
                self.feed_mounts = Some(mounts);
                self.turn_in_flight = false;
                outcome
            }
            None => MountCallOutcome::NotHandled(*call),
        };
        match outcome {
            MountCallOutcome::Handled(result) => {
                let value = match result {
                    Ok(obj) => ResumeValue::Return(obj),
                    Err(err) => ResumeValue::Error(err.into_exception()),
                };
                match self.resume(value, &mut *on_print).await {
                    // The result never reached the child (too large or too deep
                    // to encode), so the call is still suspended — answer it
                    // with that error instead, letting the sandbox raise a
                    // catchable exception rather than stranding the feed. Only
                    // a rejection before the frame is written leaves `pending`
                    // set, so this cannot catch a genuine sandbox exception.
                    Err(PoolError::Runtime(exc)) if self.pending.is_some() => {
                        self.resume(ResumeValue::Error(exc), on_print).await.map(Some)
                    }
                    other => other.map(Some),
                }
            }
            MountCallOutcome::NotHandled(call) => {
                let Some(Pending::Call { os_call, .. }) = &mut self.pending else {
                    unreachable!("checked above");
                };
                *os_call = Some(Box::new(call));
                Ok(None)
            }
        }
    }

    /// Answers a [`TurnEvent::NameLookup`]: `Some(value)` resolves the name,
    /// `None` makes the sandbox raise `NameError`.
    pub async fn resume_name_lookup(
        &mut self,
        value: Option<MontyObject>,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        if !matches!(self.pending, Some(Pending::NameLookup)) {
            return Err(PoolError::Protocol("no suspended name lookup to resume".into()));
        }
        if let Some(obj) = &value {
            ensure_sendable([obj])?;
        }
        let kind = match value {
            Some(obj) => pb::resume_name_lookup::Kind::Value(obj.into()),
            None => pb::resume_name_lookup::Kind::Undefined(pb::Unit {}),
        };
        let request = request(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
            kind: Some(kind),
        }));
        // `pending` left set — see the comment in [`Self::resume`]
        self.expect_turn(&request, on_print).await
    }

    /// Answers a [`TurnEvent::ResolveFutures`] with results for some or all
    /// pending call ids. Each result must be `Return` or `Error` — a future
    /// cannot resolve to another future or to "not found".
    pub async fn resume_futures(
        &mut self,
        results: Vec<(u32, ResumeValue)>,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        if !matches!(self.pending, Some(Pending::Futures)) {
            return Err(PoolError::Protocol("no suspended futures to resume".into()));
        }
        let results = results
            .into_iter()
            .map(|(call_id, value)| {
                if let ResumeValue::Return(obj) = &value {
                    ensure_sendable([obj])?;
                }
                let kind = match value {
                    ResumeValue::Return(obj) => pb::ext_function_result::Kind::ReturnValue(obj.into()),
                    ResumeValue::Error(exc) => pb::ext_function_result::Kind::Error((&exc).into()),
                    ResumeValue::Future | ResumeValue::NotFound | ResumeValue::NotHandled => {
                        return Err(PoolError::Protocol(
                            format!("future {call_id} must resolve to Return or Error").into(),
                        ));
                    }
                };
                Ok(pb::FutureResult {
                    call_id,
                    result: Some(pb::ExtFunctionResult { kind: Some(kind) }),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let request = request(pb::parent_request::Kind::ResumeFutures(pb::ResumeFutures { results }));
        // `pending` left set — see the comment in [`Self::resume`]
        self.expect_turn(&request, on_print).await
    }

    /// Installs third-party Python packages into the session, making them
    /// importable by subsequent feeds. Session-scoped and repeatable; an empty
    /// `requirements` list is a no-op.
    ///
    /// Only an embedded-CPython worker honors this. The
    /// `monty` sandbox worker has no host interpreter to install for and a uv
    /// install failure both surface as [`PoolError::Runtime`] (the latter
    /// carrying uv's stderr); the session stays usable in either case. Bounded
    /// by the pool's `request_timeout`, so raise it for large dependency sets.
    ///
    /// Each requirement is validated here, at the pool boundary, before any
    /// frame is sent: a string that uv would parse as an option rather than a
    /// package specifier is rejected with [`PoolError::Runtime`] (a
    /// `ValueError`). See [`validate_requirement`] for the rationale.
    pub async fn install_dependencies(&mut self, requirements: Vec<String>) -> Result<(), PoolError> {
        self.ensure_ready()?;
        if self.pending.is_some() {
            return Err(PoolError::Protocol(
                "install_dependencies called while a suspension is awaiting an answer".into(),
            ));
        }
        // Installing nothing trivially succeeds on any worker — including the
        // sandbox worker, which would otherwise reject the request outright.
        if requirements.is_empty() {
            return Ok(());
        }
        for requirement in &requirements {
            validate_requirement(requirement).map_err(invalid_requirement)?;
        }
        let request = request(pb::parent_request::Kind::InstallDependencies(pb::InstallDependencies {
            requirements,
        }));
        let mut no_print = on_print_sync(|_, _| {});
        let deadline = self.pool.config.request_timeout;
        match self.request_turn(&request, deadline, &mut no_print).await? {
            ControlEvent::Ok => Ok(()),
            other => Err(self.protocol_violation(format!("unexpected reply to InstallDependencies: {other:?}"))),
        }
    }

    /// Serializes the session (idle or suspended) into opaque bytes that
    /// [`Checkout::restore`] can restore — including into a
    /// different worker after this one crashes. The session stays live.
    pub async fn dump(&mut self) -> Result<Vec<u8>, PoolError> {
        let request = request(pb::parent_request::Kind::Dump(pb::Dump {}));
        let mut no_print = on_print_sync(|_, _| {});
        let deadline = self.pool.config.request_timeout;
        match self.request_turn(&request, deadline, &mut no_print).await? {
            ControlEvent::Dump(state) => Ok(state),
            other => Err(self.protocol_violation(format!("unexpected reply to Dump: {other:?}"))),
        }
    }

    /// Ends the session and returns the worker to the pool.
    ///
    /// Consumes the checkout. On error the worker is discarded (and the
    /// error reported), but the pool remains healthy either way.
    pub async fn finish(mut self) -> Result<(), PoolError> {
        // A websocket worker is single-use — the pool discards it after every
        // checkout — so there is no point round-tripping a `Reset` to ready it
        // for reuse. Closing the connection (Close frame, then socket) is what
        // ends the session; the child reads it as a clean EOF and exits. Only
        // subprocess workers are reset and returned to the idle pool for the
        // next checkout.
        if self.pool.config.transport.is_websocket() {
            if let Some(mut worker) = self.worker.take() {
                // guard, not a trailing release: the worker is already out of
                // `self`, so a caller dropping this future mid-goodbye would
                // leave `Checkout::drop` with nothing to release. Disarmed
                // before `release_worker`, which releases the slot itself.
                let capacity = CapacityGuard::new(&self.pool);
                worker.close_transport().await;
                capacity.disarm();
                self.pool.release_worker(worker);
            }
            return Ok(());
        }
        let request = request(pb::parent_request::Kind::Reset(pb::Reset {}));
        let mut no_print = on_print_sync(|_, _| {});
        let deadline = self.pool.config.request_timeout;
        match self.request_turn(&request, deadline, &mut no_print).await? {
            ControlEvent::Ok => {
                if let Some(mut worker) = self.worker.take() {
                    worker.checkouts_served += 1;
                    self.pool.release_worker(worker);
                }
                Ok(())
            }
            other => Err(self.protocol_violation(format!("unexpected reply to Reset: {other:?}"))),
        }
    }

    /// OS process id of the worker, when it is a local subprocess (`None` for a
    /// remote WebSocket worker, or a finished checkout). Diagnostics/tests.
    pub fn pid(&self) -> Option<u32> {
        self.worker.as_ref().and_then(Worker::pid)
    }

    /// Sends a request and requires the reply to be a [`TurnEvent`].
    ///
    /// This is the entry point for *execution* turns (feed/resume — the
    /// turns where the sandbox runs code), so the deadline includes
    /// [`Self::backstop_deadline`] on top of the configured request timeout.
    async fn expect_turn(
        &mut self,
        request: &pb::ParentRequest,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        let deadline = min_deadline(self.pool.config.request_timeout, self.backstop_deadline());
        match self.request_turn(request, deadline, on_print).await? {
            ControlEvent::Turn(event) => Ok(event),
            other => Err(self.protocol_violation(format!("expected a turn event, got {other:?}"))),
        }
    }

    /// Parent-side kill deadline derived from the session's `max_duration`:
    /// the execution budget remaining after the time the worker has reported
    /// consuming so far, plus the configured grace. The child enforces the
    /// limit itself with a clean `TimeoutError`; this deadline only fires
    /// when that enforcement fails (e.g. a wedged or compromised child that
    /// stops checking its clock).
    fn backstop_deadline(&self) -> Option<Duration> {
        let budget = self.duration_budget?;
        let grace = self.pool.config.duration_limit_grace?;
        Some(budget.saturating_sub(self.reported_execution) + grace)
    }

    /// Adopts the timing fields the worker stamps onto every turn-ending
    /// event. The reported total only ever ratchets up — a compromised worker
    /// must not rewind the parent's view of its consumed budget (it can still
    /// under-report, but each turn stays bounded by `budget + grace`). The
    /// budget itself is only adopted when the parent doesn't already know it,
    /// i.e. after [`Checkout::restore`].
    fn note_reported_time(&mut self, event: &pb::ChildEvent) {
        self.reported_execution = self
            .reported_execution
            .max(Duration::from_micros(event.total_execution_micros));
        if self.duration_budget.is_none() {
            self.duration_budget = event.max_duration_micros.map(Duration::from_micros);
        }
    }

    /// The core protocol turn, bounded by `deadline`: when it expires the turn
    /// I/O is abandoned (safe — `Worker::recv` is cancel-safe, partial-frame
    /// state stays in the worker) and the worker is killed and discarded.
    ///
    /// Also enforces the cancellation contract (see the type docs): a caller
    /// that dropped a previous turn future mid-I/O left the protocol state
    /// unknowable, so the worker is discarded on the next call.
    async fn request_turn(
        &mut self,
        request: &pb::ParentRequest,
        deadline: Option<Duration>,
        on_print: OnPrint<'_>,
    ) -> Result<ControlEvent, PoolError> {
        self.ensure_ready()?;
        self.turn_in_flight = true;
        self.armed_deadline = deadline;
        let outcome = match deadline {
            Some(limit) => match timeout(limit, self.turn_io(request, on_print)).await {
                Ok(outcome) => outcome,
                Err(_elapsed) => Err(self.poison_timeout().await),
            },
            None => self.turn_io(request, on_print).await,
        };
        self.turn_in_flight = false;
        outcome
    }

    /// Sends `request` and returns the child's turn-ending event, as protobuf —
    /// no conversion to [`TurnEvent`].
    ///
    /// For callers that already speak the wire (a relay bridging a remote
    /// client): rebuilding a `ChildEvent` frame from a [`TurnEvent`] means
    /// hand-inverting the whole protocol, so this hands back what the child
    /// actually sent. `on_event` sees each streamed `Print` before the
    /// turn-ender.
    ///
    /// **Bypasses this checkout's suspension bookkeeping** (`pending`,
    /// `feed_mounts`, `restored_script_name`) — never interleave with
    /// `feed`/`resume`/`restore`. Worker lifecycle, poisoning and the
    /// `max_duration` backstop work as on the typed path; a raw `Load`
    /// re-adopts the dump's budget like [`Checkout::restore`]. A `FatalError`
    /// (or WebSocket `ShutdownDump`) turn-ender is returned so the driver can
    /// forward it, but discards the worker first — later calls report
    /// [`PoolError::Finished`].
    ///
    /// # Security
    /// `request` is typically a remote client's, so it is treated as hostile:
    /// `Configure`/`Reset`/`Shutdown` are refused here (a client could
    /// otherwise `Reset` away the operator-chosen resource limits and
    /// re-`Configure` its own). A `Load`'s bytes DO reach the worker's
    /// deserialiser — the driver must verify a dump is one it issued
    /// (monty-server signs and checks them) before passing it in.
    ///
    /// # Errors
    /// As [`Checkout::feed`]: a dead worker, a protocol violation, or a turn
    /// that outlived `request_timeout` or the remaining `max_duration` budget.
    pub async fn turn_raw(
        &mut self,
        request: &pb::ParentRequest,
        on_event: OnRawEvent<'_>,
    ) -> Result<pb::ChildEvent, PoolError> {
        self.ensure_ready()?;
        // A raw client could otherwise `Reset` the child back to its default
        // (unlimited) session budget and re-`Configure` with its own limits.
        // Caller misuse, so the worker stays usable.
        if matches!(
            request.kind,
            None | Some(
                pb::parent_request::Kind::Configure(_)
                    | pb::parent_request::Kind::Reset(_)
                    | pb::parent_request::Kind::Shutdown(_)
            )
        ) {
            return Err(PoolError::Protocol(
                "lifecycle requests (Configure/Reset/Shutdown) cannot be driven through turn_raw".into(),
            ));
        }
        // As `restore`: forget the Configure-time budget and re-adopt the
        // dump's from the reply's timing fields. Snapshotted because the
        // pre-send frame-size rejection leaves the session live.
        let saved_timing = matches!(request.kind, Some(pb::parent_request::Kind::Load(_)))
            .then(|| (self.duration_budget, self.reported_execution));
        if saved_timing.is_some() {
            self.duration_budget = None;
            self.reported_execution = Duration::ZERO;
        }
        self.turn_in_flight = true;
        // as `expect_turn`: `request_timeout` alone would drop the `max_duration`
        // backstop, leaving a wedged child bounded by a timeout that may be unset
        let deadline = min_deadline(self.pool.config.request_timeout, self.backstop_deadline());
        self.armed_deadline = deadline;
        let outcome = match deadline {
            Some(limit) => match timeout(limit, self.turn_io_raw(request, on_event)).await {
                Ok(outcome) => outcome,
                Err(_elapsed) => Err(self.poison_timeout().await),
            },
            None => self.turn_io_raw(request, on_event).await,
        };
        self.turn_in_flight = false;
        // an error with the worker still alive is the pre-send frame-size
        // rejection: the `Load` never reached the child, put the budget back
        if let Some((budget, reported)) = saved_timing
            && outcome.is_err()
            && self.worker.is_some()
        {
            self.duration_budget = budget;
            self.reported_execution = reported;
        }
        outcome
    }

    /// The body of [`Self::turn_raw`]: send, stream, return the turn-ender.
    ///
    /// Mirrors `turn_io`'s failure handling, but events are returned rather
    /// than classified — including `FatalError`/`Shutdown`, which discard the
    /// worker yet still hand the frame back.
    async fn turn_io_raw(
        &mut self,
        request: &pb::ParentRequest,
        on_event: OnRawEvent<'_>,
    ) -> Result<pb::ChildEvent, PoolError> {
        let Some(worker) = self.worker.as_mut() else {
            return Err(PoolError::Finished);
        };
        if let Err(err) = worker.send(request).await {
            // an oversize frame is rejected before any bytes are written, so
            // the worker is still synced — see `turn_io`
            return Err(match err {
                FrameError::FrameTooLarge { len, max } => PoolError::Runtime(MontyException::new(
                    ExcType::RuntimeError,
                    Some(format!(
                        "request frame of {len} bytes exceeds the maximum of {max} bytes"
                    )),
                )),
                _ => self.poison("sending a request").await,
            });
        }
        loop {
            let event = match self.worker.as_mut().expect("checked above").recv().await {
                Ok(event) => event,
                Err(FrameError::Decode(err)) => {
                    return Err(self.protocol_violation(format!("invalid payload from worker: {err}")));
                }
                Err(_) => return Err(self.poison("waiting for a reply").await),
            };
            self.note_reported_time(&event);
            // strict alternation: zero or more `Print`s, then exactly one
            // turn-ender — so anything that is not a print ends the turn
            if matches!(event.kind, Some(pb::child_event::Kind::Print(_))) {
                on_event(&event).await;
            } else if matches!(event.kind, Some(pb::child_event::Kind::FatalError(_))) {
                // the far end is gone (see `fatal_error`): discard the worker
                // as the typed path does, but still hand back the child's own
                // account of its death for the driver to forward
                self.reap_announced_exit().await;
                return Ok(event);
            } else if matches!(event.kind, Some(pb::child_event::Kind::Shutdown(_))) {
                return if self.pool.config.transport.is_websocket() {
                    // the serving relay is gone: discard as `turn_io` does,
                    // handing the frame (and its dump) back to forward
                    self.discard_worker();
                    Ok(event)
                } else {
                    // Only a serving relay sends this, never a child — and a
                    // raw driver's relay signs dumps on the way past, so
                    // accepting one would have it vouch for bytes its child
                    // minted.
                    Err(self.protocol_violation("subprocess worker sent a ShutdownDump"))
                };
            } else if event.kind.is_none() {
                // every proto field is optional, so a hostile worker can send
                // an event with no kind set; it ends no turn and must not
                // reach the driver's client as one
                return Err(self.protocol_violation("worker sent an event with no kind"));
            } else {
                return Ok(event);
            }
        }
    }

    /// Fails fast when the checkout cannot start protocol work: the worker is
    /// gone (`Finished`), or a previous turn / mount servicing was cancelled
    /// mid-flight — the worker can no longer be trusted, so it is discarded
    /// here. Every public operation must hit this before its own validation,
    /// so cancellation surfaces as the contract's error, not a state check.
    fn ensure_ready(&mut self) -> Result<(), PoolError> {
        if self.worker.is_none() {
            Err(PoolError::Finished)
        } else if self.turn_in_flight {
            self.discard_worker();
            Err(PoolError::Protocol(
                "a previous protocol turn was cancelled mid-flight; the worker was discarded".into(),
            ))
        } else {
            Ok(())
        }
    }

    /// One request/reply exchange: send the request, stream prints, classify
    /// the turn-ending event. All failure paths discard the worker except
    /// `Runtime` / `Typing`, which are sandbox-level outcomes.
    async fn turn_io(&mut self, request: &pb::ParentRequest, on_print: OnPrint<'_>) -> Result<ControlEvent, PoolError> {
        let Some(worker) = self.worker.as_mut() else {
            return Err(PoolError::Finished);
        };
        if let Err(err) = worker.send(request).await {
            // An oversize frame is rejected *before* any bytes are written, so
            // the worker never saw the request and is still synced — surface a
            // clean, catchable error instead of discarding a healthy worker as
            // if it had crashed. For a `resume*` request this also leaves
            // `pending` set (nothing overwrites it on this path), so the
            // suspension stays answerable with a smaller value. Every other
            // send failure is a real I/O break (dead worker / closed pipe).
            return Err(match err {
                FrameError::FrameTooLarge { len, max } => PoolError::Runtime(MontyException::new(
                    ExcType::RuntimeError,
                    Some(format!(
                        "request frame of {len} bytes exceeds the maximum of {max} bytes"
                    )),
                )),
                _ => self.poison("sending a request").await,
            });
        }
        loop {
            let event = match self.worker.as_mut().expect("checked above").recv().await {
                Ok(event) => event,
                // a decode failure means the frame arrived intact but its
                // payload was garbage (including values that fail semantic
                // validation, which happens during decode) — the worker
                // misbehaved, it didn't die
                Err(FrameError::Decode(err)) => {
                    return Err(self.protocol_violation(format!("invalid payload from worker: {err}")));
                }
                Err(_) => return Err(self.poison("waiting for a reply").await),
            };
            // Print events carry no timing (the fields are zero), so this is
            // a no-op for them thanks to the monotonic-max ratchet.
            self.note_reported_time(&event);
            // Only a `Load` reply carries this; it lets `restore` report the
            // dump's script name without parsing the opaque dump bytes.
            if let Some(name) = &event.restored_script_name {
                self.restored_script_name = Some(name.clone());
            }
            match event.kind {
                Some(pb::child_event::Kind::Print(print)) => {
                    let stream = match print.stream() {
                        pb::PrintStream::Stderr => PrintStream::Stderr,
                        pb::PrintStream::Stdout | pb::PrintStream::Unspecified => PrintStream::Stdout,
                    };
                    on_print(stream, &print.text).await;
                }
                Some(pb::child_event::Kind::FunctionCall(call)) => {
                    self.pending = Some(Pending::Call {
                        call_id: call.call_id,
                        function_name: call.function_name.clone(),
                        os_call: None,
                    });
                    return self.convert_turn(|| {
                        Ok(TurnEvent::FunctionCall {
                            function_name: call.function_name,
                            args: call.args,
                            kwargs: call.kwargs,
                            call_id: call.call_id,
                            method_call: call.method_call,
                        })
                    });
                }
                Some(pb::child_event::Kind::OsCall(call)) => {
                    let call_id = call.call_id;
                    // Every announcement (fresh or re-announced after
                    // `restore`) decodes into a typed `OsFunctionCall`; a
                    // payload the child could never legitimately produce is a
                    // protocol violation.
                    let function_call = match call.call {
                        None => return Err(self.protocol_violation("OsCall event with no call")),
                        Some(kind) => match OsFunctionCall::try_from(kind) {
                            Ok(function_call) => function_call,
                            Err(err) => {
                                return Err(self.protocol_violation(format!("invalid OS call payload: {err}")));
                            }
                        },
                    };
                    // Every OS call surfaces, mount-covered or not: the caller
                    // decides how to answer it, and reaches this feed's mounts
                    // through `resume_from_mounts`. The typed call is retained
                    // for that; the caller-facing `(name, args, kwargs)` shape
                    // is projected from a clone.
                    let function_name = function_call.name().to_owned();
                    let (args, kwargs) = function_call.clone().to_args();
                    self.pending = Some(Pending::Call {
                        call_id,
                        function_name: function_name.clone(),
                        os_call: Some(Box::new(function_call)),
                    });
                    return Ok(ControlEvent::Turn(TurnEvent::OsCall {
                        function_name,
                        args,
                        kwargs,
                        call_id,
                    }));
                }
                Some(pb::child_event::Kind::NameLookup(lookup)) => {
                    self.pending = Some(Pending::NameLookup);
                    return Ok(ControlEvent::Turn(TurnEvent::NameLookup { name: lookup.name }));
                }
                Some(pb::child_event::Kind::ResolveFutures(futures)) => {
                    self.pending = Some(Pending::Futures);
                    return Ok(ControlEvent::Turn(TurnEvent::ResolveFutures {
                        pending_call_ids: futures.pending_call_ids,
                    }));
                }
                Some(pb::child_event::Kind::Complete(complete)) => {
                    self.pending = None;
                    // the feed is over — drop its mounts so overlay writes
                    // cannot leak into the next feed
                    self.feed_mounts = None;
                    return self.convert_turn(|| {
                        let value = complete
                            .value
                            .ok_or(monty_proto::ProtoConvertError::MissingField("Complete.value"))?;
                        Ok(TurnEvent::Complete(value.into_object()?))
                    });
                }
                Some(pb::child_event::Kind::Error(error)) => {
                    // an error reply to `Dump` (e.g. an oversize dump) does not
                    // end the in-flight feed — the child stays suspended and
                    // resumable, so keep the pending call and mounts
                    if !matches!(request.kind, Some(pb::parent_request::Kind::Dump(_))) {
                        self.pending = None;
                        self.feed_mounts = None;
                    }
                    let Some(exception) = error.exception else {
                        return Err(self.protocol_violation("error event with no exception"));
                    };
                    return match MontyException::try_from(exception) {
                        Ok(exc) => Err(PoolError::Runtime(exc)),
                        Err(err) => Err(self.protocol_violation(format!("invalid exception payload: {err}"))),
                    };
                }
                Some(pb::child_event::Kind::TypingError(typing)) => {
                    self.pending = None;
                    self.feed_mounts = None;
                    return Err(PoolError::Typing(typing.diagnostics));
                }
                Some(pb::child_event::Kind::Ok(_)) => return Ok(ControlEvent::Ok),
                Some(pb::child_event::Kind::DumpResult(dump)) => return Ok(ControlEvent::Dump(dump.state)),
                Some(pb::child_event::Kind::FatalError(fatal)) => {
                    return Err(self.fatal_error(&fatal.message).await);
                }
                Some(pb::child_event::Kind::Shutdown(shutdown)) => {
                    // Only a serving relay sends this, never a child — so a
                    // local subprocess claiming to shut down is a protocol
                    // violation, not a shutdown. Accepting it would let the
                    // child hand back a dump it minted itself, which the
                    // caller would then restore as trusted session state.
                    if !self.pool.config.transport.is_websocket() {
                        return Err(self.protocol_violation("subprocess worker sent a ShutdownDump"));
                    }
                    // The serving side is shutting down and dropped this
                    // request unexecuted; its worker is gone.
                    //
                    // NOTE(sign-dump): `dump` is server-minted and travels
                    // back through this client into `Checkout::restore`; if
                    // host-side dump signing lands it must pass the same
                    // verification there as any other dump.
                    self.discard_worker();
                    return Err(PoolError::Shutdown { dump: shutdown.dump });
                }
                None => {
                    return Err(self.protocol_violation("unexpected event"));
                }
            }
        }
    }

    /// Runs a fallible payload conversion; conversion failures mean the
    /// worker sent garbage, which discards it.
    fn convert_turn(
        &mut self,
        convert: impl FnOnce() -> Result<TurnEvent, monty_proto::ProtoConvertError>,
    ) -> Result<ControlEvent, PoolError> {
        match convert() {
            Ok(event) => Ok(ControlEvent::Turn(event)),
            Err(err) => Err(self.protocol_violation(format!("invalid payload from worker: {err}"))),
        }
    }

    /// Builds this feed's mount table, or `None` for the common mount-less
    /// feed. Runs inline: the specs' directories were opened when the caller
    /// built them, so nothing here touches the host filesystem.
    fn build_feed_mounts(mounts: Vec<MountSpec>) -> Option<MountTable> {
        (!mounts.is_empty()).then(|| build_mount_table(mounts))
    }

    /// Runs blocking host mount work on tokio's blocking pool, so a stalled
    /// filesystem (NFS, FUSE) occupies a blocking thread instead of a runtime
    /// worker shared with other sessions' turns and timers. A blocking-task
    /// failure (panic / runtime shutdown) discards the worker — callers'
    /// error contracts promise that any non-`Runtime`/`Typing` error means
    /// exactly that.
    async fn run_blocking<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PoolError> {
        match spawn_blocking(work).await {
            Ok(value) => Ok(value),
            Err(err) => Err(self.protocol_violation(format!("blocking mount task failed: {err}"))),
        }
    }

    /// Discards the worker after it violated the protocol on an intact stream
    /// (unexpected event kind, undecodable payload). Unlike [`Self::poison`]
    /// this is not a crash — the worker answered, just wrongly — so it maps
    /// to [`PoolError::Protocol`] rather than `Crashed`/`Timeout`.
    fn protocol_violation(&mut self, context: impl Into<Cow<'static, str>>) -> PoolError {
        self.discard_worker();
        PoolError::Protocol(context.into())
    }

    /// Discards the worker after it announced a `FatalError`.
    ///
    /// The frame arrives on an intact stream, but it means the far end is
    /// *gone*: a child writes it and exits immediately (version skew, frame
    /// desync, panic), and a serving relay uses it to report that it could not
    /// start a worker at all. That is a crash with a reason attached rather
    /// than a protocol disagreement, so it is reported as one — a caller that
    /// handles crashes by starting a new session needs no extra arm, and the
    /// worker's own account lands in [`CrashCause::Announced`].
    async fn fatal_error(&mut self, message: &str) -> PoolError {
        let status = self.reap_announced_exit().await;
        self.pending = None;
        self.feed_mounts = None;
        PoolError::Crashed {
            status,
            cause: CrashCause::Announced {
                reason: message.to_owned(),
            },
        }
    }

    /// Takes and reaps the worker behind a `FatalError` frame, returning its
    /// exit status when one was observed. Shared by [`Self::fatal_error`] and
    /// the raw path, which discards the worker but returns the frame itself.
    async fn reap_announced_exit(&mut self) -> Option<ExitStatus> {
        match self.worker.take() {
            // the child exits right after the frame, so give it a moment to do
            // so before killing: that is what surfaces e.g. the non-zero status
            // of a version-skew exit, which a SIGKILL would replace with the
            // signal and lose
            Some(mut worker) => {
                // guard, not a trailing release: a caller dropping this future
                // mid-reap must still release the slot
                let _capacity = CapacityGuard::new(&self.pool);
                let status = worker.reap_or_kill(FATAL_EXIT_GRACE).await;
                drop(worker);
                status
            }
            None => None,
        }
    }

    /// Discards the worker after an I/O failure and classifies it as an
    /// out-of-memory kill, a crash, or — on the WebSocket transport, where the
    /// worker is a remote process this client cannot reap — a disconnect.
    /// Deadline expiry goes through [`Self::poison_timeout`] instead.
    async fn poison(&mut self, context: &str) -> PoolError {
        let Some(mut worker) = self.worker.take() else {
            return PoolError::Finished;
        };
        self.pending = None;
        self.feed_mounts = None;
        // guard, not a trailing release: a caller dropping this future
        // mid-reap must still release the slot
        let _capacity = CapacityGuard::new(&self.pool);
        // A worker that exits deliberately (an allocation refused, see
        // `OOM_EXIT_CODE`) is racing us: SIGKILLing it mid-exit would replace
        // its code with `signal: 9` and lose the classification. Give it the
        // same grace `fatal_error` does — a dead child reaps on the first poll,
        // and only a wedged-alive one pays for it, on an already-failed turn.
        // A deadline expiry never lands here (see `poison_timeout`), so nothing
        // is waiting on this grace that should have been killed outright.
        let status = worker.reap_or_kill(FATAL_EXIT_GRACE).await;
        drop(worker);
        if self.pool.config.transport.is_websocket() {
            PoolError::Disconnected {
                context: context.to_owned(),
            }
        } else if status.and_then(|status| status.code()) == Some(monty_types::OOM_EXIT_CODE) {
            // the worker is gone, unlike every other `Runtime` error — the
            // checkout is already finished, so later calls report `Finished`
            PoolError::Runtime(MontyException::new(
                ExcType::MemoryError,
                Some("the worker exceeded its memory limit and was terminated".to_owned()),
            ))
        } else {
            PoolError::Crashed {
                status,
                cause: CrashCause::Vanished {
                    context: context.to_owned(),
                },
            }
        }
    }

    /// Discards the worker after the turn deadline expired: the turn's I/O
    /// future has already been dropped (safe — `Worker::recv` is cancel-safe),
    /// so this only kills, reaps, and classifies.
    async fn poison_timeout(&mut self) -> PoolError {
        if let Some(mut worker) = self.worker.take() {
            // guard, not a trailing release: a caller dropping this future
            // mid-reap must still release the slot
            let _capacity = CapacityGuard::new(&self.pool);
            let _ = worker.kill_and_reap().await;
            drop(worker);
        }
        self.pending = None;
        self.feed_mounts = None;
        PoolError::Timeout {
            timeout: self.armed_deadline.unwrap_or(Duration::ZERO),
        }
    }

    /// Discards the worker without crash classification, for turn-enders
    /// that are not crashes: protocol violations (the worker answered, just
    /// wrongly) and server `Shutdown` replies. Fatal errors instead go
    /// through [`Self::fatal_error`], which reaps and classifies.
    fn discard_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            drop(worker);
            self.pool.release_capacity();
        }
        self.pending = None;
        self.feed_mounts = None;
        self.turn_in_flight = false;
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        // a checkout abandoned mid-session cannot be trusted back into the
        // pool: kill the worker and free its capacity
        if let Some(worker) = self.worker.take() {
            drop(worker);
            self.pool.release_capacity();
        }
    }
}

/// Internal classification of a turn-ending event: real turn events for the
/// caller, plus the control acks (`Ok` / `DumpResult`) used by the checkout
/// lifecycle itself.
#[derive(Debug)]
enum ControlEvent {
    Turn(TurnEvent),
    Ok,
    Dump(Vec<u8>),
}

/// How long a child that announced a `FatalError` is given to exit on its own
/// before it is killed. It writes the frame and exits immediately, so this is
/// a scheduling allowance rather than a real wait — and it is only ever spent
/// on a turn that has already failed.
const FATAL_EXIT_GRACE: Duration = Duration::from_millis(100);

/// Wraps a request kind in a `ParentRequest`.
///
/// The one place the pool builds a request, so `ParentRequest`'s
/// message-level fields are decided here rather than at every callsite.
/// `trace_parent` is left unset: the pool does not propagate a tracing
/// context yet, and an absent one just means the child starts its own trace.
pub(crate) fn request(kind: pb::parent_request::Kind) -> pb::ParentRequest {
    pb::ParentRequest {
        kind: Some(kind),
        trace_parent: None,
    }
}

/// Rejects values too deeply nested for the wire (see
/// `monty_proto::MAX_VALUE_DEPTH`) with a session-preserving runtime error —
/// sending them would produce a frame the worker cannot decode.
fn ensure_sendable<'a>(values: impl IntoIterator<Item = &'a MontyObject>) -> Result<(), PoolError> {
    if values.into_iter().any(exceeds_max_value_depth) {
        Err(PoolError::Runtime(MontyException::new(
            ExcType::RuntimeError,
            Some("Max input depth exceeded".to_owned()),
        )))
    } else {
        Ok(())
    }
}

/// Converts a shared requirement-validation failure into a session-preserving
/// Python `ValueError`.
fn invalid_requirement(message: String) -> PoolError {
    PoolError::Runtime(MontyException::new(ExcType::ValueError, Some(message)))
}

/// The tighter of two optional deadlines (`None` means no deadline).
fn min_deadline(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (deadline, None) | (None, deadline) => deadline,
    }
}

/// Builds the parent-side [`MountTable`] for one feed from its (non-empty)
/// specs. Infallible and free of filesystem I/O: each spec already carries its
/// opened directory, so this only pairs those descriptors with a per-feed mode.
fn build_mount_table(mounts: Vec<MountSpec>) -> MountTable {
    let mut table = MountTable::new();
    for mount in mounts {
        let mode = match mount.mode {
            MountSpecMode::ReadOnly => MountMode::ReadOnly,
            MountSpecMode::ReadWrite => MountMode::ReadWrite,
            // Overlay state is created fresh per feed: writes live only as
            // long as the feed and are discarded with it.
            MountSpecMode::Overlay => MountMode::OverlayMemory(OverlayState::new()),
        };
        // No filesystem access: the root was opened when the spec was built.
        let mount = monty_fs::Mount::with_root(mount.root, mode, mount.write_bytes_limit)
            .with_memory_usage_limit(mount.memory_usage_limit);
        table.push_mount(mount);
    }
    table
}
