//! A single worker the pool drives over the wire protocol: either a local
//! `monty subprocess` child (framed stdio) or a remote child reached over a
//! WebSocket. Both expose the same send/recv/kill surface so the checkout turn
//! loop and the watchdog are transport-agnostic.
//!
//! TODO(async pool): the pool is blocking/threaded, so the WebSocket worker uses
//! a *synchronous* client and each in-flight remote turn pins one thread for the
//! whole network round trip. To scale to many concurrent remote sandboxes,
//! `monty-pool` should become async end-to-end (tokio + `tokio-tungstenite`) so
//! those turns share event-loop threads instead of one blocking thread each.

use std::{
    env,
    net::{Shutdown, TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, Once, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use monty_proto::{FrameError, FrameReader, MAX_FRAME_LEN, decode_frame, encode_to_capped_vec, pb, write_frame};
use rustls::crypto::aws_lc_rs::default_provider;
use tungstenite::{
    Error as WsError, Message, WebSocket,
    client::{IntoClientRequest, uri_mode},
    client_tls_with_config,
    protocol::WebSocketConfig,
    stream::{MaybeTlsStream, Mode},
};

use crate::{MontyTransport, PoolConfig, PoolError};

/// The synchronous WebSocket client socket type for a remote worker.
type WsSocket = WebSocket<MaybeTlsStream<TcpStream>>;

/// A worker plus its recycle counter. The transport-specific I/O state lives in
/// [`WorkerKind`]; `checkouts_served` is shared logic.
pub(crate) struct Worker {
    kind: WorkerKind,
    /// Shared kill channel: the watchdog clones it to interrupt this worker's
    /// blocked read on a deadline, and the worker reads/resets the timeout flag
    /// and kills through it during teardown. Identical for both transports, so
    /// it lives here rather than being duplicated inside each [`WorkerKind`].
    interrupt: Arc<Interrupt>,
    /// Checkouts this worker has served, for `max_checkouts_per_worker`.
    pub(crate) checkouts_served: u32,
}

/// The two transports a worker can speak the protocol over.
enum WorkerKind {
    Subprocess(SubprocessWorker),
    // Boxed: the WebSocket socket (with its TLS state and buffers) is far larger
    // than the subprocess handle, so inlining it would bloat every `Worker`.
    WebSocket(Box<WebSocketWorker>),
}

/// A local `monty subprocess` child with framed stdio pipes.
///
/// The `Child` handle lives behind `Arc<Mutex<..>>` so the watchdog can kill the
/// process while the owning thread is blocked reading from it.
struct SubprocessWorker {
    /// The child handle, shared (`Arc`) with the worker's [`Interrupt`] so the
    /// watchdog can kill the process while the owner is blocked reading it.
    child: Arc<Mutex<Child>>,
    writer: ChildStdin,
    reader: FrameReader<ChildStdout>,
}

/// A remote child reached over a WebSocket. One binary message per protocol
/// frame (no length prefix — the message boundary is the frame). The watchdog
/// interrupts a blocked read by shutting down the raw TCP socket the worker's
/// [`Interrupt`] holds a clone of (the WebSocket analogue of killing a child).
struct WebSocketWorker {
    socket: WsSocket,
    /// Set once the connection is closed/killed, so `is_dead` reports it.
    closed: Arc<AtomicBool>,
}

impl WebSocketWorker {
    /// Sends one event as a single binary WebSocket message (no length prefix), and
    /// flushes — the protocol is strict alternation, so the frame must hit the wire.
    fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        let body = encode_to_capped_vec(request)?;
        self.socket
            .write(Message::Binary(body.into()))
            .map_err(ws_to_frame_error)?;
        self.socket.flush().map_err(ws_to_frame_error)?;
        Ok(())
    }

    /// Reads one `ChildEvent` from the WebSocket, skipping control frames. A
    /// close/EOF *without* a prior turn-ender means the child died — surfaced as
    /// [`FrameError::Truncated`], mirroring the stdio crash contract.
    fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        loop {
            match self.socket.read() {
                Ok(Message::Binary(data)) => return decode_frame::<pb::ChildEvent>(data.as_ref()),
                // tungstenite auto-queues the Pong; flush it and keep reading.
                Ok(Message::Ping(_)) => {
                    let _ = self.socket.flush();
                }
                Ok(Message::Pong(_)) => {}
                // A clean close, or text/raw frames the protocol never uses.
                Ok(Message::Close(_) | Message::Text(_) | Message::Frame(_)) => return Err(FrameError::Truncated),
                Err(WsError::Io(err)) => return Err(FrameError::Io(err)),
                Err(_) => return Err(FrameError::Truncated),
            }
        }
    }
}

impl Worker {
    pub(crate) fn new(config: &PoolConfig) -> Result<Self, PoolError> {
        match &config.transport {
            MontyTransport::Subprocess(binary_path) => Self::subprocess(binary_path),
            // Bound the dial by `request_timeout` (see `websocket`); a missing
            // one falls back to a generous fixed budget.
            MontyTransport::Websocket(url) => {
                Self::websocket(url, config.request_timeout.unwrap_or(DEFAULT_DIAL_TIMEOUT))
            }
        }
    }

    /// Spawns a local `monty subprocess` child with framed pipes.
    ///
    /// There is no spawn-time handshake: a wrong or broken binary surfaces as
    /// an error on the first request the worker serves (typically the
    /// `Configure` of its first checkout).
    fn subprocess(binary_path: &PathBuf) -> Result<Self, PoolError> {
        let mut command = Command::new(binary_path);
        command
            .arg("subprocess")
            // For extra safety, spawn the worker with an empty environment.
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        // Windows processes misbehave without SystemRoot (CRT and WinAPI
        // lookups); it names the OS install directory and is not sensitive.
        if cfg!(windows)
            && let Ok(system_root) = env::var("SystemRoot")
        {
            command.env("SystemRoot", system_root);
        }
        let mut child = command
            // stderr is inherited: child diagnostics stay visible to the host
            .spawn()
            .map_err(|err| PoolError::Spawn(format!("{}: {err}", binary_path.display())))?;

        let writer = child.stdin.take().expect("piped stdin");
        let reader = FrameReader::new(child.stdout.take().expect("piped stdout"));
        let child = Arc::new(Mutex::new(child));
        let interrupt = Interrupt::new(InterruptKind::Subprocess(Arc::clone(&child)));
        Ok(Self::with_kind(
            WorkerKind::Subprocess(SubprocessWorker { child, writer, reader }),
            interrupt,
        ))
    }

    /// Connects to a remote child over a WebSocket, dialing `url` verbatim. Any
    /// session/rendezvous routing the URL needs is the caller's responsibility.
    ///
    /// `timeout` bounds the dial (TCP connect + TLS/WS handshake): `checkout_timeout`
    /// only covers waiting for capacity, not the synchronous handshake that follows,
    /// so a hung dial would otherwise stall the checkout forever.
    fn websocket(url: &str, timeout: Duration) -> Result<Self, PoolError> {
        install_crypto_provider();
        let socket = dial_ws(url, timeout)?;
        // Clone the underlying TCP socket up front for the watchdog's interrupt
        // handle (reaching it through the TLS stream once connected). Without it
        // the watchdog could never unblock a hung read, silently voiding the
        // hard-timeout guarantee — so refuse the worker rather than build one we
        // can't kill.
        let tcp = underlying_tcp(socket.get_ref())
            .and_then(|tcp| tcp.try_clone().ok())
            .ok_or_else(|| {
                PoolError::Spawn(format!(
                    "{url}: could not clone the connection socket for timeout enforcement"
                ))
            })?;
        let interrupt = Interrupt::new(InterruptKind::WebSocket(tcp));
        Ok(Self::with_kind(
            WorkerKind::WebSocket(Box::new(WebSocketWorker {
                socket,
                closed: Arc::new(AtomicBool::new(false)),
            })),
            interrupt,
        ))
    }

    fn with_kind(kind: WorkerKind, interrupt: Arc<Interrupt>) -> Self {
        Self {
            kind,
            interrupt,
            checkouts_served: 0,
        }
    }

    pub(crate) fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => write_frame(&mut w.writer, request),
            WorkerKind::WebSocket(w) => w.send(request),
        }
    }

    /// Reads one event; EOF/close is an error here because within a checkout the
    /// child must never close its side first.
    pub(crate) fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => w.reader.read::<pb::ChildEvent>()?.ok_or(FrameError::Truncated),
            WorkerKind::WebSocket(w) => w.recv(),
        }
    }

    /// The OS process id, when the worker is a local subprocess (`None` for a
    /// remote WebSocket worker, which has no local process).
    pub(crate) fn pid(&self) -> Option<u32> {
        match &self.kind {
            WorkerKind::Subprocess(w) => Some(lock_ignore_poison(&w.child).id()),
            WorkerKind::WebSocket(_) => None,
        }
    }

    /// The worker's shared kill channel. The watchdog clones it to arm a
    /// deadline; the worker reads and resets the timeout flag through it.
    pub(crate) fn interrupt(&self) -> &Arc<Interrupt> {
        &self.interrupt
    }

    /// Whether the watchdog killed this worker (consumes the flag's meaning:
    /// call once when classifying a read failure).
    pub(crate) fn was_killed_for_timeout(&self) -> bool {
        self.interrupt().was_killed_for_timeout()
    }

    /// Clears the sticky timeout flag at the start of a turn, scoping it to the
    /// currently-armed deadline. The watchdog sets the flag but never clears it,
    /// so without this reset a stale kill could misclassify the next turn's
    /// first I/O failure as a timeout.
    pub(crate) fn reset_killed_for_timeout(&self) {
        self.interrupt().reset_killed_for_timeout();
    }

    /// Whether the worker has already died (used to discard workers that died
    /// while idle in the pool). WebSocket workers are never pooled idle, so
    /// this only reflects an already-observed close for them.
    pub(crate) fn is_dead(&self) -> bool {
        match &self.kind {
            WorkerKind::Subprocess(w) => lock_ignore_poison(&w.child).try_wait().is_ok_and(|s| s.is_some()),
            WorkerKind::WebSocket(w) => w.closed.load(Ordering::SeqCst),
        }
    }

    /// Tears the worker down (kills the child / closes the socket) and reaps it,
    /// returning the process exit status when there is one.
    pub(crate) fn kill_and_reap(&mut self) -> Option<ExitStatus> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                let mut child = lock_ignore_poison(&w.child);
                let _ = child.kill();
                child.wait().ok()
            }
            WorkerKind::WebSocket(w) => {
                w.closed.store(true, Ordering::SeqCst);
                // Shut the TCP socket down directly rather than writing a
                // WebSocket Close frame: the socket's write timeout was cleared
                // after the handshake, so `socket.close()` could block
                // indefinitely on a peer that has stopped draining — and this
                // runs on the caller's thread on the normal single-use teardown
                // path. A FIN is read by the child as a clean EOF and it exits,
                // so the graceful Close frame buys nothing here.
                self.interrupt.kill();
                None
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

/// Maps a tungstenite error from a *send* onto a `FrameError`.
fn ws_to_frame_error(err: WsError) -> FrameError {
    match err {
        WsError::Io(err) => FrameError::Io(err),
        _ => FrameError::Truncated,
    }
}

/// Fallback dial budget when the pool sets no `request_timeout` (which otherwise
/// also bounds the WebSocket dial). Generous, since it only guards a stuck dial.
const DEFAULT_DIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Dials `url` as a blocking WebSocket client, bounding both the TCP connect and
/// the TLS/WS handshake by `timeout` so a stuck peer cannot hang a checkout. DNS
/// resolution is left to the OS resolver (typically fast); everything after it is
/// time-boxed. Frame/message limits are raised to monty's [`MAX_FRAME_LEN`] so
/// the transport never rejects a frame the protocol itself would accept.
///
/// The handshake's socket read/write timeouts are cleared once connected: during
/// a session, reads block and are interrupted only by the watchdog shutting the
/// socket down, never by a per-read deadline.
fn dial_ws(url: &str, timeout: Duration) -> Result<WsSocket, PoolError> {
    let spawn_err = |msg: String| PoolError::Spawn(format!("{url}: {msg}"));

    let request = url
        .into_client_request()
        .map_err(|err| spawn_err(format!("invalid WebSocket URL: {err}")))?;
    let uri = request.uri();
    let mode = uri_mode(uri).map_err(|err| spawn_err(err.to_string()))?;
    let host = uri.host().ok_or_else(|| spawn_err("URL has no host".to_owned()))?;
    // Strip the brackets from an IPv6 literal host (`[::1]` -> `::1`).
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let port = uri.port_u16().unwrap_or(match mode {
        Mode::Plain => 80,
        Mode::Tls => 443,
    });

    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|err| spawn_err(format!("could not resolve {host}:{port}: {err}")))?;

    // Try each resolved address in turn, bounding the *total* connect time by
    // `timeout` so a list of dead addresses cannot multiply the budget.
    let deadline = Instant::now() + timeout;
    let mut stream = None;
    let mut last_err = None;
    for addr in addrs {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&addr, remaining) {
            Ok(tcp) => {
                stream = Some(tcp);
                break;
            }
            Err(err) => last_err = Some(err),
        }
    }
    let stream = stream.ok_or_else(|| {
        spawn_err(match last_err {
            Some(err) => format!("connect failed: {err}"),
            None => "connect timed out".to_owned(),
        })
    })?;
    let _ = stream.set_nodelay(true);
    // Time-box the handshake I/O too, else a peer that completes the TCP connect
    // but stalls the TLS/WS handshake would hang the dial indefinitely.
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let ws_config = WebSocketConfig::default()
        .max_frame_size(Some(MAX_FRAME_LEN as usize))
        .max_message_size(Some(MAX_FRAME_LEN as usize));
    let (socket, _response) = client_tls_with_config(request, stream, Some(ws_config), None)
        .map_err(|err| spawn_err(format!("handshake failed: {err}")))?;

    // Restore blocking reads for the session (see the fn doc).
    if let Some(tcp) = underlying_tcp(socket.get_ref()) {
        let _ = tcp.set_read_timeout(None);
        let _ = tcp.set_write_timeout(None);
    }
    Ok(socket)
}

/// Reaches the raw `TcpStream` behind a (possibly TLS-wrapped) WebSocket stream,
/// so it can be cloned for the watchdog's shutdown handle. Returns `None` for an
/// unknown stream variant; [`Worker::websocket`] treats that as a dial failure
/// (a worker the watchdog can't interrupt is worse than none).
fn underlying_tcp(stream: &MaybeTlsStream<TcpStream>) -> Option<&TcpStream> {
    match stream {
        MaybeTlsStream::Plain(tcp) => Some(tcp),
        MaybeTlsStream::Rustls(tls) => Some(tls.get_ref()),
        _ => None,
    }
}

/// Installs the process-level rustls `CryptoProvider` exactly once before the
/// first `wss://` dial. rustls 0.23 panics on first TLS use when it can't pick a
/// provider automatically (both `aws-lc-rs` and `ring`, or neither, compiled
/// in), so we name `aws_lc_rs` explicitly. Idempotent via `Once`, and the
/// install error is ignored: another part of the process (e.g. a host embedding
/// the pool) may have already installed a provider, which is fine.
fn install_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = default_provider().install_default();
    });
}

/// Locks a possibly poisoned mutex; a panic elsewhere must not stop us from
/// killing/reaping children.
pub(crate) fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Shared kill channel between a worker and the watchdog thread.
///
/// Bundles the transport-specific way to interrupt the owner's blocked read
/// with the sticky flag that lets the owner classify the resulting I/O failure
/// as a timeout rather than a crash. The two always travel together — the
/// watchdog sets the flag *immediately before* killing — so they live in one
/// `Arc` the worker creates and the watchdog clones when it arms a deadline. A
/// plain `enum` (the transport set is closed) keeps the watchdog
/// transport-agnostic without dynamic dispatch or a per-arm allocation.
pub(crate) struct Interrupt {
    kind: InterruptKind,
    /// Set by the watchdog right before it kills; read by the owner to tell a
    /// timeout-kill from a crash. Sticky: the checkout resets it per turn so a
    /// previous turn's kill cannot misclassify this one.
    killed_for_timeout: AtomicBool,
}

/// Transport-specific way to unblock a worker's blocked read from another
/// thread: kill the child process, or shut down the socket under it.
enum InterruptKind {
    /// Kill the child process.
    Subprocess(Arc<Mutex<Child>>),
    /// The raw TCP socket under the (possibly TLS-wrapped) WebSocket stream;
    /// `shutdown(Both)` surfaces an I/O error in the owner's blocked `read`.
    /// A WebSocket worker that can't expose this socket cannot be interrupted,
    /// so the dial refuses to build one (see [`Worker::websocket`]) — the
    /// socket is therefore always present here.
    WebSocket(TcpStream),
}

impl Interrupt {
    fn new(kind: InterruptKind) -> Arc<Self> {
        Arc::new(Self {
            kind,
            killed_for_timeout: AtomicBool::new(false),
        })
    }

    /// Interrupts the worker's blocked read. Best-effort and idempotent: any
    /// failure is ignored because the worker is being discarded regardless.
    pub(crate) fn kill(&self) {
        match &self.kind {
            InterruptKind::Subprocess(child) => {
                let _ = lock_ignore_poison(child).kill();
            }
            InterruptKind::WebSocket(tcp) => {
                let _ = tcp.shutdown(Shutdown::Both);
            }
        }
    }

    /// Flags the imminent kill as a deadline timeout. The watchdog MUST call
    /// this *before* [`Interrupt::kill`] so the owner's failed read always
    /// observes the flag.
    pub(crate) fn flag_timeout(&self) {
        self.killed_for_timeout.store(true, Ordering::SeqCst);
    }

    fn was_killed_for_timeout(&self) -> bool {
        self.killed_for_timeout.load(Ordering::SeqCst)
    }

    fn reset_killed_for_timeout(&self) {
        self.killed_for_timeout.store(false, Ordering::SeqCst);
    }
}
