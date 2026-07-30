//! A single worker the pool drives over the wire protocol: either a local
//! `monty subprocess` child (framed stdio) or a remote child reached over a
//! WebSocket. Both expose the same async send/recv/kill surface so the
//! checkout turn loop is transport-agnostic.
//!
//! `recv` must be cancel-safe: the checkout races each turn against a
//! `tokio::time` deadline, and dropping a plain `read_exact` future mid-frame
//! would lose bytes and desync the stream. Rather than paying for a pump task
//! and channel per worker (a cross-task wakeup per event), cancel-safety comes
//! from keeping the partial-frame state *in the worker*: [`FrameRecv`] holds
//! the buffer and fill offset across polls, so a dropped `recv` future loses
//! nothing and the next call resumes exactly where it stopped. The WebSocket
//! transport gets the same property for free — partial-message state lives
//! inside the `WebSocketStream`, and `Stream::poll_next` is cancel-safe.

use std::{
    env, mem,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::Once,
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use monty_proto::{FrameError, MAX_FRAME_LEN, decode_frame, encode_framed_vec, encode_to_capped_vec, pb};
use rustls::crypto::aws_lc_rs::default_provider;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Error as WsError, Message, protocol::WebSocketConfig},
};

use crate::{MontyTransport, PoolConfig, PoolError};

/// The async WebSocket stream type for a remote worker.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A worker plus its recycle counter. The transport-specific I/O halves live
/// in [`WorkerKind`]; both transports keep any partial-read state inside the
/// worker so `recv` is cancel-safe (see the module docs).
pub(crate) struct Worker {
    kind: WorkerKind,
    /// Checkouts this worker has served, for `max_checkouts_per_worker`.
    pub(crate) checkouts_served: u32,
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
}

/// A remote child reached over a WebSocket. One binary message per protocol
/// frame (no length prefix — the message boundary is the frame). `Option` so
/// teardown can drop the stream in place: dropping it closes the TCP
/// connection — the async analogue of killing a child.
struct WebSocketWorker {
    stream: Option<WsStream>,
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
            })),
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
        Ok(Self {
            kind: WorkerKind::WebSocket(Box::new(WebSocketWorker { stream: Some(stream) })),
            checkouts_served: 0,
        })
    }

    /// Sends one request, flushed to the wire — the protocol is strict
    /// alternation, so an unflushed frame would deadlock both sides. An
    /// oversize frame is rejected *before* any I/O so the stream stays synced
    /// (see `Checkout::request_turn`).
    pub(crate) async fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                // prefix + body in one buffer: a single write syscall, and a
                // pipe write needs no flush
                let framed = encode_framed_vec(request)?;
                w.stdin.write_all(&framed).await?;
                Ok(())
            }
            WorkerKind::WebSocket(w) => {
                let body = encode_to_capped_vec(request)?;
                match &mut w.stream {
                    Some(stream) => stream
                        .send(Message::Binary(body.into()))
                        .await
                        .map_err(ws_to_frame_error),
                    None => Err(FrameError::Truncated),
                }
            }
        }
    }

    /// Receives one event. EOF/close is an error here because within a
    /// checkout the child must never close its side first.
    ///
    /// Cancel-safe: partial-frame state persists in the worker (see the
    /// module docs), which is what lets `Checkout` race a turn against its
    /// deadline.
    pub(crate) async fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => match w.recv.recv().await? {
                Some(event) => Ok(event),
                None => Err(FrameError::Truncated), // clean EOF mid-checkout is still a vanished peer
            },
            WorkerKind::WebSocket(w) => {
                let Some(stream) = &mut w.stream else {
                    return Err(FrameError::Truncated);
                };
                // Ping/Pong are handled by tokio-tungstenite itself and skipped
                // here; a close, EOF, or any frame the protocol never uses is
                // surfaced as `Truncated`, which the checkout classifies as
                // `PoolError::Disconnected`.
                loop {
                    return match stream.next().await {
                        Some(Ok(Message::Binary(data))) => decode_frame::<pb::ChildEvent>(data.as_ref()),
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                        // A clean close, or text/raw frames the protocol never uses.
                        Some(Ok(Message::Close(_) | Message::Text(_) | Message::Frame(_))) | None => {
                            Err(FrameError::Truncated)
                        }
                        Some(Err(WsError::Io(err))) => Err(FrameError::Io(err)),
                        Some(Err(_)) => Err(FrameError::Truncated),
                    };
                }
            }
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
                // Drop the stream rather than sending a WebSocket Close frame:
                // a peer that has stopped draining could block the close write
                // indefinitely. With the stream gone the TCP socket closes; the
                // child reads that as a clean EOF and exits, so the graceful
                // Close frame buys nothing here.
                w.stream = None;
                None
            }
        }
    }
}

/// A cancel-safe reader of length-prefixed frames from the child's stdout.
///
/// Async `read_exact` is not cancel-safe — dropping it mid-frame loses the
/// bytes already read. Here the buffer and fill offset are fields, and each
/// await is a single `read` (which never consumes bytes when it returns
/// `Pending`), so a `recv` future dropped at any await point leaves the
/// accumulated bytes in place for the next call to resume from.
struct FrameRecv {
    stdout: ChildStdout,
    state: RecvState,
}

/// Read progress on the current frame; survives a cancelled `recv`.
enum RecvState {
    /// Accumulating the 4-byte LE length prefix.
    Len { buf: [u8; 4], filled: usize },
    /// Accumulating a body whose length the prefix announced.
    Body { buf: Vec<u8>, filled: usize },
}

impl FrameRecv {
    fn new(stdout: ChildStdout) -> Self {
        Self {
            stdout,
            state: RecvState::Len { buf: [0; 4], filled: 0 },
        }
    }

    /// Reads and decodes one frame, enforcing [`MAX_FRAME_LEN`] before
    /// allocating the body. `Ok(None)` on EOF at a frame boundary — the peer
    /// closed between messages; EOF inside a frame is [`FrameError::Truncated`].
    async fn recv(&mut self) -> Result<Option<pb::ChildEvent>, FrameError> {
        loop {
            match &mut self.state {
                RecvState::Len { buf, filled } => {
                    while *filled < buf.len() {
                        let n = self.stdout.read(&mut buf[*filled..]).await?;
                        if n == 0 {
                            // EOF at the boundary is a clean close; inside the
                            // prefix the peer died mid-write.
                            return if *filled == 0 {
                                Ok(None)
                            } else {
                                Err(FrameError::Truncated)
                            };
                        }
                        *filled += n;
                    }
                    let len = u32::from_le_bytes(*buf);
                    if len > MAX_FRAME_LEN {
                        return Err(FrameError::FrameTooLarge {
                            len,
                            max: MAX_FRAME_LEN,
                        });
                    }
                    // Allocation is up front but bounded by MAX_FRAME_LEN
                    // (256 MiB), keeping byzantine peers bounded to one frame
                    // buffer per worker.
                    self.state = RecvState::Body {
                        buf: vec![0u8; len as usize],
                        filled: 0,
                    };
                }
                RecvState::Body { buf, filled } => {
                    while *filled < buf.len() {
                        let n = self.stdout.read(&mut buf[*filled..]).await?;
                        if n == 0 {
                            // EOF after a length prefix is always mid-frame.
                            return Err(FrameError::Truncated);
                        }
                        *filled += n;
                    }
                    let done = mem::replace(&mut self.state, RecvState::Len { buf: [0; 4], filled: 0 });
                    let RecvState::Body { buf: body, .. } = done else {
                        unreachable!("matched Body above");
                    };
                    // `decode_frame` resets the per-frame decode budget;
                    // decoding is fully synchronous, so the thread-local budget
                    // cannot interleave with another worker's frame.
                    return decode_frame::<pb::ChildEvent>(&body).map(Some);
                }
            }
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
