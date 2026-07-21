#![doc = include_str!("../README.md")]

mod checkout;
mod pool;
mod watchdog;
mod worker;

use std::{borrow::Cow, error, fmt, io, num::NonZero, path::PathBuf, process::ExitStatus, thread, time::Duration};

use monty::MontyException;
pub use monty_proto::{MAX_VALUE_DEPTH, exceeds_max_value_depth};

pub use crate::{
    checkout::{Checkout, MountSpec, MountSpecMode, OnPrint, ReplConfig, ResumeValue, TurnEvent},
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
    /// that hangs in ways the sandbox limits cannot see.
    pub request_timeout: Option<Duration>,
    /// Grace period for the automatic `max_duration` backstop.
    ///
    /// When a session has a `ResourceLimits::max_duration` budget, the worker
    /// reports its cumulative execution time on every turn-ending event (the
    /// sandbox clock is the single source of truth: it runs only while the
    /// interpreter executes, never during suspensions waiting on the host or
    /// between feeds), and the parent arms each execution turn's watchdog
    /// with the remaining budget plus this grace.
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
    /// The worker process died (segfault, abort, external kill, or EOF on
    /// its pipes). The worker has been discarded; the pool stays usable.
    Crashed {
        /// Exit status, when the process could be reaped.
        status: Option<ExitStatus>,
        /// What the pool was doing when the death was observed.
        context: String,
    },
    /// The watchdog killed the worker after `request_timeout` elapsed.
    Timeout {
        /// The configured timeout that expired.
        timeout: Duration,
    },
    /// The worker violated the wire protocol, or the caller violated the
    /// checkout state machine. Worker-originated protocol failures discard the
    /// worker; caller misuse leaves it intact.
    Protocol(Cow<'static, str>),
    /// The sandboxed code raised a Python exception. The worker and its
    /// session remain alive and usable.
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
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crashed { status, context } => match status {
                Some(status) => write!(f, "monty worker crashed while {context}: {status}"),
                None => write!(f, "monty worker crashed while {context}"),
            },
            Self::Timeout { timeout } => {
                write!(f, "monty worker killed after exceeding request timeout of {timeout:?}")
            }
            Self::Protocol(msg) => write!(f, "monty worker protocol error: {msg}"),
            Self::Runtime(exc) => write!(f, "{exc}"),
            Self::Typing(diagnostics) => write!(f, "type checking failed:\n{diagnostics}"),
            Self::Exhausted => f.write_str("no monty worker became available within the checkout timeout"),
            Self::Spawn(msg) => write!(f, "failed to spawn monty worker: {msg}"),
            Self::Finished => f.write_str("this checkout has already been finished"),
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
            context: format!("performing I/O: {err}"),
        }
    }
}
