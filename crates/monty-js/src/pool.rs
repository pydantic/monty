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
//! Threading model: protocol turns block on subprocess I/O, so they run on
//! tokio's blocking pool via [`Env::spawn_future_with_callback`] — never on
//! the JS event loop. Sandbox `print()` output streams mid-turn through a
//! threadsafe function; the turn thread blocks until the JS callback has run
//! (`call_async`), preserving print ordering and backpressure exactly like
//! the in-process bindings. The checkout mutex is therefore held for a whole
//! turn: event-loop-thread methods must never block on it (`worker_pid` uses
//! `try_lock`, everything else locks inside the spawned future).

use std::{
    fmt,
    result::Result as StdResult,
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError},
    time::Duration,
};

use monty::{ExcType, MontyException, MontyObject, PrintStream, StackFrame};
use monty_pool::{
    exceeds_max_value_depth, Checkout, MountSpec, MountSpecMode, OnPrint, Pool, PoolConfig, PoolError, ReplConfig,
    ResumeValue, TurnEvent,
};
use napi::{
    bindgen_prelude::{
        block_on, spawn_blocking, Array, Buffer, FnArgs, FromNapiValue, Function, JsObjectValue, Object, PromiseRaw,
        Unknown,
    },
    threadsafe_function::UnknownReturnValue,
    Env, Result,
};
use napi_derive::napi;

use crate::{
    convert::{js_to_monty, monty_to_js},
    limits::{extract_limits, JsResourceLimits},
};

/// Deepest *list-like* value nesting the wire protocol accepts (dicts and
/// dataclasses cost more recursion budget per level, so nest less deeply).
#[napi]
#[expect(clippy::cast_possible_truncation, reason = "MAX_VALUE_DEPTH is 48")]
pub const MAX_VALUE_DEPTH: u32 = monty_pool::MAX_VALUE_DEPTH as u32;

/// The live pool, shared between the pool object and its sessions. `None`
/// until `start()` and again after `close()`.
type SharedPool = Arc<Mutex<Option<Arc<Pool>>>>;
/// One session's worker handle. `None` before `enter()`, after `finish()`,
/// and after the worker is discarded on a crash.
type SharedCheckout = Arc<Mutex<Option<Checkout>>>;
/// The per-turn JS print callback, callable from the blocking turn thread.
type PrintCallback<'env> = Function<'env, FnArgs<(String, String)>, UnknownReturnValue>;

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
        config.checkout_timeout = options.checkout_timeout_ms.map(duration_from_ms).transpose()?;
        config.request_timeout = options.request_timeout_ms.map(duration_from_ms).transpose()?;
        config.duration_limit_grace = options.duration_limit_grace_ms.map(duration_from_ms).transpose()?;
        config.max_checkouts_per_worker = options.max_checkouts_per_worker;
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
            let pool = spawn_blocking(move || Pool::new(config))
                .await
                .map_err(task_error)?
                .map_err(pool_error)?;
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
            },
            checkout: Arc::new(Mutex::new(None)),
            pending_not_handled: Arc::new(Mutex::new(None)),
        })
    }

    /// Shuts the pool down: idle workers exit, capacity is gone. Sessions
    /// still checked out keep their workers until they finish.
    #[napi]
    pub fn close<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let slot = Arc::clone(&self.pool);
        env.spawn_future(async move {
            spawn_blocking(move || drop(lock(&slot).take()))
                .await
                .map_err(task_error)
        })
    }
}

/// One worker process dedicated to one REPL session. Wrapped by the
/// TypeScript `MontySession` class — not part of the public API.
#[napi(js_name = "NativeSession")]
pub struct NativeSession {
    pool: SharedPool,
    repl_config: ReplConfig,
    checkout: SharedCheckout,
    /// The exception the sandbox raises when the host declines the pending
    /// OS call. Kept Rust-side so the full exception (traceback included)
    /// round-trips instead of being rebuilt from strings.
    pending_not_handled: Arc<Mutex<Option<MontyException>>>,
}

#[napi]
impl NativeSession {
    /// Checks a worker out of the pool (spawning one if allowed) and creates
    /// the REPL session in it. Rejects with the pool error message on
    /// exhaustion or spawn failure.
    #[napi]
    pub fn enter<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let pool = Arc::clone(&self.pool);
        let repl_config = self.repl_config.clone();
        let slot = Arc::clone(&self.checkout);
        env.spawn_future(async move {
            spawn_blocking(move || {
                let pool = lock(&pool)
                    .as_ref()
                    .map(Arc::clone)
                    .ok_or_else(|| invalid("the pool is not started — create it with Monty.create()"))?;
                let checkout = pool.checkout(&repl_config).map_err(pool_error)?;
                *lock(&slot) = Some(checkout);
                Ok(())
            })
            .await
            .map_err(task_error)?
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
        mounts: Vec<NativeMount>,
        skip_type_check: bool,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let inputs = convert_inputs(env, inputs)?;
        let mounts = mounts
            .into_iter()
            .map(MountSpec::try_from)
            .collect::<Result<Vec<_>>>()?;
        self.run_turn(env, on_print, move |checkout, on_print| {
            checkout.feed(&code, inputs, mounts, skip_type_check, on_print)
        })
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
        self.run_turn(env, on_print, move |checkout, on_print| {
            checkout.resume(resume, on_print)
        })
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
        self.run_turn(env, on_print, move |checkout, on_print| {
            checkout.resume(ResumeValue::Error(exc), on_print)
        })
    }

    /// Answers an `osCall` suspension by declining it: the sandbox raises the
    /// call's default exception (full traceback preserved Rust-side).
    #[napi]
    pub fn resume_not_handled<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let exc = lock(&self.pending_not_handled)
            .take()
            .unwrap_or_else(|| MontyException::new(ExcType::RuntimeError, Some("OS call is not supported".to_owned())));
        self.run_turn(env, on_print, move |checkout, on_print| {
            checkout.resume(ResumeValue::Error(exc), on_print)
        })
    }

    /// Answers a `functionCall` suspension whose name has no handler: the
    /// sandbox raises `NameError`.
    #[napi]
    pub fn resume_not_found<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_turn(env, on_print, |checkout, on_print| {
            checkout.resume(ResumeValue::NotFound, on_print)
        })
    }

    /// Registers the pending call as an external future (the JS promise stays
    /// in TypeScript); other sandbox tasks keep executing.
    #[napi]
    pub fn resume_future<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_turn(env, on_print, |checkout, on_print| {
            checkout.resume(ResumeValue::Future, on_print)
        })
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
        self.run_turn(env, on_print, move |checkout, on_print| {
            checkout.resume_name_lookup(resolved, on_print)
        })
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
        self.run_turn(env, on_print, move |checkout, on_print| {
            checkout.resume_futures(results, on_print)
        })
    }

    /// Restores a dump into this session's freshly configured worker. Resolves
    /// to a turn object: a suspension when the dump was mid-feed, or `loaded`
    /// for an idle dump. The TypeScript `load` / `loadSnapshot` split inspects
    /// the kind and enforces "fresh session only".
    #[napi]
    pub fn restore<'env>(
        &self,
        env: &'env Env,
        state: Buffer,
        mounts: Vec<NativeMount>,
        on_print: PrintCallback<'env>,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let mounts = mounts
            .into_iter()
            .map(MountSpec::try_from)
            .collect::<Result<Vec<_>>>()?;
        let state = state.to_vec();
        self.run_outcome(env, on_print, move |checkout, on_print| {
            // JS snapshots expose no script name, so the restored name is unused
            match checkout.restore(state, mounts, on_print) {
                Ok((Some(event), _)) => TurnOutcome::Event(event),
                Ok((None, _)) => TurnOutcome::LoadedIdle,
                Err(err) => TurnOutcome::from(Err(err)),
            }
        })
    }

    /// Serializes the worker's session state (idle or suspended) into opaque
    /// bytes via monty's dump format. The session stays usable.
    #[napi]
    pub fn dump<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, Buffer>> {
        let slot = Arc::clone(&self.checkout);
        env.spawn_future(async move {
            spawn_blocking(move || {
                let mut guard = lock(&slot);
                let checkout = guard.as_mut().ok_or_else(|| pool_error(PoolError::Finished))?;
                checkout.dump().map(Buffer::from).map_err(pool_error)
            })
            .await
            .map_err(task_error)?
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
        self.run_outcome(env, on_print, move |checkout, _on_print| {
            match checkout.install_dependencies(requirements) {
                Ok(()) => TurnOutcome::Ok,
                Err(err) => TurnOutcome::from(Err::<TurnEvent, _>(err)),
            }
        })
    }

    /// Ends the session and returns the worker to the pool (best effort — a
    /// crashed worker has already been discarded and replaced).
    #[napi]
    pub fn finish<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let slot = Arc::clone(&self.checkout);
        env.spawn_future(async move {
            spawn_blocking(move || {
                // take in its own statement so the lock is released before
                // the (blocking) finish turn runs
                let checkout = lock(&slot).take();
                if let Some(checkout) = checkout {
                    // best effort: a worker that cannot reset is discarded by
                    // monty-pool itself
                    let _ = checkout.finish();
                }
            })
            .await
            .map_err(task_error)
        })
    }

    /// OS process id of this session's worker, or `null` when no worker is
    /// attached or a turn is in flight (the turn thread holds the checkout
    /// lock — blocking the event loop on it would deadlock with the print
    /// callback, which needs the event loop).
    #[napi(getter)]
    pub fn worker_pid(&self) -> Option<u32> {
        try_lock(&self.checkout)?.as_ref().and_then(Checkout::pid)
    }
}

impl NativeSession {
    /// Runs one protocol turn on the blocking pool and resolves it to a JS
    /// turn object. Pool-level failures (runtime error, typing error, crash,
    /// timeout, protocol desync) resolve as turn objects too — the
    /// TypeScript layer raises its public error classes from them; the
    /// promise only rejects for binding-level bugs.
    fn run_turn<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
        turn: impl FnOnce(&mut Checkout, OnPrint<'_>) -> StdResult<TurnEvent, PoolError> + Send + 'static,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        self.run_outcome(env, on_print, move |checkout, on_print| {
            TurnOutcome::from(turn(checkout, on_print))
        })
    }

    /// The shared turn machinery behind [`run_turn`](Self::run_turn): locks the
    /// checkout off the event loop, streams prints, and resolves the computed
    /// [`TurnOutcome`] to a JS turn object. `compute` returns the outcome
    /// directly so the `load` turn (which yields `Option<TurnEvent>`) can map
    /// its idle case to [`TurnOutcome::LoadedIdle`].
    fn run_outcome<'env>(
        &self,
        env: &'env Env,
        on_print: PrintCallback<'env>,
        compute: impl FnOnce(&mut Checkout, OnPrint<'_>) -> TurnOutcome + Send + 'static,
    ) -> Result<PromiseRaw<'env, Object<'env>>> {
        let tsfn = on_print.build_threadsafe_function().build()?;
        let slot = Arc::clone(&self.checkout);
        let pending_not_handled = Arc::clone(&self.pending_not_handled);
        env.spawn_future_with_callback(
            async move {
                spawn_blocking(move || {
                    let mut guard = lock(&slot);
                    let Some(checkout) = guard.as_mut() else {
                        return TurnOutcome::Protocol("the session is closed — check out a new one".to_owned());
                    };
                    // Forward each print to JS and *wait for the callback to
                    // run* (not merely queue), preserving print ordering
                    // relative to the turn's resolution and providing
                    // backpressure against print floods. The TypeScript
                    // wrapper captures callback failures itself and never
                    // throws back across this boundary.
                    let mut on_print = |stream: PrintStream, text: &str| {
                        let stream = match stream {
                            PrintStream::Stdout => "stdout",
                            PrintStream::Stderr => "stderr",
                        };
                        let _ = block_on(tsfn.call_async(FnArgs::from((stream.to_owned(), text.to_owned()))));
                    };
                    let outcome = compute(checkout, &mut on_print);
                    if let TurnOutcome::Event(TurnEvent::OsCall { not_handled_error, .. }) = &outcome {
                        lock(&pending_not_handled).clone_from(not_handled_error);
                    }
                    outcome
                })
                .await
                .map_err(task_error)
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
    /// A `load` restored an idle (between-feeds) session — there is no
    /// suspension to resume. Only produced by [`NativeSession::load`].
    LoadedIdle,
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
            Err(PoolError::Crashed { status, context }) => Self::Crashed {
                message: format!("monty worker crashed while {context}"),
                timed_out: false,
                exit_status: status.map(|status| status.to_string()),
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
            method_call,
        }) => {
            obj.set("kind", "functionCall")?;
            obj.set("functionName", function_name)?;
            obj.set("args", values_to_js(env, &args)?)?;
            obj.set("kwargs", pairs_to_js(env, &kwargs)?)?;
            obj.set("callId", call_id)?;
            obj.set("methodCall", method_call)?;
        }
        TurnOutcome::Event(TurnEvent::OsCall {
            function_name,
            args,
            kwargs,
            call_id,
            not_handled_error,
        }) => {
            obj.set("kind", "osCall")?;
            obj.set("functionName", function_name)?;
            obj.set("args", values_to_js(env, &args)?)?;
            obj.set("kwargs", pairs_to_js(env, &kwargs)?)?;
            obj.set("callId", call_id)?;
            if let Some(exc) = not_handled_error {
                obj.set("notHandledError", exception_to_js(env, &exc)?)?;
            }
        }
        TurnOutcome::Event(TurnEvent::NameLookup { name }) => {
            obj.set("kind", "nameLookup")?;
            obj.set("name", name)?;
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
    match js_to_monty(value, *env) {
        Ok(value) if exceeds_max_value_depth(&value) => ResumeValue::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("Max input depth exceeded".to_owned()),
        )),
        Ok(value) => ResumeValue::Return(value),
        Err(err) => ResumeValue::Error(MontyException::new(ExcType::TypeError, Some(err.reason.clone()))),
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
            .map(|limit| {
                if limit.is_finite() && limit >= 0.0 && limit.fract() == 0.0 {
                    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    Ok(limit as u64)
                } else {
                    Err(invalid("writeBytesLimit must be a non-negative integer"))
                }
            })
            .transpose()?;
        Ok(Self {
            virtual_path: mount.virtual_path,
            host_path: mount.host_path.into(),
            mode,
            write_bytes_limit,
        })
    }
}

/// Converts a millisecond count from JS into a `Duration`.
fn duration_from_ms(ms: f64) -> Result<Duration> {
    Duration::try_from_secs_f64(ms / 1000.0).map_err(|err| invalid(&format!("invalid timeout: {err}")))
}

/// Locks a shared slot, ignoring poisoning (a panic elsewhere must not wedge
/// the pool). Never call on the event-loop thread for the checkout slot — a
/// turn holds that lock for its whole duration; use [`try_lock`] there.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Non-blocking [`lock`]: `None` when the lock is held (e.g. by a turn in
/// flight). Safe to call on the event-loop thread.
fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(err)) => Some(err.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

fn pool_error(err: PoolError) -> napi::Error {
    napi::Error::from_reason(err.to_string())
}

fn task_error(err: impl fmt::Display) -> napi::Error {
    napi::Error::from_reason(format!("worker task failed: {err}"))
}

fn invalid(message: &str) -> napi::Error {
    napi::Error::new(napi::Status::InvalidArg, message.to_owned())
}
