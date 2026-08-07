#![doc = include_str!("../README.md")]

mod checkout;
mod pool;
#[cfg(feature = "telemetry-adapter")]
mod telemetry;
#[cfg(feature = "telemetry-adapter")]
pub mod telemetry_adapter;
#[cfg(feature = "telemetry-adapter")]
mod telemetry_json;
mod worker;

use std::{borrow::Cow, error, fmt, io, num::NonZero, path::PathBuf, process::ExitStatus, thread, time::Duration};

pub use monty_proto::{MAX_VALUE_DEPTH, exceeds_max_value_depth};
use monty_types::MontyException;

pub use crate::{
    checkout::{
        Checkout, MountSpec, MountSpecMode, OnPrint, OnRawEvent, PrintFuture, ReplConfig, ResumeValue, TurnEvent,
        on_print_sync,
    },
    pool::Pool,
};

/// How the pool reaches its workers.
#[derive(Debug, Clone)]
pub enum MontyTransport {
    /// Spawn a local `monty subprocess` child and talk to it over framed
    /// stdio pipes. Takes path to the `monty` (or compatible child) binary.
    Subprocess(PathBuf),
    /// Connect *out* to a remote child over a WebSocket — either a relay (which
    /// pairs this connection with a child that dialed in with the same session
    /// id) or a child running a server. One binary message per protocol frame.
    ///
    /// The URL is dialed verbatim — if a relay needs the two ends to share a
    /// session id in the path (`/<uuid>/parent`), the caller is responsible for
    /// putting it there. Takes full `ws://`/`wss://` URL to dial.
    Websocket(String),
}

impl MontyTransport {
    /// Whether this is the remote WebSocket transport, whose workers are
    /// single-use (dialed per checkout, never pooled idle or reused).
    pub(crate) fn is_websocket(&self) -> bool {
        matches!(self, Self::Websocket(_))
    }
}

/// Configuration for a [`Pool`].
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Workers spawned eagerly at pool creation and kept warm. Forced to 0 for
    /// the [`MontyTransport::Websocket`] transport (connections are made
    /// per-checkout, not pre-warmed).
    pub min_processes: usize,
    /// Hard cap on live workers; checkouts beyond this wait.
    pub max_processes: usize,
    /// How workers are reached (spawned locally or connected to remotely).
    pub transport: MontyTransport,
    /// How long [`Pool::checkout`] waits for a free worker before
    /// [`PoolError::Exhausted`]. `None` waits forever.
    pub checkout_timeout: Option<Duration>,
    /// Parent-side hard deadline per protocol turn: when it expires the
    /// worker is killed and the call fails with [`PoolError::Timeout`]. This
    /// backstops the child-side `ResourceLimits` — it also catches a child
    /// that hangs in ways the sandbox limits cannot see. Synchronous host
    /// telemetry callbacks prevent the timer from being polled while they run.
    pub request_timeout: Option<Duration>,
    /// Grace period for the automatic `max_duration` backstop.
    ///
    /// When a session has a `ResourceLimits::max_duration` budget, the worker
    /// reports its cumulative execution time on every turn-ending event (the
    /// sandbox clock is the single source of truth: it runs only while the
    /// interpreter executes, never during suspensions waiting on the host or
    /// between feeds), and the parent bounds each execution turn by the
    /// remaining budget plus this grace.
    pub duration_limit_grace: Option<Duration>,
    /// Recycle (kill and respawn) a worker after this many checkouts, to
    /// bound the impact of any slow leak in a long-lived child.
    pub max_checkouts_per_worker: Option<u32>,
}

impl PoolConfig {
    /// Creates a subprocess-transport config with defaults: `min_processes = 1`,
    /// `max_processes =` available parallelism, no timeouts, a 1s
    /// `duration_limit_grace`, no recycling.
    pub fn subprocess(binary_path: impl Into<PathBuf>) -> Self {
        Self::with_transport(MontyTransport::Subprocess(binary_path.into()))
    }

    /// Creates a WebSocket-transport config dialing `url` verbatim per checkout.
    /// `min_processes` is 0 (no pre-warming — connections are made per-checkout).
    pub fn websocket(url: impl Into<String>) -> Self {
        let mut config = Self::with_transport(MontyTransport::Websocket(url.into()));
        config.min_processes = 0;
        config
    }

    /// Shared constructor for both transports.
    fn with_transport(transport: MontyTransport) -> Self {
        Self {
            min_processes: 1,
            max_processes: thread::available_parallelism().map_or(4, NonZero::get),
            transport,
            checkout_timeout: None,
            request_timeout: None,
            duration_limit_grace: Some(Duration::from_secs(1)),
            max_checkouts_per_worker: None,
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
    /// The worker was killed after its turn outlived `request_timeout` (or
    /// the `max_duration` backstop deadline).
    Timeout {
        /// The configured timeout that expired.
        timeout: Duration,
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
    /// A worker process could not be spawned.
    Spawn(String),
    /// The checkout was already finished or its worker already discarded.
    Finished,
    /// The remote worker's connection dropped without a turn-ending event
    /// (WebSocket transport only — the local analogue is [`Self::Crashed`]).
    /// The sandbox may have died, or the server may have dropped the session
    /// by policy (idle/session/turn timeout, capacity); the two are
    /// indistinguishable to a client that only learns of the close, so this
    /// deliberately says no more than "the connection went away".
    Disconnected {
        /// What the pool was doing when the disconnect was observed.
        context: String,
    },
    /// The remote server is shutting down and did **not** run the request —
    /// re-running it on a fresh session is safe. `dump` carries the session
    /// state captured just before shutdown, restorable via
    /// [`Checkout::restore`] on a fresh checkout.
    Shutdown {
        /// Restorable session dump, absent when there was no session yet or
        /// the server's dump failed.
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
            Self::Finished => f.write_str("this checkout has already been finished"),
            Self::Disconnected { context } => write!(f, "monty worker connection closed while {context}"),
            Self::Shutdown { dump } => match dump {
                Some(_) => {
                    f.write_str("monty server is shutting down; the request did not run (session dump attached)")
                }
                None => f.write_str("monty server is shutting down; the request did not run"),
            },
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
