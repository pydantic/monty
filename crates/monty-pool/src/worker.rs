//! A single worker the pool drives over the wire protocol: either a local
//! `monty subprocess` child (framed stdio) or a remote child reached over a
//! WebSocket. Both expose the same async send/recv/kill surface so the
//! checkout turn loop is transport-agnostic.
//!
//! Reads are pumped by a dedicated task per worker: async frame reads are not
//! cancel-safe (dropping a read future mid-frame loses bytes and desyncs the
//! stream), so the pump owns the read half and forwards whole decoded events
//! over a bounded channel. The consumer's `recv` — a channel read — *is*
//! cancel-safe, which is what lets the checkout race a turn against a
//! `tokio::time` deadline without corrupting the stream it may yet reuse.

use std::{
    env, io,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::Once,
    time::Duration,
};

use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use monty_proto::{FrameError, MAX_FRAME_LEN, decode_frame, encode_to_capped_vec, pb};
use rustls::crypto::aws_lc_rs::default_provider;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Error as WsError, Message, protocol::WebSocketConfig},
};

use crate::{MontyTransport, PoolConfig, PoolError};

/// The async WebSocket stream type for a remote worker.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How many decoded child events the pump may buffer ahead of the consumer.
///
/// Small on purpose: the pump blocks once it is full, so a print flood is
/// backpressured through the pipe / socket exactly as it was when reads were
/// synchronous, instead of ballooning host memory.
const EVENT_CHANNEL_CAPACITY: usize = 16;

/// A worker plus its recycle counter. The transport-specific *write* half
/// lives in [`WorkerKind`]; reads arrive through `events`, fed by the pump
/// task that owns the read half.
pub(crate) struct Worker {
    kind: WorkerKind,
    /// Decoded events from the pump task. A closed channel means the pump
    /// ended: the peer reached EOF/close, or the read stream broke.
    events: mpsc::Receiver<Result<pb::ChildEvent, FrameError>>,
    /// The pump task, aborted on kill/drop so the read half is released
    /// promptly rather than lingering until the peer closes.
    pump: JoinHandle<()>,
    /// Checkouts this worker has served, for `max_checkouts_per_worker`.
    pub(crate) checkouts_served: u32,
}

/// The two transports a worker can speak the protocol over (write halves).
enum WorkerKind {
    Subprocess(SubprocessWorker),
    // Boxed: the WebSocket sink (with its TLS state and buffers) is far larger
    // than the subprocess handle, so inlining it would bloat every `Worker`.
    WebSocket(Box<WebSocketWorker>),
}

/// A local `monty subprocess` child: the process handle (kill-on-drop) and its
/// framed stdin. Stdout lives in the pump task.
struct SubprocessWorker {
    child: Child,
    stdin: ChildStdin,
}

/// A remote child reached over a WebSocket: the write half of the split
/// stream. One binary message per protocol frame (no length prefix — the
/// message boundary is the frame). `Option` so teardown can drop the sink in
/// place: with the pump aborted (read half gone), dropping it closes the
/// connection — the async analogue of killing a child.
struct WebSocketWorker {
    sink: Option<SplitSink<WsStream, Message>>,
}

impl Worker {
    pub(crate) async fn new(config: &PoolConfig) -> Result<Self, PoolError> {
        match &config.transport {
            MontyTransport::Subprocess(binary_path) => Self::subprocess(binary_path),
            // Bound the dial by `request_timeout` (see `websocket`); a missing
            // one falls back to a generous fixed budget.
            MontyTransport::Websocket(url) => {
                Self::websocket(url, config.request_timeout.unwrap_or(DEFAULT_DIAL_TIMEOUT)).await
            }
        }
    }

    /// Spawns a local `monty subprocess` child with framed pipes and starts
    /// its frame pump.
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
            .stdout(Stdio::piped())
            // the pool must never leak a live sandbox: an abandoned handle
            // kills the child even when no explicit teardown ran
            .kill_on_drop(true);
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

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, events) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let pump = tokio::spawn(pump_frames(stdout, tx));
        Ok(Self {
            kind: WorkerKind::Subprocess(SubprocessWorker { child, stdin }),
            events,
            pump,
            checkouts_served: 0,
        })
    }

    /// Connects to a remote child over a WebSocket, dialing `url` verbatim. Any
    /// session/rendezvous routing the URL needs is the caller's responsibility.
    ///
    /// `dial_timeout` bounds the whole dial (DNS + TCP connect + TLS/WS
    /// handshake): `checkout_timeout` only covers waiting for capacity, so a
    /// hung dial would otherwise stall the checkout forever. Frame/message
    /// limits are raised to monty's [`MAX_FRAME_LEN`] so the transport never
    /// rejects a frame the protocol itself would accept.
    async fn websocket(url: &str, dial_timeout: Duration) -> Result<Self, PoolError> {
        install_crypto_provider();
        let ws_config = WebSocketConfig::default()
            .max_frame_size(Some(MAX_FRAME_LEN as usize))
            .max_message_size(Some(MAX_FRAME_LEN as usize));
        let dial = connect_async_tls_with_config(url, Some(ws_config), true, None);
        let (stream, _response) = timeout(dial_timeout, dial)
            .await
            .map_err(|_| PoolError::Spawn(format!("{url}: connect timed out after {dial_timeout:?}")))?
            .map_err(|err| PoolError::Spawn(format!("{url}: {err}")))?;
        let (sink, stream) = stream.split();
        let (tx, events) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let pump = tokio::spawn(pump_ws(stream, tx));
        Ok(Self {
            kind: WorkerKind::WebSocket(Box::new(WebSocketWorker { sink: Some(sink) })),
            events,
            pump,
            checkouts_served: 0,
        })
    }

    /// Sends one request, flushed to the wire — the protocol is strict
    /// alternation, so an unflushed frame would deadlock both sides. An
    /// oversize frame is rejected *before* any I/O so the stream stays synced
    /// (see `Checkout::request_turn`).
    pub(crate) async fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        let body = encode_to_capped_vec(request)?;
        match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                let len = u32::try_from(body.len()).expect("capped by encode_to_capped_vec");
                w.stdin.write_all(&len.to_le_bytes()).await?;
                w.stdin.write_all(&body).await?;
                w.stdin.flush().await?;
                Ok(())
            }
            WorkerKind::WebSocket(w) => match &mut w.sink {
                Some(sink) => sink.send(Message::Binary(body.into())).await.map_err(ws_to_frame_error),
                None => Err(FrameError::Truncated),
            },
        }
    }

    /// Receives one event from the pump. EOF/close is an error here because
    /// within a checkout the child must never close its side first.
    ///
    /// Cancel-safe (it is a channel read; partial frames never leave the pump),
    /// which is what lets `Checkout` race a turn against its deadline.
    pub(crate) async fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        match self.events.recv().await {
            Some(result) => result,
            None => Err(FrameError::Truncated),
        }
    }

    /// The OS process id, when the worker is a local subprocess (`None` for a
    /// remote WebSocket worker, or once the child has been reaped).
    pub(crate) fn pid(&self) -> Option<u32> {
        match &self.kind {
            WorkerKind::Subprocess(w) => w.child.id(),
            WorkerKind::WebSocket(_) => None,
        }
    }

    /// Whether the worker has already died (used to discard workers that died
    /// while idle in the pool). WebSocket workers are never pooled idle, so
    /// they always report alive here.
    pub(crate) fn is_dead(&mut self) -> bool {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => w.child.try_wait().is_ok_and(|status| status.is_some()),
            WorkerKind::WebSocket(_) => false,
        }
    }

    /// Reaps a child that is already on its way out, falling back to
    /// [`Self::kill_and_reap`] if it has not gone within `grace`.
    ///
    /// For deaths the child *announces* before exiting — a `FatalError`, where
    /// it deliberately sets an exit code — killing it immediately would race
    /// its own exit and report the signal instead of that code, losing the one
    /// diagnostic the announcement was made to carry.
    pub(crate) async fn reap_or_kill(&mut self, grace: Duration) -> Option<ExitStatus> {
        if let WorkerKind::Subprocess(w) = &mut self.kind
            && let Ok(Ok(status)) = timeout(grace, w.child.wait()).await
        {
            self.pump.abort();
            return Some(status);
        }
        self.kill_and_reap().await
    }

    /// Tears the worker down (kills the child / closes the connection) and
    /// reaps it, returning the process exit status when there is one.
    pub(crate) async fn kill_and_reap(&mut self) -> Option<ExitStatus> {
        // aborting the pump drops the read half; for a WebSocket that is one
        // of the two halves keeping the connection open
        self.pump.abort();
        match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                let _ = w.child.start_kill();
                w.child.wait().await.ok()
            }
            WorkerKind::WebSocket(w) => {
                // Drop the write half rather than sending a WebSocket Close
                // frame: a peer that has stopped draining could block the
                // close write indefinitely. With both halves gone the TCP
                // socket closes; the child reads that as a clean EOF and
                // exits, so the graceful Close frame buys nothing here.
                w.sink = None;
                None
            }
        }
    }
}

impl Drop for Worker {
    /// Synchronous backstop teardown: the pump is aborted (releasing the read
    /// half) and a subprocess child is killed by its `kill_on_drop` handle,
    /// with tokio reaping it in the background. Paths that need the exit
    /// status call [`Worker::kill_and_reap`] before dropping.
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// Pumps length-prefixed frames from a subprocess's stdout into the event
/// channel until EOF, a framing error, or the consumer going away. Exactly one
/// terminal `Err` is forwarded so the consumer can classify the breakage; a
/// clean EOF just closes the channel.
async fn pump_frames(stdout: ChildStdout, tx: mpsc::Sender<Result<pb::ChildEvent, FrameError>>) {
    let mut stdout = stdout;
    loop {
        match read_frame(&mut stdout).await {
            Ok(Some(event)) => {
                if tx.send(Ok(event)).await.is_err() {
                    return; // worker dropped; nobody is listening
                }
            }
            Ok(None) => return, // clean EOF at a frame boundary
            Err(err) => {
                let _ = tx.send(Err(err)).await;
                return;
            }
        }
    }
}

/// Reads one length-prefixed frame, enforcing [`MAX_FRAME_LEN`] before
/// allocating. `Ok(None)` on EOF at (or within) the length prefix — the peer
/// closed between messages; EOF inside the body is [`FrameError::Truncated`].
async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Option<pb::ChildEvent>, FrameError> {
    let mut len_bytes = [0u8; 4];
    match reader.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(FrameError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    // Allocation is up front but bounded by MAX_FRAME_LEN (256 MiB), keeping
    // byzantine peers bounded to one frame buffer per worker.
    let mut body = vec![0u8; len as usize];
    match reader.read_exact(&mut body).await {
        Ok(_) => {}
        // EOF after a length prefix is always mid-frame.
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Truncated),
        Err(err) => return Err(err.into()),
    }
    // `decode_frame` resets the per-frame decode budget; decoding is fully
    // synchronous, so the thread-local budget cannot interleave with another
    // pump task's frame.
    decode_frame::<pb::ChildEvent>(&body).map(Some)
}

/// Pumps WebSocket messages into the event channel: one binary message per
/// protocol frame. Ping/Pong are handled by tokio-tungstenite itself and
/// skipped here; a close, EOF, or any frame the protocol never uses ends the
/// pump (surfaced to the consumer as [`FrameError::Truncated`], which the
/// checkout classifies as [`crate::PoolError::Disconnected`]).
async fn pump_ws(mut stream: SplitStream<WsStream>, tx: mpsc::Sender<Result<pb::ChildEvent, FrameError>>) {
    loop {
        let forward = match stream.next().await {
            Some(Ok(Message::Binary(data))) => decode_frame::<pb::ChildEvent>(data.as_ref()),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            // A clean close, or text/raw frames the protocol never uses.
            Some(Ok(Message::Close(_) | Message::Text(_) | Message::Frame(_))) | None => return,
            Some(Err(WsError::Io(err))) => Err(FrameError::Io(err)),
            Some(Err(_)) => Err(FrameError::Truncated),
        };
        let stop = forward.is_err();
        if tx.send(forward).await.is_err() || stop {
            return;
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

/// Fallback dial budget when the pool sets no `request_timeout` (which otherwise
/// also bounds the WebSocket dial). Generous, since it only guards a stuck dial.
const DEFAULT_DIAL_TIMEOUT: Duration = Duration::from_secs(30);

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
