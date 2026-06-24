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
    net::{Shutdown, TcpStream},
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use monty_proto::{FrameError, FrameReader, decode_frame, encode_to_capped_vec, pb, write_frame};
use tungstenite::{Error as WsError, Message, WebSocket, stream::MaybeTlsStream};
use uuid::Uuid;

use crate::{MontyTransport, PoolConfig, PoolError};

/// The synchronous WebSocket client socket type for a remote worker.
type WsSocket = WebSocket<MaybeTlsStream<TcpStream>>;

/// Something the watchdog can kill from another thread while the owning thread
/// is blocked in a read. A boxed `dyn Killable` lets the watchdog stay ignorant
/// of the transport (kill a process vs. shut down a socket).
pub(crate) trait Killable: Send + Sync {
    fn kill(&self);
}

/// A worker plus its recycle counter. The transport-specific state lives in
/// [`WorkerKind`]; `checkouts_served` is shared logic.
pub(crate) struct Worker {
    kind: WorkerKind,
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
    child: Arc<Mutex<Child>>,
    writer: ChildStdin,
    reader: FrameReader<ChildStdout>,
    killed_for_timeout: Arc<AtomicBool>,
}

/// A remote child reached over a WebSocket. One binary message per protocol
/// frame (no length prefix — the message boundary is the frame).
struct WebSocketWorker {
    socket: WsSocket,
    /// A clone of the underlying TCP socket, handed to the watchdog so it can
    /// `shutdown(Both)` — and thereby unblock a blocked `read` — *without*
    /// taking the `socket` the owner holds. This is the WebSocket analogue of
    /// killing the subprocess child via its separate `Arc<Mutex<Child>>`.
    shutdown: Arc<WsShutdown>,
    killed_for_timeout: Arc<AtomicBool>,
    /// Set once the connection is closed/killed, so `is_dead` reports it.
    closed: Arc<AtomicBool>,
}

/// Kill handle for a subprocess worker: kills the child process.
struct SubprocessKill(Arc<Mutex<Child>>);

impl Killable for SubprocessKill {
    fn kill(&self) {
        let _ = lock_ignore_poison(&self.0).kill();
    }
}

/// Kill handle for a WebSocket worker: shuts down the underlying TCP socket,
/// which unblocks a blocked `read` with an I/O error.
struct WsShutdown {
    tcp: Option<TcpStream>,
}

impl Killable for WsShutdown {
    fn kill(&self) {
        if let Some(tcp) = &self.tcp {
            let _ = tcp.shutdown(Shutdown::Both);
        }
    }
}

impl Worker {
    /// Spawns a local `monty subprocess` child with framed pipes.
    ///
    /// There is no spawn-time handshake: a wrong or broken binary surfaces as
    /// an error on the first request the worker serves (typically the
    /// `Configure` of its first checkout).
    pub(crate) fn spawn(config: &PoolConfig) -> Result<Self, PoolError> {
        let MontyTransport::Subprocess { binary_path } = &config.transport else {
            return Err(PoolError::Spawn(
                "internal error: spawn called for a non-subprocess transport".to_owned(),
            ));
        };
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
        Ok(Self::new(WorkerKind::Subprocess(SubprocessWorker {
            child: Arc::new(Mutex::new(child)),
            writer,
            reader,
            killed_for_timeout: Arc::new(AtomicBool::new(false)),
        })))
    }

    /// Connects to a remote child over a WebSocket. `session_id` is appended to
    /// the URL when `append_session_id` is set, so a relay can pair this
    /// connection with the child that dialed in with the same id.
    pub(crate) fn connect_ws(config: &PoolConfig, session_id: Uuid) -> Result<Self, PoolError> {
        let MontyTransport::Websocket { url, append_session_id } = &config.transport else {
            return Err(PoolError::Spawn(
                "internal error: connect_ws called for a non-websocket transport".to_owned(),
            ));
        };
        let target = if *append_session_id {
            format!("{}/{session_id}", url.trim_end_matches('/'))
        } else {
            url.clone()
        };
        let (socket, _response) =
            tungstenite::connect(&target).map_err(|err| PoolError::Spawn(format!("{target}: {err}")))?;
        // Clone the underlying TCP socket up front for the watchdog's shutdown
        // handle (reaching it through the TLS stream once connected).
        let tcp = underlying_tcp(socket.get_ref()).and_then(|tcp| tcp.try_clone().ok());
        Ok(Self::new(WorkerKind::WebSocket(Box::new(WebSocketWorker {
            socket,
            shutdown: Arc::new(WsShutdown { tcp }),
            killed_for_timeout: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        }))))
    }

    fn new(kind: WorkerKind) -> Self {
        Self {
            kind,
            checkouts_served: 0,
        }
    }

    pub(crate) fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => write_frame(&mut w.writer, request),
            WorkerKind::WebSocket(w) => ws_send(&mut w.socket, request),
        }
    }

    /// Reads one event; EOF/close is an error here because within a checkout the
    /// child must never close its side first.
    pub(crate) fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => w.reader.read::<pb::ChildEvent>()?.ok_or(FrameError::Truncated),
            WorkerKind::WebSocket(w) => ws_recv(&mut w.socket),
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

    /// Watchdog handles: the kill target and the timeout flag.
    pub(crate) fn kill_handles(&self) -> (Arc<dyn Killable>, Arc<AtomicBool>) {
        match &self.kind {
            WorkerKind::Subprocess(w) => (
                Arc::new(SubprocessKill(Arc::clone(&w.child))),
                Arc::clone(&w.killed_for_timeout),
            ),
            WorkerKind::WebSocket(w) => (
                Arc::clone(&w.shutdown) as Arc<dyn Killable>,
                Arc::clone(&w.killed_for_timeout),
            ),
        }
    }

    /// Whether the watchdog killed this worker (consumes the flag's meaning:
    /// call once when classifying a read failure).
    pub(crate) fn was_killed_for_timeout(&self) -> bool {
        self.killed_for_timeout().load(Ordering::SeqCst)
    }

    /// Clears the sticky timeout flag at the start of a turn, scoping it to the
    /// currently-armed deadline. The watchdog sets the flag but never clears it,
    /// so without this reset a stale kill could misclassify the next turn's
    /// first I/O failure as a timeout.
    pub(crate) fn reset_killed_for_timeout(&self) {
        self.killed_for_timeout().store(false, Ordering::SeqCst);
    }

    fn killed_for_timeout(&self) -> &Arc<AtomicBool> {
        match &self.kind {
            WorkerKind::Subprocess(w) => &w.killed_for_timeout,
            WorkerKind::WebSocket(w) => &w.killed_for_timeout,
        }
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
                let _ = w.socket.close(None);
                w.shutdown.kill();
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

/// Sends one event as a single binary WebSocket message (no length prefix), and
/// flushes — the protocol is strict alternation, so the frame must hit the wire.
fn ws_send(socket: &mut WsSocket, request: &pb::ParentRequest) -> Result<(), FrameError> {
    let body = encode_to_capped_vec(request)?;
    socket.write(Message::Binary(body.into())).map_err(ws_to_frame_error)?;
    socket.flush().map_err(ws_to_frame_error)?;
    Ok(())
}

/// Reads one `ChildEvent` from the WebSocket, skipping control frames. A
/// close/EOF *without* a prior turn-ender means the child died — surfaced as
/// [`FrameError::Truncated`], mirroring the stdio crash contract.
fn ws_recv(socket: &mut WsSocket) -> Result<pb::ChildEvent, FrameError> {
    loop {
        match socket.read() {
            Ok(Message::Binary(data)) => return decode_frame::<pb::ChildEvent>(data.as_ref()),
            // tungstenite auto-queues the Pong; flush it and keep reading.
            Ok(Message::Ping(_)) => {
                let _ = socket.flush();
            }
            Ok(Message::Pong(_)) => {}
            // A clean close, or text/raw frames the protocol never uses.
            Ok(Message::Close(_) | Message::Text(_) | Message::Frame(_)) => return Err(FrameError::Truncated),
            Err(WsError::Io(err)) => return Err(FrameError::Io(err)),
            Err(_) => return Err(FrameError::Truncated),
        }
    }
}

/// Maps a tungstenite error from a *send* onto a `FrameError`.
fn ws_to_frame_error(err: WsError) -> FrameError {
    match err {
        WsError::Io(err) => FrameError::Io(err),
        _ => FrameError::Truncated,
    }
}

/// Reaches the raw `TcpStream` behind a (possibly TLS-wrapped) WebSocket stream,
/// so it can be cloned for the watchdog's shutdown handle. Returns `None` for an
/// unknown stream variant (the watchdog then cannot interrupt a blocked read).
fn underlying_tcp(stream: &MaybeTlsStream<TcpStream>) -> Option<&TcpStream> {
    match stream {
        MaybeTlsStream::Plain(tcp) => Some(tcp),
        MaybeTlsStream::Rustls(tls) => Some(tls.get_ref()),
        _ => None,
    }
}

/// Locks a possibly poisoned mutex; a panic elsewhere must not stop us from
/// killing/reaping children.
pub(crate) fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
