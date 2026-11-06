#![doc = include_str!("../README.md")]

mod checkout;
mod cpu;
mod cpu_clock;
mod pool;
mod remote;
#[cfg(feature = "telemetry")]
pub mod telemetry;
/// Compatibility alias for the former public module name.
#[cfg(feature = "telemetry")]
pub use telemetry as telemetry_adapter;
mod worker;

use std::{borrow::Cow, error, fmt, io, num::NonZero, path::PathBuf, process::ExitStatus, thread, time::Duration};

use monty_proto::DEFAULT_MAX_DECODE_BYTES;
pub use monty_proto::DEFAULT_PRINT_FLUSH_INTERVAL;
/// Why a remote server ended a session's stream; see [`RemoteDisconnect::cause`].
pub use monty_proto::grpc::Cause as RemoteCause;
use monty_types::{ExecutionClock, MontyException};

#[cfg(feature = "telemetry")]
use crate::telemetry::Metrics;
pub use crate::{
    checkout::{
        Checkout, CheckoutOptions, MountSpec, MountSpecMode, OnPrint, OnRawEvent, Persistence, PrintFuture, ReplConfig,
        ResumeValue, StreamedCall, TurnEvent, on_print_sync,
    },
    pool::Pool,
};

/// How the pool reaches its workers.
#[derive(Debug, Clone)]
pub enum MontyTransport {
    /// Spawn a local `monty subprocess` child and talk to it over framed
    /// stdio pipes. Takes path to the `monty` (or compatible child) binary.
    Subprocess(PathBuf),
    /// Connect *out* to a remote server speaking the protocol over gRPC: one
    /// HTTP/2 connection per pool, one `MontySession.Session` stream per
    /// checkout, carrying one protocol message per gRPC message. The intended
    /// peer is a relay such as `monty-server`, which pairs each stream with a
    /// worker of its own.
    ///
    /// Takes the `http://` or `https://` URL of the server; `ws://` and
    /// `wss://` are accepted and read as those two.
    Remote(String),
}

impl MontyTransport {
    /// Whether this is the remote transport, whose workers are single-use (a
    /// stream opened per checkout, never pooled idle or reused).
    pub(crate) fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }
}

/// Default grace on each of the two duration backstops: how long the parent
/// waits past a sandbox time limit for the worker to raise `TimeoutError`
/// itself before killing it.
pub const DEFAULT_DURATION_LIMIT_GRACE: Duration = Duration::from_secs(1);

/// Default [`PoolConfig::keep_alive`]: the fastest cadence a gRPC server's
/// default ping policy permits (grpc-go, grpc-java and grpc-core all refuse
/// pings closer than five minutes apart and answer with `GOAWAY` after two).
pub const DEFAULT_KEEP_ALIVE: Duration = Duration::from_mins(5);

/// Default [`PoolConfig::max_checkouts_per_worker`]: bounds how many sessions
/// one worker serves while amortising its respawn over many checkouts.
pub const DEFAULT_MAX_CHECKOUTS_PER_WORKER: u32 = 1000;

/// Configuration for a [`Pool`].
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Floor on live workers: spawned at pool creation, and replaced in the
    /// background whenever one is recycled, crashes or is discarded. Forced to
    /// 0 for the [`MontyTransport::Remote`] transport (streams are opened
    /// per-checkout, not pre-warmed).
    pub min_processes: usize,
    /// Hard cap on live workers; checkouts beyond this wait. For the remote
    /// transport, a cap on concurrent streams.
    pub max_processes: usize,
    /// How workers are reached (spawned locally or connected to remotely).
    pub transport: MontyTransport,
    /// Clock for local worker sessions (default wall time). CPU mode requires Linux
    /// subprocess workers and a finite [`Self::request_timeout`].
    /// Remote pools adopt the server's clock without local CPU supervision.
    pub execution_clock: ExecutionClock,
    /// How long [`Pool::checkout`] waits for a free worker before
    /// [`PoolError::Exhausted`]. `None` waits forever.
    pub checkout_timeout: Option<Duration>,
    /// How often the remote transport sends an HTTP/2 `PING` on its
    /// connection while a stream is open, to notice a dead TCP peer on the
    /// first hop; `None` sends none. Liveness through proxies is the server's
    /// in-band `Ping`, so this is a backstop, and a server may police it: a
    /// cadence faster than its policy allows loses the connection, and every
    /// stream on it, to `GOAWAY`. Defaults to [`DEFAULT_KEEP_ALIVE`]. Ignored
    /// by the subprocess transport.
    pub keep_alive: Option<Duration>,
    /// Parent-side hard deadline per protocol turn: when it expires the
    /// worker is killed and the call fails with [`PoolError::Timeout`]. This
    /// backstops the child-side `ResourceLimits` — it also catches a child
    /// that hangs in ways the sandbox limits cannot see. Synchronous host
    /// telemetry callbacks prevent the timer from being polled while they run.
    /// On the remote transport it also bounds opening a stream, from a cold
    /// dial through the server's admission (30 s when `None`); that expiry
    /// fails with [`PoolError::Spawn`] instead, and there is no worker to kill.
    pub request_timeout: Option<Duration>,
    /// Per-frame decode allocation budget, capped at [`DEFAULT_MAX_DECODE_BYTES`].
    /// Excludes wire buffers and host-language conversion, and does not change
    /// the sandbox's memory limit. On the remote transport it also caps each
    /// message's size (between 64 KiB and the 256 MiB frame limit), refusing
    /// a larger one unbuffered.
    pub max_decode_bytes: usize,
    /// Extra feed allowance in the pool's clock. CPU mode grants it once per feed.
    /// Wall mode uses reported time; CPU mode uses OS CPU. Exhaustion kills with `Timeout` or `CpuLimit`, respectively.
    /// `None` disables this backstop. `request_timeout` remains independent.
    pub feed_duration_limit_grace: Option<Duration>,
    /// Extra allowance for `ResourceLimits::max_turn_duration`, in the pool's clock.
    /// Each feed or resume starts a new turn allowance. `None` disables this backstop.
    pub turn_duration_limit_grace: Option<Duration>,
    /// Recycle (kill and respawn) a worker after this many checkouts, bounding
    /// how long a slow leak or a compromised child survives. Defaults to
    /// [`DEFAULT_MAX_CHECKOUTS_PER_WORKER`]; `None` never recycles. With the
    /// subprocess transport, `Some(1)` (or `Some(0)`) gives every session a fresh process — the
    /// setting for mutually untrusted callers — usually spawned before it asked
    /// when `min_processes > 0`.
    pub max_checkouts_per_worker: Option<u32>,
    /// Resume a session transparently when a relay that stores sessions answers
    /// a request with `Shutdown`: the checkout redials, reloads the state the
    /// shutdown named and re-sends the request the relay reported it did not run. The session's suspension
    /// and sleep totals carry over. Remote transport only; on by default.
    pub auto_resume: bool,
    /// Let calls to async host functions that the sandbox does not await at once
    /// stream to the host without suspending the worker (`Configure.accepts_streamed_calls`).
    /// Saves a round trip per call, which matters most over the network, but moves
    /// call-time errors to the `await`. On by default for [`Self::remote`], off for [`Self::subprocess`].
    pub stream_async_calls: bool,
    /// Where pool and turn metrics are recorded, from
    /// [`TelemetryAdapterHandle::metrics`](telemetry::TelemetryAdapterHandle::metrics).
    /// `None` records nothing at all. Independent of tracing: metrics cover
    /// every checkout, traced or not. Worker up/down counters total over all
    /// pools that record into the same host meter.
    #[cfg(feature = "telemetry")]
    pub metrics: Option<Metrics>,
}

impl PoolConfig {
    /// Creates a subprocess-transport config with defaults: `min_processes = 1`,
    /// `max_processes =` available parallelism, no timeouts, a 1s grace on
    /// each of the two duration backstops, recycling after
    /// [`DEFAULT_MAX_CHECKOUTS_PER_WORKER`] checkouts.
    pub fn subprocess(binary_path: impl Into<PathBuf>) -> Self {
        Self::with_transport(MontyTransport::Subprocess(binary_path.into()))
    }

    /// Creates a remote-transport config for the gRPC server at `url`, with a
    /// stream opened per checkout. `min_processes` is 0 (no pre-warming) and
    /// `stream_async_calls` is on.
    pub fn remote(url: impl Into<String>) -> Self {
        let mut config = Self::with_transport(MontyTransport::Remote(url.into()));
        config.min_processes = 0;
        config.stream_async_calls = true;
        config
    }

    /// Shared constructor for both transports.
    fn with_transport(transport: MontyTransport) -> Self {
        Self {
            min_processes: 1,
            max_processes: thread::available_parallelism().map_or(4, NonZero::get),
            transport,
            execution_clock: ExecutionClock::Wall,
            checkout_timeout: None,
            keep_alive: Some(DEFAULT_KEEP_ALIVE),
            request_timeout: None,
            feed_duration_limit_grace: Some(DEFAULT_DURATION_LIMIT_GRACE),
            turn_duration_limit_grace: Some(DEFAULT_DURATION_LIMIT_GRACE),
            max_checkouts_per_worker: Some(DEFAULT_MAX_CHECKOUTS_PER_WORKER),
            auto_resume: true,
            stream_async_calls: false,
            max_decode_bytes: DEFAULT_MAX_DECODE_BYTES,
            #[cfg(feature = "telemetry")]
            metrics: None,
        }
    }
}

/// Why a pool operation failed.
#[derive(Debug)]
pub enum PoolError {
    /// The worker is gone: it died (segfault, abort, external kill, or EOF on
    /// its pipes), or it announced a `FatalError` and exited — which a serving
    /// relay also uses to report that it could not start a worker at all. The
    /// worker has been discarded; the pool stays usable.
    Crashed {
        /// Exit status, when the process could be reaped.
        status: Option<ExitStatus>,
        /// How the pool learned of the death, and what it can say about it.
        cause: CrashCause,
    },
    /// The worker exceeded `request_timeout` or a wall-mode feed/turn backstop.
    Timeout {
        /// The configured timeout that expired.
        timeout: Duration,
    },
    /// The supervisor killed the worker after its OS-observed CPU allowance expired.
    CpuLimit {
        /// The remaining CPU allowance for this turn, including any configured CPU grace.
        limit: Duration,
        /// CPU consumed since the supervisor's fresh per-turn OS baseline.
        consumed: Duration,
    },
    /// The worker violated the wire protocol, or the caller violated the
    /// checkout state machine. Worker-originated protocol failures discard the
    /// worker; caller misuse leaves it intact — except a turn cancelled
    /// mid-flight, where the abandoned worker is discarded too.
    Protocol(Cow<'static, str>),
    /// The sandboxed code raised a Python exception. The worker and its
    /// session remain alive and usable — except for the one `MemoryError` a
    /// worker exceeding its memory limit produces, where the worker is already
    /// dead and the checkout finished (its distinct message distinguishes it
    /// from the interpreter's own `MemoryError`).
    Runtime(MontyException),
    /// Type checking rejected the fed snippet (sessions created with
    /// `type_check`). The worker and session remain alive; the snippet did
    /// not run.
    Typing(String),
    /// No worker became available within `checkout_timeout`.
    Exhausted,
    /// A worker process could not be spawned, or a remote stream could not
    /// be opened for a reason other than the server refusing it: a dial that
    /// failed or timed out, a malformed metadata entry.
    Spawn(String),
    /// The remote server refused to open the session stream (remote transport
    /// only) with an admission code: `RESOURCE_EXHAUSTED`, `UNAVAILABLE`,
    /// `UNAUTHENTICATED`, `PERMISSION_DENIED` or `FAILED_PRECONDITION`; any
    /// other status is [`Self::Spawn`]. `refusal` carries the status, so a
    /// caller can act on its code and cause rather than on the message. No
    /// session existed, so nothing ran.
    Refused {
        /// The server's URL, as configured.
        url: String,
        /// The status the refusal named: its code, message and cause.
        refusal: Box<RemoteDisconnect>,
    },
    /// The checkout was already finished or its worker already discarded.
    Finished,
    /// The remote worker's stream ended without a turn-ending event (remote
    /// transport only — the local analogue is [`Self::Crashed`]). The sandbox
    /// may have died, the connection may have dropped, or the server may have
    /// ended the session by policy — in which case `remote` carries the
    /// status it ended with and, when the server named one, the cause.
    /// Neither says whether the request ran: a disconnect is not a verdict on
    /// it, so it is not replayed.
    Disconnected {
        /// What the pool was doing when the disconnect was observed.
        context: String,
        /// Transport evidence, absent when the reader task supplied no details.
        remote: Option<Box<RemoteDisconnect>>,
    },
    /// The remote server is shutting down and did **not** run the request —
    /// re-running it on a fresh session is safe. `dump` restores the session
    /// via [`Checkout::restore`] on a fresh checkout: the session's ID from a
    /// relay that stores sessions, returned only when
    /// [`PoolConfig::auto_resume`] could not resume it.
    Shutdown {
        /// What restores the session; absent when there is nothing to load —
        /// no session yet, an ephemeral one, a relay without storage, or a
        /// park that failed.
        dump: Option<Vec<u8>>,
    },
}

/// How the pool learned a worker had died — the two are mutually exclusive,
/// which is why they are one field rather than two `Option`s: a worker that
/// states its own reason makes the pool's note of what it was doing
/// redundant, and the message uses one or the other, never both.
///
/// Orthogonal to [`PoolError::Crashed`]'s `status`, which is present or not
/// in either case (a worker can announce a reason *and* be reaped for its
/// exit code — the usual shape of a version-skew death).
#[derive(Debug)]
pub enum CrashCause {
    /// It vanished — segfault, abort, external kill, EOF on its pipes — so
    /// all the pool can say is what it was doing at the time.
    Vanished {
        /// What the pool was doing when the death was observed.
        context: String,
    },
    /// It announced a `FatalError` and exited, so its own account replaces
    /// the pool's. A serving relay also uses this to report that it could not
    /// start a worker at all. A worker that exceeded its memory limit is not
    /// here at all: that exit code classifies into
    /// [`PoolError::Runtime`] carrying a `MemoryError`.
    Announced {
        /// What the worker said before dying.
        reason: String,
    },
}

/// The first recorded stream failure, without a claim about whether guest code ran.
///
/// Exactly one of two shapes: a status the server ended the stream with
/// (`status_code`, `status_message`, and `cause` when it sent one), or a
/// transport failure (`transport_error`) for which there was no status.
#[derive(Debug, Clone)]
pub struct RemoteDisconnect {
    /// The gRPC status code the server ended the stream with, by its
    /// canonical name (`DEADLINE_EXCEEDED`, `RESOURCE_EXHAUSTED`, ...);
    /// `OK` when it simply finished the stream.
    pub status_code: Option<&'static str>,
    /// The message on that status.
    pub status_message: Option<String>,
    /// The policy reason the server attached to that status, when it is one
    /// this build knows.
    pub cause: Option<RemoteCause>,
    /// The transport or protocol error for this failure; absent when the
    /// server sent a status.
    pub transport_error: Option<String>,
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crashed { status, cause } => {
                match cause {
                    CrashCause::Announced { reason } => write!(f, "monty worker crashed: {reason}")?,
                    CrashCause::Vanished { context } => write!(f, "monty worker crashed while {context}")?,
                }
                match status {
                    Some(status) => write!(f, " ({status})"),
                    None => Ok(()),
                }
            }
            Self::Timeout { timeout } => {
                write!(f, "monty worker killed after exceeding request timeout of {timeout:?}")
            }
            Self::Protocol(msg) => write!(f, "monty worker protocol error: {msg}"),
            Self::Runtime(exc) => write!(f, "{exc}"),
            Self::Typing(diagnostics) => write!(f, "type checking failed:\n{diagnostics}"),
            Self::Exhausted => f.write_str("no monty worker became available within the checkout timeout"),
            Self::Spawn(msg) => write!(f, "failed to spawn monty worker: {msg}"),
            Self::Refused { url, refusal } => write!(
                f,
                "failed to spawn monty worker: {url}: {}: {}",
                refusal.status_code.unwrap_or("UNKNOWN"),
                refusal.status_message.as_deref().unwrap_or_default()
            ),
            Self::Finished => f.write_str("this checkout has already been finished"),
            Self::Disconnected { context, .. } => write!(f, "monty worker connection closed while {context}"),
            Self::Shutdown { dump } => match dump {
                Some(_) => {
                    f.write_str("monty server is shutting down; the request did not run (restorable state attached)")
                }
                None => f.write_str("monty server is shutting down; the request did not run"),
            },
            Self::CpuLimit { limit, consumed } => {
                write!(
                    f,
                    "monty worker exceeded CPU allowance {limit:?} (observed {consumed:?})"
                )
            }
        }
    }
}

impl error::Error for PoolError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Runtime(exc) => Some(exc),
            _ => None,
        }
    }
}

impl From<io::Error> for PoolError {
    fn from(err: io::Error) -> Self {
        Self::Crashed {
            status: None,
            cause: CrashCause::Vanished {
                context: format!("performing I/O: {err}"),
            },
        }
    }
}
