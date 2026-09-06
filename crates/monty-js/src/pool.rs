//! napi binding over `monty-pool`: crash-isolated execution in pools of
//! `monty subprocess` workers, the Node.js counterpart of `pydantic_monty`.
//!
//! The architecture mirrors the Python binding but keeps JS-shaped concerns in
//! TypeScript: this module exposes *turn-level* primitives ([`NativePool`],
//! [`NativeSession`] with `feed`/`resume*` methods that each run one protocol
//! turn), while the drive loop — dispatching external function calls, OS
//! callbacks and async futures between turns — lives in `ts/session.ts` where
//! promises are native. Every turn resolves to a plain JS "turn object"
//! (`{ kind: 'complete' | 'functionCall' | ... }`); pool-level failures
//! (crash, timeout, protocol desync) resolve as turn objects too, so the
//! TypeScript layer owns the public error classes.
//!
//! Threading model: `monty-pool` is async, so every protocol turn is a future
//! spawned on napi's tokio runtime via [`Env::spawn_future_with_callback`] —
//! never on the JS event loop thread. Sandbox `print()` output streams
//! mid-turn through a threadsafe function; the turn awaits each JS callback
//! (`call_async`), preserving print ordering and backpressure. The checkout
//! mutex (a tokio mutex) is held for a whole turn: event-loop-thread methods
//! must never block on it (`worker_pid` uses `try_lock`, everything else
//! locks inside the spawned future).

use std::{
    future::Future,
    pin::Pin,
    result::Result as StdResult,
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use monty_pool::{
    exceeds_max_value_depth,
    telemetry::{TelemetryAdapterHandle, TelemetryContext},
    Checkout, CheckoutOptions, MountSpec, MountSpecMode, OnPrint, Pool, PoolConfig, PoolError, PrintFuture, ReplConfig,
    ResumeValue, TurnEvent,
};
use monty_types::{
    AssertMessageAnnotations, ExcType, MontyException, MontyObject, NameLookupResult, PrintStream, StackFrame,
    TypeCheckingConfig, TypeCheckingFormat,
};
use napi::{
    bindgen_prelude::{
        Array, Buffer, ClassInstance, FnArgs, FromNapiValue, Function, JsObjectValue, Object, PromiseRaw, Unknown,
    },
    threadsafe_function::UnknownReturnValue,
    Env, Error, Result,
};
use napi_derive::napi;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    convert::{js_to_monty, monty_to_js},
    limits::{extract_limits, JsResourceLimits},
    telemetry::{configured_adapter, configured_tracing_adapter},
};

/// Deepest *list-like* value nesting the wire protocol accepts (dicts and
/// dataclasses cost more recursion budget per level, so nest less deeply).
#[napi]
#[expect(clippy::cast_possible_truncation, reason = "MAX_VALUE_DEPTH is 48")]
pub const MAX_VALUE_DEPTH: u32 = monty_pool::MAX_VALUE_DEPTH as u32;

/// The live pool, shared between the pool object and its sessions. `None`
/// until `start()` and again after `close()`. A std mutex: only ever held to
/// clone/take the `Arc`, never across awaits.
type SharedPool = Arc<Mutex<Option<Arc<Pool>>>>;
/// One session's worker handle. `None` before `enter()`, after `finish()`,
/// and after the worker is discarded on a crash. A *tokio* mutex: it is held
/// for a whole protocol turn, across that turn's awaits, serializing turns.
type SharedCheckout = Arc<AsyncMutex<Option<Checkout>>>;
/// The per-turn JS print callback, reached from the turn future through a
/// threadsafe function.
type PrintCallback<'env> = Function<'env, FnArgs<(String, String)>, UnknownReturnValue>;

/// The boxed future a turn closure returns: one computation borrowing the
/// locked checkout and the per-turn print callback.
type OutcomeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Identity helper pinning the HRTB closure shape [`NativeSession::run_outcome`]
/// accepts — without it, closure lifetime inference fails at every callsite.
fn outcome_fn<T, F>(f: F) -> F
where
    F: for<'a> FnOnce(&'a mut Checkout, OnPrint<'a>) -> OutcomeFuture<'a, T> + Send,
{
    f
}

/// Pool construction options. Timeouts are pre-normalised to milliseconds by
/// the TypeScript layer (which also applies the `durationLimitGrace` default
/// and resolves the binary path).
#[napi(object, js_name = "NativePoolOptions")]
pub struct NativePoolOptions {
    /// Resolved path to the `monty` binary.
    pub binary_path: String,
    /// Workers spawned eagerly by `start()` and kept warm.
    pub min_processes: u32,
    /// Hard cap on live workers; checkouts beyond it wait.
    pub max_processes: u32,
    /// How long `enter()` waits for a free worker (ms). Absent: forever.
    pub checkout_timeout_ms: Option<f64>,
    /// Parent-side hard deadline per protocol turn (ms).
    pub request_timeout_ms: Option<f64>,
    /// Grace for the automatic `maxDurationSecs` backstop (ms). Absent:
    /// backstop disabled.
    pub duration_limit_grace_ms: Option<f64>,
    /// Recycle a worker after serving this many checkouts.
    pub max_checkouts_per_worker: Option<u32>,
}

/// Session options for `checkout()`.
#[napi(object, js_name = "NativeCheckoutOptions")]
pub struct NativeCheckoutOptions {
    /// Script name used in tracebacks and type-check diagnostics.
    pub script_name: String,
    /// Sandbox resource limits enforced inside the worker.
    pub limits: Option<JsResourceLimits>,
    /// Type-check each fed snippet before executing it.
    pub type_check: bool,
    /// Stub declarations made available to type checking.
    pub type_check_stubs: Option<String>,
    /// How typing diagnostics are rendered, e.g. `'full'` or `'concise'`.
    /// Chosen here rather than on the thrown error because the checker's
    /// structured diagnostics never leave the worker.
    pub type_check_format: Option<String>,
    /// Render typing diagnostics with ANSI colour escapes.
    pub type_check_color: Option<bool>,

    /// Give failed `assert` statements pytest-style introspected messages
    /// (see limitations/assert.md), wire-encoded: absent = on with the
    /// default 120-byte operand-repr truncation, `0` = off, `n` = truncate
    /// operand reprs to `n` bytes. The TypeScript wrapper normalizes the
    /// public `boolean | number` option into this encoding.
    pub assert_message_annotations: Option<u32>,

    /// How long the worker may hold buffered `print()` output before sending
    /// it (ms). Absent: the worker's default. `0` restores line buffering,
    /// delivering each completed line on its own.
    pub print_flush_interval_ms: Option<f64>,
}

/// Per-feed settings other than the mounts, passed by the TypeScript
/// `MontySession` from its feed options.
#[napi(object, js_name = "NativeFeedOptions")]
pub struct NativeFeedOptions {
    /// Absolute virtual working directory; unset takes the first mount, else `/`.
    pub cwd: Option<String>,
    /// Skip type checking for this feed even when the session enables it.
    pub skip_type_check: bool,
}

/// One mount entry for a feed, pre-validated by the TypeScript `MountDir`.
#[napi(object, js_name = "NativeMount")]
pub struct NativeMount {
    /// Absolute virtual POSIX path inside the sandbox, e.g. `/mnt/data`.
    pub virtual_path: String,
    /// Host directory to expose.
    pub host_path: String,
    /// `'read-only'`, `'read-write'` or `'overlay'`.
    pub mode: String,
    /// Cap on total bytes written through this mount.
    pub write_bytes_limit: Option<f64>,
    /// Aggregate budget for retained overlay data and transient results.
    pub memory_usage_limit: f64,
}

/// A mount whose host directory is opened here and held until this object is
/// dropped, so every feed using it mounts that same directory. Wrapped by the
/// TypeScript `MountDir` — not part of the public API.
#[napi(js_name = "NativeMountDir")]
pub struct NativeMountDir {
    /// `None` once closed: the open directory is released and no later feed
    /// can mount it. A feed already running holds its own reference.
    spec: Option<MountSpec>,
}

#[napi]
impl NativeMountDir {
    /// Opens the host directory, throwing if it is missing, is not a directory,
    /// or the virtual path is not absolute.
    #[napi(constructor)]
    pub fn new(mount: NativeMount) -> Result<Self> {
        Ok(Self {
            spec: Some(MountSpec::try_from(mount)?),
        })
    }

    /// Releases the open directory. Idempotent.
    #[napi]
    pub fn close(&mut self) {
        self.spec = None;
    }
}

/// A pool of `monty` worker subprocesses. Wrapped by the TypeScript `Monty`
/// class — not part of the public API.
#[napi(js_name = "NativePool")]
pub struct NativePool {
    config: PoolConfig,
    pool: SharedPool,
}

#[napi]
impl NativePool {
    /// Validates and stores the configuration; workers are spawned by
    /// [`start`](Self::start).
    #[napi(constructor)]
    pub fn new(options: NativePoolOptions) -> Result<Self> {
        let mut config = PoolConfig::subprocess(&options.binary_path);
        config.min_processes = options.min_processes as usize;
        config.max_processes = options.max_processes as usize;
        config.checkout_timeout = options
            .checkout_timeout_ms
            .map(|ms| duration_from_ms("checkoutTimeout", ms))
            .transpose()?;
        config.request_timeout = options
            .request_timeout_ms
            .map(|ms| duration_from_ms("requestTimeout", ms))
            .transpose()?;
        config.duration_limit_grace = options
            .duration_limit_grace_ms
            .map(|ms| duration_from_ms("durationLimitGrace", ms))
            .transpose()?;
        config.max_checkouts_per_worker = options.max_checkouts_per_worker;
        config.metrics = configured_adapter().map(TelemetryAdapterHandle::metrics);
        if config.max_processes < 1 {
            return Err(invalid("maxProcesses must be at least 1"));
        }
        if config.min_processes > config.max_processes {
            return Err(invalid("minProcesses cannot exceed maxProcesses"));
        }
        Ok(Self {
            config,
            pool: Arc::new(Mutex::new(None)),
        })
    }

    /// Spawns the prewarmed workers off the event loop.
    #[napi]
    pub fn start<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let config = self.config.clone();
        let slot = Arc::clone(&self.pool);
        env.spawn_future(async move {
            let pool = Pool::new(config).await.map_err(pool_error)?;
            *lock(&slot) = Some(Arc::new(pool));
            Ok(())
        })
    }

    /// Prepares a session; its worker is checked out by `NativeSession.enter`.
    #[napi]
    pub fn checkout(&self, options: NativeCheckoutOptions) -> Result<NativeSession> {
        let limits = options.limits.map(extract_limits).transpose()?;
        Ok(NativeSession {
            pool: Arc::clone(&self.pool),
            repl_config: ReplConfig {
                script_name: options.script_name,
                limits,
                type_check: options.type_check,
                type_check_stubs: options.type_check_stubs,
                type_check_config: TypeCheckingConfig {
                    format: match options.type_check_format {
                        Some(name) => TypeCheckingFormat::from_name(&name).map_err(Error::from_reason)?,
                        None => TypeCheckingFormat::default(),
                    },
                    color: options.type_check_color.unwrap_or(false),
                },
                assert_message_annotations: options.assert_message_annotations.map_or_else(
                    AssertMessageAnnotations::default,
                    AssertMessageAnnotations::from_max_bytes,
                ),
                print_flush_interval: options
                    .print_flush_interval_ms
                    .map(|ms| duration_from_ms("printFlushInterval", ms))
                    .transpose()?,
            },
            checkout: Arc::new(AsyncMutex::new(None)),
        })
    }

    /// Shuts the pool down: idle workers exit, capacity is gone. Sessions
    /// still checked out keep their workers until they finish.
    #[napi]
    pub fn close<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let slot = Arc::clone(&self.pool);
        env.spawn_future(async move {
            let pool = lock(&slot).take();
            if let Some(pool) = pool {
                pool.close().await;
                drop(pool);
            }
            Ok(())
        })
    }
}

/// Distributed context captured synchronously before native session entry.
#[napi(object, js_name = "NativeTelemetryContext")]
pub struct NativeTelemetryContext {
    /// W3C trace ID of the active host span.
    pub trace_id: Option<String>,
    /// W3C span ID of the active host span.
    pub span_id: Option<String>,
    /// W3C trace flags of the active host span.
    pub trace_flags: Option<u8>,
    /// Vendor trace state propagated with the active host span.
    pub trace_state: Option<String>,
}

impl NativeTelemetryContext {
    /// Converts valid distributed context while ignoring a malformed adapter value.
    fn parse(self, adapter: &TelemetryAdapterHandle) -> TelemetryContext {
        self.trace_id
            .zip(self.span_id)
            .and_then(|(trace_id, span_id)| {
                adapter
                    .context_with_remote(
                        &trace_id,
                        &span_id,
                        self.trace_flags.unwrap_or_default(),
                        self.trace_state.as_deref().unwrap_or_default(),
                        false,
                    )
                    .ok()
            })
            .unwrap_or_else(|| adapter.unparented_context())
    }
}

/// One worker process dedicated to one REPL session. Wrapped by the
/// TypeScript `MontySession` class — not part of the public API.
#[napi(js_name = "NativeSession")]
pub struct NativeSession {
    pool: SharedPool,
    repl_config: ReplConfig,
    checkout: SharedCheckout,
}

#[napi]
impl NativeSession {
    /// Checks a worker out of the pool (spawning one if allowed) and creates
    /// the REPL session in it. Rejects with the pool error message on
    /// exhaustion or spawn failure.
    #[napi]
    pub fn enter<'env>(
        &self,
        env: &'env Env,
        telemetry_context: Option<NativeTelemetryContext>,
    ) -> Result<PromiseRaw<'env, ()>> {
        let pool = Arc::clone(&self.pool);
        let repl_config = self.repl_config.clone();
        let slot = Arc::clone(&self.checkout);
        let telemetry_context =
            telemetry_context.and_then(|context| configured_tracing_adapter().map(|adapter| context.parse(adapter)));
        env.spawn_future(async move {
            let pool = lock(&pool)
                .as_ref()
                .map(Arc::clone)
                .ok_or_else(|| invalid("the pool is not started — create it with Monty.create()"))?;
            let checkout = pool
                .checkout_with(
                    &repl_config,
                    CheckoutOptions::default().with_telemetry(telemetry_context),
                )
                .await
                .map_err(pool_error)?;
            *slot.lock().await = Some(checkout);
            Ok(())
        })
    }

    /// Runs one feed turn: executes `code` until completion or the first
    /// suspension, streaming prints to `on_print`. Resolves to a turn object.
    #[napi]
    pub fn feed<'env>(
        &self,
        env: &'env Env,
        code: String,
        inputs: Option<Object<'env>>,
        mounts: Vec<ClassInstance<'env, NativeMountDir>>,
        options: NativeFeedOptions,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let inputs = convert_inputs(env, inputs)?;
        let mounts = mount_specs(&mounts)?;
        let NativeFeedOptions { cwd, skip_type_check } = options;
        self.run_turn(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| {
                Box::pin(async move {
                    checkout
                        .feed_with_cwd(code, inputs, mounts, cwd.as_deref(), skip_type_check, on_print)
                        .await
                })
            }),
        )
    }

    /// Answers a `functionCall`/`osCall` suspension with a return value. A
    /// value that cannot cross the wire becomes a catchable in-sandbox error
    /// instead — this method never fails for value reasons, because the
    /// worker is suspended awaiting exactly one resume.
    #[napi]
    pub fn resume_return<'env>(
        &self,
        env: &'env Env,
        value: Unknown<'env>,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let resume = sendable_resume(env, value);
        self.run_turn(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| Box::pin(checkout.resume(resume, on_print))),
        )
    }

    /// Answers a suspension with an exception (`excType` must be a Python
    /// exception type name monty knows; anything else becomes RuntimeError).
    #[napi]
    pub fn resume_error<'env>(
        &self,
        env: &'env Env,
        exc_type: String,
        message: String,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let exc = exception_from_parts(&exc_type, message);
        self.run_turn(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| Box::pin(checkout.resume(ResumeValue::Error(exc), on_print))),
        )
    }

    /// Answers an `osCall` suspension by declining it: the sandbox raises the
    /// call's own no-handler default.
    #[napi]
    pub fn resume_not_handled<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_turn(
            env,
            on_print,
            outcome_fn(|checkout, on_print| Box::pin(checkout.resume(ResumeValue::NotHandled, on_print))),
        )
    }

    /// Offers an `osCall` suspension to this feed's mounts. Resolves to the
    /// next turn when a mount serviced the call, or to `{kind: 'notMounted'}`
    /// when none covered it and the caller must answer it itself.
    #[napi]
    pub fn resume_from_mounts<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_outcome(
            env,
            on_print,
            outcome_fn(|checkout, on_print| {
                Box::pin(async move {
                    match checkout.resume_from_mounts(on_print).await {
                        Ok(Some(event)) => TurnOutcome::Event(event),
                        Ok(None) => TurnOutcome::NotMounted,
                        Err(err) => TurnOutcome::from(StdResult::<TurnEvent, PoolError>::Err(err)),
                    }
                })
            }),
        )
    }

    /// Answers a `functionCall` suspension whose name has no handler: the
    /// sandbox raises `NameError`.
    #[napi]
    pub fn resume_not_found<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_turn(
            env,
            on_print,
            outcome_fn(|checkout, on_print| Box::pin(checkout.resume(ResumeValue::NotFound, on_print))),
        )
    }

    /// Registers the pending call as an external future (the JS promise stays
    /// in TypeScript); other sandbox tasks keep executing.
    #[napi]
    pub fn resume_future<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_turn(
            env,
            on_print,
            outcome_fn(|checkout, on_print| Box::pin(checkout.resume(ResumeValue::Future, on_print))),
        )
    }

    /// Answers a `nameLookup` suspension against `externalLookup`. A callable
    /// entry resolves to a host function proxy, passed here as its display name
    /// (`function_name`); any other entry is passed inside the `value` wrapper
    /// (`{ value: ... }`) and converted to a wire value returned directly. The
    /// wrapper exists because napi maps a bare JS `null`/`undefined` argument to
    /// "absent" — without it, an entry whose value *is* `null` would be
    /// indistinguishable from an undefined name. With both arguments absent the
    /// name is undefined and the sandbox raises `NameError`. A `value` that
    /// cannot cross the wire rejects the turn (the worker has not yet observed
    /// the name).
    #[napi]
    pub fn resume_name_lookup<'env>(
        &self,
        env: &'env Env,
        function_name: Option<String>,
        value: Option<Object<'env>>,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let resolved = match value {
            Some(wrapper) => Some(name_lookup_value(env, &wrapper)?),
            None => function_name.map(|name| MontyObject::Function { name, docstring: None }),
        };
        self.run_turn(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| Box::pin(checkout.resume_name_lookup(resolved, on_print))),
        )
    }

    /// Answers a lazy attribute `nameLookup` (one carrying an `objectId`) with
    /// the host's value. Unlike `resume_name_lookup`, a value that cannot cross
    /// the wire is raised in the sandbox as `TypeError`, as `resume_return`
    /// does: the host has served the attribute, so the failure is sandbox
    /// code's to catch rather than a rejected turn.
    #[napi]
    pub fn resume_lazy_attr<'env>(
        &self,
        env: &'env Env,
        value: Unknown<'env>,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let resolved = match sendable_value(env, value) {
            Ok(value) => NameLookupResult::Value(value),
            Err(exc) => NameLookupResult::Error(exc),
        };
        self.run_turn(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| Box::pin(checkout.resume_name_lookup(resolved, on_print))),
        )
    }

    /// Answers a `nameLookup` suspension with an exception raised where the
    /// lookup suspended — how a host error while serving a lazy attribute
    /// reaches sandbox code as a catchable exception. Same `exc_type` mapping
    /// as `resume_error`.
    #[napi]
    pub fn resume_name_lookup_error<'env>(
        &self,
        env: &'env Env,
        exc_type: String,
        message: String,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let exc = exception_from_parts(&exc_type, message);
        self.run_turn(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| {
                Box::pin(checkout.resume_name_lookup(NameLookupResult::Error(exc), on_print))
            }),
        )
    }

    /// Answers a `resolveFutures` suspension with the settled promises'
    /// outcomes: an array of `{ callId, ok, value?, excType?, message? }`.
    #[napi]
    pub fn resolve_futures<'env>(
        &self,
        env: &'env Env,
        results: Vec<Object<'env>>,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let results = results
            .into_iter()
            .map(|result| {
                let call_id: u32 = require(&result, "callId")?;
                let ok: bool = require(&result, "ok")?;
                let value = if ok {
                    match result.get::<Unknown>("value")? {
                        Some(value) => sendable_resume(env, value),
                        None => ResumeValue::Return(MontyObject::None),
                    }
                } else {
                    let exc_type: String = require(&result, "excType")?;
                    let message: String = require(&result, "message")?;
                    ResumeValue::Error(exception_from_parts(&exc_type, message))
                };
                Ok((call_id, value))
            })
            .collect::<Result<Vec<_>>>()?;
        self.run_turn(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| Box::pin(checkout.resume_futures(results, on_print))),
        )
    }

    /// Restores a dump into this session's freshly configured worker. Resolves
    /// to a turn object: a suspension when the dump was mid-feed, or `loaded`
    /// for an idle dump. The TypeScript `loadSession` / `loadSnapshot` split
    /// inspects the kind and enforces "fresh session only".
    #[napi]
    pub fn restore<'env>(
        &self,
        env: &'env Env,
        state: Buffer,
        mounts: Vec<ClassInstance<'env, NativeMountDir>>,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let mounts = mount_specs(&mounts)?;
        let state = state.to_vec();
        self.run_outcome(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| {
                Box::pin(async move {
                    // JS snapshots expose no script name, so the restored name is unused
                    match checkout.restore(state, mounts, on_print).await {
                        Ok((Some(event), _)) => TurnOutcome::Event(event),
                        Ok((None, _)) => TurnOutcome::LoadedIdle,
                        Err(err) => TurnOutcome::from(Err(err)),
                    }
                })
            }),
        )
    }

    /// Serializes the worker's session state (idle or suspended) into opaque
    /// bytes via monty's dump format. The session stays usable.
    #[napi]
    pub fn dump<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, Buffer>> {
        let slot = Arc::clone(&self.checkout);
        env.spawn_future(async move {
            let mut guard = slot.lock().await;
            let checkout = guard.as_mut().ok_or_else(|| pool_error(PoolError::Finished))?;
            checkout.dump().await.map(Buffer::from).map_err(pool_error)
        })
    }

    /// Installs third-party Python packages into the session via the worker's
    /// `uv`, making them importable by later feeds. Session-scoped and
    /// repeatable. Resolves to a turn object: `{kind:'ok'}` on success, or an
    /// `error` / `crashed` / `protocol` outcome the TypeScript layer raises
    /// (a uv failure, or the `monty` sandbox worker rejecting the request,
    /// arrives as `error`). Streams no prints, but takes `on_print` to share the
    /// turn machinery; the callback is never invoked.
    #[napi]
    pub fn install_dependencies<'env>(
        &self,
        env: &'env Env,
        requirements: Vec<String>,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_outcome(
            env,
            on_print,
            outcome_fn(move |checkout, _on_print| {
                Box::pin(async move {
                    match checkout.install_dependencies(requirements).await {
                        Ok(()) => TurnOutcome::Ok,
                        Err(err) => TurnOutcome::from(Err::<TurnEvent, _>(err)),
                    }
                })
            }),
        )
    }

    /// Ends the session and returns the worker to the pool (best effort — a
    /// crashed worker has already been discarded and replaced).
    #[napi]
    pub fn finish<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let slot = Arc::clone(&self.checkout);
        env.spawn_future(async move {
            // take in its own statement so the lock is released before the
            // finish turn runs
            let checkout = slot.lock().await.take();
            if let Some(checkout) = checkout {
                // best effort: a worker that cannot reset is discarded by
                // monty-pool itself
                let _ = checkout.finish().await;
            }
            Ok(())
        })
    }

    /// OS process id of this session's worker, or `null` when no worker is
    /// attached or a turn is in flight (the turn thread holds the checkout
    /// lock — blocking the event loop on it would deadlock with the print
    /// callback, which needs the event loop).
    #[napi(getter)]
    pub fn worker_pid(&self) -> Option<u32> {
        self.checkout.try_lock().ok()?.as_ref().and_then(Checkout::pid)
    }
}

impl NativeSession {
    /// Runs one protocol turn as a spawned future and resolves it to a JS
    /// turn object. Pool-level failures (runtime error, typing error, crash,
    /// timeout, protocol desync) resolve as turn objects too — the
    /// TypeScript layer raises its public error classes from them; the
    /// promise only rejects for binding-level bugs.
    fn run_turn<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
        turn: impl for<'a> FnOnce(&'a mut Checkout, OnPrint<'a>) -> OutcomeFuture<'a, StdResult<TurnEvent, PoolError>>
            + Send
            + 'static,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_outcome(
            env,
            on_print,
            outcome_fn(move |checkout, on_print| {
                Box::pin(async move { TurnOutcome::from(turn(checkout, on_print).await) })
            }),
        )
    }

    /// The shared turn machinery behind [`run_turn`](Self::run_turn): locks the
    /// checkout inside the spawned future, streams prints, and resolves the
    /// computed [`TurnOutcome`] to a JS turn object. `compute` returns the
    /// outcome directly so the restore turn (which yields `Option<TurnEvent>`)
    /// can map its idle case to [`TurnOutcome::LoadedIdle`].
    fn run_outcome<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
        compute: impl for<'a> FnOnce(&'a mut Checkout, OnPrint<'a>) -> OutcomeFuture<'a, TurnOutcome> + Send + 'static,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let tsfn = Arc::new(on_print.build_threadsafe_function().build()?);
        let slot = Arc::clone(&self.checkout);
        env.spawn_future_with_callback(
            async move {
                let mut guard = slot.lock().await;
                let Some(checkout) = guard.as_mut() else {
                    return Ok(TurnOutcome::Protocol(
                        "the session is closed — check out a new one".to_owned(),
                    ));
                };
                // Forward each print to JS and *await the callback having
                // run* (not merely queued), preserving print ordering
                // relative to the turn's resolution and providing
                // backpressure against print floods. The TypeScript
                // wrapper captures callback failures itself and never
                // throws back across this boundary.
                let mut on_print = move |stream: PrintStream, text: &str| -> PrintFuture {
                    let stream = match stream {
                        PrintStream::Stdout => "stdout",
                        PrintStream::Stderr => "stderr",
                    };
                    let tsfn = Arc::clone(&tsfn);
                    let args = FnArgs::from((stream.to_owned(), text.to_owned()));
                    Box::pin(async move {
                        let _ = tsfn.call_async(args).await;
                    })
                };
                Ok(compute(checkout, &mut on_print).await)
            },
            turn_to_js,
        )
    }
}

/// One turn's result, computed off the event loop and converted to a JS
/// object once back on it.
enum TurnOutcome {
    /// The turn ended in a suspension or completion.
    Event(TurnEvent),
    /// The sandbox raised; worker and session stay usable.
    Runtime(MontyException),
    /// Type checking rejected the snippet; worker and session stay usable.
    Typing(String),
    /// The worker died (crash or watchdog kill); the session is lost.
    Crashed {
        message: String,
        timed_out: bool,
        exit_status: Option<String>,
    },
    /// The worker (or caller) violated the protocol; the session is lost.
    Protocol(String),
    /// A restore of an idle (between-feeds) dump — there is no suspension to
    /// resume. Only produced by [`NativeSession::restore`].
    LoadedIdle,
    /// No mount covered the pending OS call, which is still suspended for the
    /// caller to answer. Only produced by
    /// [`NativeSession::resume_from_mounts`].
    NotMounted,
    /// A non-feed request succeeded with no value or suspension. Produced by
    /// [`NativeSession::install_dependencies`].
    Ok,
}

impl From<StdResult<TurnEvent, PoolError>> for TurnOutcome {
    fn from(result: StdResult<TurnEvent, PoolError>) -> Self {
        match result {
            Ok(event) => Self::Event(event),
            Err(PoolError::Runtime(exc)) => Self::Runtime(exc),
            Err(PoolError::Typing(diagnostics)) => Self::Typing(diagnostics),
            Err(err @ PoolError::Timeout { .. }) => Self::Crashed {
                message: err.to_string(),
                timed_out: true,
                exit_status: None,
            },
            // `status` is `Copy`, so it can be reported *and* handed back to
            // the error, whose own Display picks between "while <doing X>"
            // and the worker's stated reason — don't re-derive that here.
            Err(PoolError::Crashed { status, cause }) => Self::Crashed {
                exit_status: status.map(|status| status.to_string()),
                message: PoolError::Crashed { status, cause }.to_string(),
                timed_out: false,
            },
            // WebSocket-only, so both are unreachable here (the napi binding
            // is subprocess-only). Mapped to their local analogue anyway
            // rather than falling through to a misleading protocol error: a
            // dropped connection and a server shutdown both mean the session
            // is gone, which is what `Crashed` tells the caller.
            Err(err @ (PoolError::Disconnected { .. } | PoolError::Shutdown { .. })) => Self::Crashed {
                message: err.to_string(),
                timed_out: false,
                exit_status: None,
            },
            Err(other) => Self::Protocol(other.to_string()),
        }
    }
}

/// Converts a turn outcome into the JS turn object consumed by
/// `ts/session.ts`. All keys are fixed strings; sandbox-controlled data only
/// ever appears in *values* (kwargs cross as `[key, value]` pairs so the
/// TypeScript layer can build a null-prototype record safely).
fn turn_to_js(env: &Env, outcome: TurnOutcome) -> Result<Object<'_>> {
    let mut obj = Object::new(env)?;
    match outcome {
        TurnOutcome::Event(TurnEvent::Complete(value)) => {
            obj.set("kind", "complete")?;
            obj.set("value", monty_to_js(&value, env)?)?;
        }
        TurnOutcome::Event(TurnEvent::FunctionCall {
            function_name,
            args,
            kwargs,
            call_id,
            object_id,
        }) => {
            obj.set("kind", "functionCall")?;
            obj.set("functionName", function_name)?;
            obj.set("args", values_to_js(env, &args)?)?;
            obj.set("kwargs", pairs_to_js(env, &kwargs)?)?;
            obj.set("callId", call_id)?;
            // the routed receiver uuid as a canonical string
            obj.set("objectId", object_id.map(|uuid| uuid.to_string()))?;
        }
        TurnOutcome::Event(TurnEvent::OsCall {
            function_name,
            args,
            kwargs,
            call_id,
        }) => {
            obj.set("kind", "osCall")?;
            obj.set("functionName", function_name)?;
            obj.set("args", values_to_js(env, &args)?)?;
            obj.set("kwargs", pairs_to_js(env, &kwargs)?)?;
            obj.set("callId", call_id)?;
        }
        TurnOutcome::Event(TurnEvent::NameLookup { name, object_id }) => {
            obj.set("kind", "nameLookup")?;
            obj.set("name", name)?;
            // the receiver uuid as a canonical string
            obj.set("objectId", object_id.map(|uuid| uuid.to_string()))?;
        }
        TurnOutcome::Event(TurnEvent::ResolveFutures { pending_call_ids }) => {
            obj.set("kind", "resolveFutures")?;
            obj.set("pendingCallIds", pending_call_ids)?;
        }
        TurnOutcome::Runtime(exc) => {
            obj.set("kind", "error")?;
            obj.set("exception", exception_to_js(env, &exc)?)?;
        }
        TurnOutcome::Typing(diagnostics) => {
            obj.set("kind", "typingError")?;
            obj.set("diagnostics", diagnostics)?;
        }
        TurnOutcome::Crashed {
            message,
            timed_out,
            exit_status,
        } => {
            obj.set("kind", "crashed")?;
            obj.set("message", message)?;
            obj.set("timedOut", timed_out)?;
            if let Some(status) = exit_status {
                obj.set("exitStatus", status)?;
            }
        }
        TurnOutcome::Protocol(message) => {
            obj.set("kind", "protocol")?;
            obj.set("message", message)?;
        }
        TurnOutcome::LoadedIdle => {
            obj.set("kind", "loaded")?;
        }
        TurnOutcome::NotMounted => {
            obj.set("kind", "notMounted")?;
        }
        TurnOutcome::Ok => {
            obj.set("kind", "ok")?;
        }
    }
    Ok(obj)
}

/// Converts positional call arguments for a turn object.
fn values_to_js<'env>(env: &'env Env, values: &[MontyObject]) -> Result<Array<'env>> {
    let mut array = env.create_array(u32::try_from(values.len()).map_err(|_| invalid("too many arguments"))?)?;
    for (i, value) in (0u32..).zip(values.iter()) {
        array.set(i, monty_to_js(value, env)?)?;
    }
    Ok(array)
}

/// Converts kwargs as an array of `[key, value]` pairs. Keys cross as plain
/// values — never as JS object property names — so a sandbox-chosen key like
/// `__proto__` cannot touch any prototype here.
fn pairs_to_js<'env>(env: &'env Env, pairs: &[(MontyObject, MontyObject)]) -> Result<Array<'env>> {
    let mut array = env.create_array(u32::try_from(pairs.len()).map_err(|_| invalid("too many kwargs"))?)?;
    for (i, (key, value)) in (0u32..).zip(pairs.iter()) {
        let mut pair = env.create_array(2)?;
        pair.set(0, monty_to_js(key, env)?)?;
        pair.set(1, monty_to_js(value, env)?)?;
        array.set(i, pair)?;
    }
    Ok(array)
}

/// Converts an exception for a turn object: type name, message, the
/// fully-rendered Python traceback (monty's `MontyException` Display — the
/// single source of truth, so `ts/errors.ts` never re-implements it), and the
/// structured frames for programmatic access via `MontyRuntimeError.traceback()`.
fn exception_to_js<'env>(env: &'env Env, exc: &MontyException) -> Result<Object<'env>> {
    let mut obj = Object::new(env)?;
    obj.set("excType", exc.exc_type().to_string())?;
    obj.set("message", exc.message().unwrap_or(""))?;
    obj.set("traceback", exc.to_string())?;
    let frames = exc.traceback();
    let mut array = env.create_array(u32::try_from(frames.len()).map_err(|_| invalid("traceback too deep"))?)?;
    for (i, frame) in (0u32..).zip(frames.iter()) {
        array.set(i, frame_to_js(env, frame)?)?;
    }
    obj.set("frames", array)?;
    Ok(obj)
}

/// Converts one stack frame, field-for-field what `renderTraceback` needs.
fn frame_to_js<'env>(env: &'env Env, frame: &StackFrame) -> Result<Object<'env>> {
    let mut obj = Object::new(env)?;
    obj.set("filename", frame.filename.as_str())?;
    obj.set("line", frame.start.line)?;
    obj.set("column", frame.start.column)?;
    obj.set("endLine", frame.end.line)?;
    obj.set("endColumn", frame.end.column)?;
    if let Some(name) = &frame.frame_name {
        obj.set("frameName", name.as_str())?;
    }
    if let Some(preview) = &frame.preview_line {
        obj.set("previewLine", preview.as_ref())?;
    }
    obj.set("hideCaret", frame.hide_caret)?;
    obj.set("hideFrameName", frame.hide_frame_name)?;
    Ok(obj)
}

/// Converts the `inputs` record into named wire values, rejecting values the
/// wire cannot carry (the feed has not started, so failing here is safe).
fn convert_inputs(env: &Env, inputs: Option<Object<'_>>) -> Result<Vec<(String, MontyObject)>> {
    let Some(inputs) = inputs else {
        return Ok(vec![]);
    };
    Object::keys(&inputs)?
        .into_iter()
        .map(|name| {
            let value = match inputs.get::<Unknown>(&name)? {
                Some(value) => js_to_monty(value, *env)?,
                None => MontyObject::None,
            };
            if exceeds_max_value_depth(&value) {
                Err(invalid("Max input depth exceeded"))
            } else {
                Ok((name, value))
            }
        })
        .collect()
}

/// Converts a non-callable `externalLookup` entry — carried inside a
/// `{ value: ... }` wrapper so JS `null`/`undefined` survive napi's
/// null-means-absent argument mapping — into a wire value for a name lookup,
/// rejecting values the wire cannot carry. Unlike a `resume_return` value
/// (which becomes a catchable in-sandbox error), the worker has not yet
/// observed the name, so a bad value fails the turn cleanly — matching the
/// Python resolver, which surfaces a conversion error rather than `NameError`.
fn name_lookup_value(env: &Env, wrapper: &Object<'_>) -> Result<MontyObject> {
    // `get_named_property` (not `get`) so an inner `undefined` still converts
    // (to `None`) instead of collapsing back to Option::None.
    let value: Unknown = wrapper.get_named_property("value")?;
    let obj = js_to_monty(value, *env)?;
    if exceeds_max_value_depth(&obj) {
        Err(invalid("Max input depth exceeded"))
    } else {
        Ok(obj)
    }
}

/// Reads a required field from a JS object argument.
fn require<T: FromNapiValue>(obj: &Object<'_>, field: &str) -> Result<T> {
    obj.get::<T>(field)?
        .ok_or_else(|| invalid(&format!("missing required field {field}")))
}

/// Converts an external call's return value into a resume. Values that
/// cannot cross the wire — unconvertible or too deeply nested — become a
/// catchable in-sandbox error instead: the worker is suspended awaiting
/// exactly one resume, so this must never fail.
fn sendable_resume(env: &Env, value: Unknown<'_>) -> ResumeValue {
    match sendable_value(env, value) {
        Ok(value) => ResumeValue::Return(value),
        Err(exc) => ResumeValue::Error(exc),
    }
}

/// Converts a host value the sandbox has already asked for, mapping one the
/// wire cannot carry to the exception raised in its place: `TypeError` for an
/// unconvertible value, `RuntimeError` for excessive nesting.
fn sendable_value(env: &Env, value: Unknown<'_>) -> StdResult<MontyObject, MontyException> {
    match js_to_monty(value, *env) {
        Ok(value) if exceeds_max_value_depth(&value) => Err(MontyException::new(
            ExcType::RuntimeError,
            Some("Max input depth exceeded".to_owned()),
        )),
        Ok(value) => Ok(value),
        Err(err) => Err(MontyException::new(ExcType::TypeError, Some(err.reason.clone()))),
    }
}

/// Builds a `MontyException` from the TypeScript error mapping (which only
/// passes Python exception type names; anything unknown is a RuntimeError).
fn exception_from_parts(exc_type: &str, message: String) -> MontyException {
    let exc_type = ExcType::from_str(exc_type).unwrap_or(ExcType::RuntimeError);
    MontyException::new(exc_type, Some(message))
}

impl TryFrom<NativeMount> for MountSpec {
    type Error = napi::Error;

    fn try_from(mount: NativeMount) -> Result<Self> {
        let mode = match mount.mode.as_str() {
            "read-only" => MountSpecMode::ReadOnly,
            "read-write" => MountSpecMode::ReadWrite,
            "overlay" => MountSpecMode::Overlay,
            other => return Err(invalid(&format!("invalid mount mode: '{other}'"))),
        };
        let write_bytes_limit = mount
            .write_bytes_limit
            .map(|limit| bytes_limit(limit, "writeBytesLimit"))
            .transpose()?;
        let memory_usage_limit = bytes_limit(mount.memory_usage_limit, "memoryUsageLimit")?;
        // Opens the directory: the spec carries a descriptor from here on, so
        // the host path is resolved once per mount object rather than per feed.
        let mut spec = Self::new(&mount.virtual_path, &mount.host_path, mode).map_err(pool_error)?;
        spec.write_bytes_limit = write_bytes_limit;
        spec.memory_usage_limit = memory_usage_limit;
        Ok(spec)
    }
}

/// Validates and narrows a JavaScript byte limit. Values at or above `2^64`
/// are rejected rather than saturated — a saturating cast would silently turn
/// an out-of-range limit into `u64::MAX`, effectively disabling the budget.
fn bytes_limit(limit: f64, name: &str) -> Result<u64> {
    if (0.0..18_446_744_073_709_551_616.0).contains(&limit) && limit.fract() == 0.0 {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(limit as u64)
    } else {
        Err(invalid(&format!("{name} must be a non-negative integer below 2**64")))
    }
}

/// Converts a millisecond count from JS into a `Duration`, naming the option
/// in the error so a rejected value says which one it was.
fn duration_from_ms(name: &str, ms: f64) -> Result<Duration> {
    Duration::try_from_secs_f64(ms / 1000.0).map_err(|err| invalid(&format!("invalid {name}: {err}")))
}

/// Locks the shared *pool* slot, ignoring poisoning (a panic elsewhere must
/// not wedge the pool). Only ever held to clone/take the `Arc<Pool>` — the
/// per-session checkout slot is a tokio mutex the turn future locks with
/// `.lock().await` (or `try_lock` on the event-loop thread).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clones each mount's opened configuration for one turn — the descriptor is
/// shared, so nothing is reopened and no host path is resolved again.
///
/// # Errors
///
/// Rejects the turn if any mount has been closed.
fn mount_specs(mounts: &[ClassInstance<'_, NativeMountDir>]) -> Result<Vec<MountSpec>> {
    mounts
        .iter()
        .map(|mount| {
            mount
                .spec
                .clone()
                .ok_or_else(|| invalid("mount is closed: create a new MountDir"))
        })
        .collect()
}

fn pool_error(err: PoolError) -> napi::Error {
    napi::Error::from_reason(err.to_string())
}

fn invalid(message: &str) -> napi::Error {
    napi::Error::new(napi::Status::InvalidArg, message.to_owned())
}
