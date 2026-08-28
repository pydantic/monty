//! A single worker the pool drives over the wire protocol: either a local
//! `monty subprocess` child (framed stdio) or a remote child reached over a
//! WebSocket. Both expose the same async send/recv/kill surface so the
//! checkout turn loop is transport-agnostic.
//!
//! `recv` must be cancel-safe: the checkout races each turn against a
//! `tokio::time` deadline, and dropping a plain `read_exact` future mid-frame
//! would lose bytes and desync the stream. For the subprocess transport,
//! rather than paying for a pump task and channel per worker (a cross-task
//! wakeup per event), cancel-safety comes from keeping the partial-frame
//! state *in the worker*: [`FrameRecv`] holds the buffer and fill offset
//! across polls, so a dropped `recv` future loses nothing and the next call
//! resumes exactly where it stopped.
//!
//! The WebSocket transport runs a background reader task instead — not for
//! cancel-safety but for liveness: tokio-tungstenite pongs only while the
//! stream is polled, and an idle client polls nothing, so a keepalive server
//! would drop the session as vanished. `recv` reads the task's channel, which
//! is cancel-safe.

use std::{
    env,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::Once,
    time::Duration,
};

use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use monty_proto::{FrameError, MAX_FRAME_LEN, decode_frame, encode_framed_into, encode_to_capped_vec, pb};
use rustls::crypto::aws_lc_rs::default_provider;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::{Handle, RuntimeFlavor},
    sync::mpsc,
    task::{JoinHandle, block_in_place},
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Bytes, Error as WsError, Message, protocol::WebSocketConfig},
};

use crate::{MontyTransport, PoolConfig, PoolError};
#[cfg(feature = "telemetry-adapter")]
use crate::{telemetry::Recorder, telemetry_adapter::TelemetryContext};

/// The async WebSocket stream type for a remote worker.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A worker plus its recycle counter. The transport-specific I/O halves live
/// in [`WorkerKind`]; both transports keep any partial-read state inside the
/// worker so `recv` is cancel-safe (see the module docs).
pub(crate) struct Worker {
    kind: WorkerKind,
    /// Checkouts this worker has served, for `max_checkouts_per_worker`.
    pub(crate) checkouts_served: u32,
    /// Records an adapter checkout's protocol turns, created only while needed.
    #[cfg(feature = "telemetry-adapter")]
    recorder: Option<Box<Recorder>>,
}

/// The two transports a worker can speak the protocol over. Both variants are
/// boxed: the WebSocket stream (TLS state and buffers) and the subprocess
/// handles + frame-read state are each far larger than a pointer, and workers
/// move through the idle queue by value.
enum WorkerKind {
    Subprocess(Box<SubprocessWorker>),
    WebSocket(Box<WebSocketWorker>),
}

/// A local `monty subprocess` child: the process handle (kill-on-drop), its
/// framed stdin, and the cancel-safe stdout frame reader.
struct SubprocessWorker {
    child: Child,
    stdin: ChildStdin,
    recv: FrameRecv,
    /// Reused encode buffer for outgoing frames (see [`Worker::send`]).
    send_buf: Vec<u8>,
}

/// A remote child reached over a WebSocket. One binary message per protocol
/// frame (no length prefix — the message boundary is the frame). The read half
/// lives in a spawned reader task (see the module docs), so teardown must
/// release both halves — see [`WebSocketWorker::close`].
struct WebSocketWorker {
    /// `send()` writes here; `None` after teardown (the closed-connection sentinel).
    sink: Option<SplitSink<WsStream, Message>>,
    /// Binary frames (or the terminal error) forwarded by the reader task.
    events: mpsc::Receiver<Result<Bytes, FrameError>>,
    /// The task owning the read half; aborted on teardown/drop.
    reader: JoinHandle<()>,
}

impl WebSocketWorker {
    /// Ends the connection: a bounded Close frame, then abort the reader and
    /// drop the sink — releasing both halves is what closes the socket.
    ///
    /// The frame is what a *proxied* peer sees: some load balancers forward a
    /// Close but never a bare TCP disconnect, leaving the session (and its
    /// capacity slot) alive until the far side's own idle timeout. Bounded so
    /// a peer that stopped draining cannot hang teardown.
    async fn close(&mut self) {
        if let Some(sink) = &mut self.sink {
            send_close(sink).await;
        }
        self.reader.abort();
        self.sink = None;
    }
}

/// Backstop for the synchronous teardown paths — an abandoned
/// [`crate::Checkout`], a discarded worker — where `close` cannot be awaited:
/// a detached task says goodbye instead. Best-effort, since a runtime shutting
/// down cancels it; it then degrades to the plain drop this replaced.
///
/// The reader abort is unconditional: on a quiet connection the task never
/// notices the worker is gone, and would hold the socket open forever.
impl Drop for WebSocketWorker {
    fn drop(&mut self) {
        self.reader.abort();
        if let Some(mut sink) = self.sink.take()
            && let Ok(handle) = Handle::try_current()
        {
            handle.spawn(async move { send_close(&mut sink).await });
        }
    }
}

/// Writes the goodbye frame, shared by both paths above so they cannot drift.
async fn send_close(sink: &mut SplitSink<WsStream, Message>) {
    let _ = timeout(CLOSE_WRITE_TIMEOUT, sink.close()).await;
}

impl Worker {
    /// Creates a worker for `config`'s transport.
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
        Ok(Self {
            kind: WorkerKind::Subprocess(Box::new(SubprocessWorker {
                child,
                stdin,
                recv: FrameRecv::new(stdout),
                send_buf: Vec::with_capacity(SEND_BUF_CAPACITY),
            })),
            checkouts_served: 0,
            #[cfg(feature = "telemetry-adapter")]
            recorder: None,
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
        // split so a background task can poll (and thereby pong) the read half
        let (sink, read_half) = stream.split();
        let (tx, events) = mpsc::channel(WS_EVENT_CHANNEL_DEPTH);
        let reader = tokio::spawn(read_ws_events(read_half, tx));
        Ok(Self {
            kind: WorkerKind::WebSocket(Box::new(WebSocketWorker {
                sink: Some(sink),
                events,
                reader,
            })),
            checkouts_served: 0,
            #[cfg(feature = "telemetry-adapter")]
            recorder: None,
        })
    }

    /// Sends one request, flushed to the wire — the protocol is strict
    /// alternation, so an unflushed frame would deadlock both sides. An
    /// oversize frame is rejected *before* any I/O so the stream stays synced
    /// (see `Checkout::request_turn`).
    pub(crate) async fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        let result = match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                // prefix + body in one reused buffer: a single write syscall
                // per request, no per-frame allocation, and a pipe write needs
                // no flush
                encode_framed_into(request, &mut w.send_buf)?;
                w.stdin.write_all(&w.send_buf).await?;
                // don't let one huge frame pin its capacity for the worker's life
                if w.send_buf.capacity() > RETAIN_BUF_MAX {
                    w.send_buf = Vec::with_capacity(SEND_BUF_CAPACITY);
                }
                Ok(())
            }
            WorkerKind::WebSocket(w) => {
                let body = encode_to_capped_vec(request)?;
                match &mut w.sink {
                    Some(sink) => sink.send(Message::Binary(body.into())).await.map_err(ws_to_frame_error),
                    None => Err(FrameError::Truncated),
                }
            }
        };
        // record only requests that hit the wire: a rejected oversize frame
        // leaves the turn (and any pending suspension) exactly where it was
        #[cfg(feature = "telemetry-adapter")]
        if result.is_ok() {
            if let Some(recorder) = &mut self.recorder {
                recorder.begin_turn(request);
            }
            if matches!(
                request.kind,
                Some(pb::parent_request::Kind::Reset(_) | pb::parent_request::Kind::Shutdown(_))
            ) {
                self.recorder = None;
            }
        }
        result
    }

    /// Assigns propagated host context before this worker starts a checkout.
    #[cfg(feature = "telemetry-adapter")]
    pub(crate) fn set_adapter_context(&mut self, context: TelemetryContext) {
        let worker_pid = self.pid();
        self.recorder
            .get_or_insert_with(|| Box::new(Recorder::new(worker_pid)))
            .set_adapter_context(context);
    }

    /// Receives one event. EOF/close is an error here because within a
    /// checkout the child must never close its side first.
    ///
    /// Cancel-safe: partial-frame state persists in the worker (see the
    /// module docs), which is what lets `Checkout` race a turn against its
    /// deadline.
    pub(crate) async fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        let event = match &mut self.kind {
            WorkerKind::Subprocess(w) => match w.recv.recv().await? {
                Some(event) => Ok(event),
                None => Err(FrameError::Truncated), // clean EOF mid-checkout is still a vanished peer
            },
            WorkerKind::WebSocket(w) => {
                // frames come from the reader task; a channel recv is
                // cancel-safe, preserving the deadline race. A torn-down worker
                // or closed channel is `Truncated` — a `PoolError::Disconnected`.
                if w.sink.is_none() {
                    return Err(FrameError::Truncated);
                }
                match w.events.recv().await {
                    Some(Ok(data)) => decode_event(&data),
                    Some(Err(err)) => Err(err),
                    None => Err(FrameError::Truncated),
                }
            }
        }?;
        // only after a complete decode, so a `recv` future dropped mid-frame
        // (deadline race) records nothing — cancel-safety intact
        #[cfg(feature = "telemetry-adapter")]
        if let Some(recorder) = &mut self.recorder {
            recorder.event(&event);
        }
        Ok(event)
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
            return Some(status);
        }
        self.kill_and_reap().await
    }

    /// Tears the worker down (kills the child / closes the connection) and
    /// reaps it, returning the process exit status when there is one.
    pub(crate) async fn kill_and_reap(&mut self) -> Option<ExitStatus> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                let _ = w.child.start_kill();
                w.child.wait().await.ok()
            }
            WorkerKind::WebSocket(w) => {
                w.close().await;
                None
            }
        }
    }

    /// Ends the connection behind a worker the caller is *not* discarding as
    /// dead — a cleanly finished checkout's single-use WebSocket worker. A
    /// no-op for subprocess workers, which the pool reuses or kills instead.
    pub(crate) async fn close_transport(&mut self) {
        if let WorkerKind::WebSocket(w) = &mut self.kind {
            w.close().await;
        }
    }
}

/// How long teardown gives a WebSocket Close frame to reach the OS. Not a
/// latency budget (no ACK or Close echo is awaited) — it only rides out
/// back-pressure from a peer that stopped reading.
const CLOSE_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Chunk size for speculative frame reads: large enough that a typical
/// prefix + body arrives in a single `read` syscall, small enough to sit
/// permanently in every idle worker.
const READ_CHUNK: usize = 8 * 1024;

/// Cap above which the reused send/recv buffers are dropped after use, so one
/// huge frame does not pin megabytes for the worker's lifetime.
const RETAIN_BUF_MAX: usize = 64 * 1024;

/// Initial capacity of the reused send buffer: covers control frames,
/// resumes with small values, and `Configure` (~150 bytes) without a realloc;
/// only large feeds/values grow past it (via `encode_framed_into`'s reserve).
const SEND_BUF_CAPACITY: usize = 1024;

/// Frames at or above this size decode under `block_in_place` (see
/// [`decode_event`]); below it (< ~4ms) inline decode beats the handoff.
const DECODE_OFFLOAD_MIN: usize = 1024 * 1024;

/// A cancel-safe reader of length-prefixed frames from the child's stdout.
///
/// Async `read_exact` is not cancel-safe — dropping it mid-frame loses the
/// bytes already read. Here the accumulation buffer and fill offset are
/// fields, and each await is a single `read` (which never consumes bytes when
/// it returns `Pending`), so a `recv` future dropped at any await point
/// leaves the accumulated bytes in place for the next call to resume from.
///
/// Reads accumulate into one buffer rather than prefix-then-body, so the
/// common whole-frame-ready case costs a single `read` syscall; bytes past
/// the frame boundary (coalesced print events) are retained for the next call.
struct FrameRecv {
    stdout: ChildStdout,
    /// Zero-initialized read buffer; `buf[consumed..filled]` is unparsed data.
    buf: Vec<u8>,
    /// Bytes already handed out as decoded frames.
    consumed: usize,
    /// Bytes read from the pipe so far.
    filled: usize,
}

impl FrameRecv {
    fn new(stdout: ChildStdout) -> Self {
        Self {
            stdout,
            buf: Vec::new(),
            consumed: 0,
            filled: 0,
        }
    }

    /// Reads and decodes one frame, enforcing [`MAX_FRAME_LEN`] before growing
    /// the buffer to the announced size. `Ok(None)` on EOF at a frame boundary
    /// — the peer closed between messages; EOF inside a frame is
    /// [`FrameError::Truncated`].
    async fn recv(&mut self) -> Result<Option<pb::ChildEvent>, FrameError> {
        // move leftover bytes from a previous coalesced read to the front so
        // the parse below always works from offset 0
        if self.consumed > 0 {
            self.buf.copy_within(self.consumed..self.filled, 0);
            self.filled -= self.consumed;
            self.consumed = 0;
        }
        loop {
            if self.filled >= 4 {
                let len = u32::from_le_bytes(self.buf[..4].try_into().expect("4 bytes"));
                if len > MAX_FRAME_LEN {
                    return Err(FrameError::FrameTooLarge {
                        len,
                        max: MAX_FRAME_LEN,
                    });
                }
                let end = 4 + len as usize;
                if self.filled >= end {
                    let event = decode_event(&self.buf[4..end])?;
                    self.consumed = end;
                    // a fully drained oversize buffer is dropped rather than
                    // pinned for the worker's lifetime
                    if self.buf.len() > RETAIN_BUF_MAX && self.filled == end {
                        self.buf = Vec::new();
                        self.consumed = 0;
                        self.filled = 0;
                    }
                    return Ok(Some(event));
                }
                // Growth is validated against MAX_FRAME_LEN above, keeping
                // byzantine peers bounded to one frame buffer per worker.
                if self.buf.len() < end {
                    self.buf.resize(end, 0);
                }
            } else if self.buf.len() < READ_CHUNK {
                self.buf.resize(READ_CHUNK, 0);
            }
            let n = self.stdout.read(&mut self.buf[self.filled..]).await?;
            if n == 0 {
                // EOF at a frame boundary is a clean close; mid-frame the
                // peer died while writing.
                return if self.filled == 0 {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            self.filled += n;
        }
    }
}

/// Decodes one complete frame body, synchronously on the current thread.
///
/// A max-size frame can cost ~1s of CPU (see `benches/decode.rs`), so frames
/// ≥ [`DECODE_OFFLOAD_MIN`] decode under `block_in_place`: the decode borrows
/// the bytes (no copy), can't outlive a cancelled turn (unlike
/// `spawn_blocking`), and the runtime keeps its worker count. On a
/// current-thread runtime, where `block_in_place` panics, decode is inline.
fn decode_event(body: &[u8]) -> Result<pb::ChildEvent, FrameError> {
    // each decode is one synchronous call, so the thread-local decode budgets
    // (reset by every `decode_frame`) cannot interleave
    if body.len() < DECODE_OFFLOAD_MIN || Handle::current().runtime_flavor() != RuntimeFlavor::MultiThread {
        decode_frame(body)
    } else {
        block_in_place(|| decode_frame(body))
    }
}

/// Depth of the reader-task → `recv` channel. One slot, reserved before the
/// poll, bounds an untrusted peer to a single queued frame (≤
/// [`MAX_FRAME_LEN`]) per worker — what the unsplit stream held. Deeper buys
/// nothing: pongs need polling, not buffering, and the server's print
/// backpressure assumes unread data sits in the socket, not in client memory.
const WS_EVENT_CHANNEL_DEPTH: usize = 1;

/// The WebSocket reader task: polls the read half continuously so
/// tokio-tungstenite pongs even while the worker is idle (see the module
/// docs), forwarding each binary frame — or the terminal error — to `recv`.
///
/// Reserving the slot before polling keeps the reader polling (and ponging)
/// rather than blocked holding a frame.
async fn read_ws_events(mut read_half: SplitStream<WsStream>, events: mpsc::Sender<Result<Bytes, FrameError>>) {
    // an Err from reserve means the receiver dropped: the worker is gone
    while let Ok(permit) = events.reserve().await {
        let result = loop {
            match read_half.next().await {
                Some(Ok(Message::Binary(data))) => break Ok(data),
                // polling is itself what pongs: tungstenite queues the pong on
                // read and flushes it during read polling
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                // a clean close, or text/raw frames the protocol never uses
                Some(Ok(Message::Close(_) | Message::Text(_) | Message::Frame(_))) | None => {
                    break Err(FrameError::Truncated);
                }
                Some(Err(WsError::Io(err))) => break Err(FrameError::Io(err)),
                Some(Err(_)) => break Err(FrameError::Truncated),
            }
        };
        // any error is terminal: deliver it and exit
        let stop = result.is_err();
        permit.send(result);
        if stop {
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
