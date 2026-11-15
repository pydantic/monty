//! The remote transport: a session per checkout on a remote server, over gRPC
//! (`http://`, `https://`) or WebSocket (`ws://`, `wss://`), chosen by the URL's scheme.
//!
//! [`Remote`] is the pool's handle on the server: for gRPC one HTTP/2 channel
//! shared by every stream ([`grpc`]), for WebSocket a connection dialed per
//! checkout ([`websocket`]). Either way a session becomes the same
//! [`RemoteWorker`]: requests go down an `mpsc` channel that the transport
//! drains, and a reader task decodes the server's messages into another
//! channel so `recv` is cancel-safe and the transport is polled even while the
//! worker sits idle — which is what lets it answer a server's `Ping` between
//! turns, the liveness signal a relay behind a proxy relies on.
//!
//! Only the read side reports why a session ended: a send that races the
//! server's goodbye fails without a word, so the reason the server gave (a
//! gRPC status, a WebSocket close code) is never shadowed by a local error.

mod grpc;
mod websocket;

use std::{
    collections::VecDeque,
    sync::{Arc, Once, OnceLock},
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use monty_proto::{FrameError, MAX_FRAME_LEN, OpeningRequest, encode_to_capped_vec, pb};
use rustls::crypto::{CryptoProvider, aws_lc_rs::default_provider};
use tokio::{
    runtime::Handle,
    sync::{
        Notify, Semaphore,
        mpsc::{self, WeakSender},
    },
    task::JoinHandle,
    time::{Instant, timeout},
};
use tonic::Code;

use self::{grpc::GrpcChannel, websocket::WebSocketDialer};
use crate::{
    PoolError, RemoteDisconnect,
    worker::{DECODE_OFFLOAD_MIN, decode_event},
};

/// A pool's handle on its remote server. Cheap to keep: nothing dials until
/// the first checkout.
pub(crate) struct Remote {
    /// The URL as the caller gave it, for error messages.
    url: String,
    transport: Transport,
    /// Bounds opening a session: the TCP+TLS dial plus the server's
    /// admission (which, for a relay, includes dialing a worker).
    dial_timeout: Duration,
    /// Caps decodes of large frames at the pool's `max_processes`, across every
    /// reader. A dropped checkout frees its capacity slot at once, while its
    /// reader may be inside a decode an abort cannot interrupt, so without
    /// this a run of drops could stack such decodes past the cap.
    decode_permits: Arc<Semaphore>,
    /// Caps closed sessions whose reader is still waiting out [`CLOSE_GRACE`]
    /// at `max_processes`: past it a close drops the session at once, so what
    /// peers can buffer on retired sessions tracks the pool's size, not its churn.
    closing: Arc<Semaphore>,
    closing_complete: Arc<Notify>,
}

/// The wire a [`Remote`] opens sessions over.
enum Transport {
    Grpc(GrpcChannel),
    WebSocket(WebSocketDialer),
}

impl Remote {
    /// Prepares the transport `url`'s scheme names, without connecting. A URL
    /// that cannot be dialed fails here, so a pool with a bad URL fails at
    /// `Pool::new` rather than at its first checkout.
    pub(crate) fn connect(
        url: &str,
        dial_timeout: Duration,
        keep_alive: Option<Duration>,
        max_processes: usize,
    ) -> Result<Self, PoolError> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| connection_failure(url, "invalid URI"))?;
        let transport = match scheme.to_ascii_lowercase().as_str() {
            scheme @ ("http" | "https") => Transport::Grpc(GrpcChannel::connect(
                url,
                scheme,
                rest,
                dial_timeout,
                keep_alive,
                max_processes,
            )?),
            scheme @ ("ws" | "wss") => Transport::WebSocket(WebSocketDialer::new(url, scheme, rest)?),
            _ => {
                return Err(connection_failure(
                    url,
                    format!("unsupported URL scheme {scheme:?}, expected http, https, ws or wss"),
                ));
            }
        };
        Ok(Self {
            url: url.to_owned(),
            transport,
            dial_timeout,
            decode_permits: Arc::new(Semaphore::new(max_processes.max(1))),
            closing: Arc::new(Semaphore::new(max_processes.max(1))),
            closing_complete: Arc::new(Notify::new()),
        })
    }

    /// Opens one session with `metadata` as its request metadata (gRPC) or
    /// upgrade headers (WebSocket); see [`crate::CheckoutOptions`]. Returns
    /// once the server has admitted it, so a refusal (capacity,
    /// authentication, no worker) surfaces here as a [`PoolError::Refused`]
    /// carrying the server's reason, and a dial that fails, times out or
    /// reaches no Monty server as a [`PoolError::Connect`], never as a worker
    /// that fails its first turn.
    pub(crate) async fn open(
        &self,
        metadata: &[(String, String)],
        max_decode_bytes: usize,
    ) -> Result<RemoteWorker, PoolError> {
        match &self.transport {
            Transport::Grpc(channel) => {
                let (requests, stream) = channel
                    .open(&self.url, self.dial_timeout, metadata, max_decode_bytes)
                    .await?;
                Ok(self.start(requests, stream, max_decode_bytes))
            }
            Transport::WebSocket(dialer) => {
                let (requests, source) = dialer
                    .open(&self.url, self.dial_timeout, metadata, max_decode_bytes)
                    .await?;
                Ok(self.start(requests, source, max_decode_bytes))
            }
        }
    }

    /// Wait for departed sessions, without waiting for checkouts that are still live.
    pub(crate) async fn wait_closed(&self, max_processes: usize) {
        loop {
            let changed = self.closing_complete.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.closing.available_permits() == max_processes.max(1) {
                return;
            }
            changed.await;
        }
    }

    /// Wraps an admitted session: `requests` feeds the transport's send side,
    /// `source` is its receive side, which the reader task takes over.
    fn start(
        &self,
        requests: mpsc::Sender<Bytes>,
        source: impl MessageSource,
        max_decode_bytes: usize,
    ) -> RemoteWorker {
        let (events_tx, events) = mpsc::channel(EVENT_CHANNEL_DEPTH);
        let disconnect = Arc::new(OnceLock::new());
        let reader = tokio::spawn(read_events(
            source,
            events_tx,
            requests.downgrade(),
            Arc::clone(&disconnect),
            max_decode_bytes,
            Arc::clone(&self.decode_permits),
        ));
        RemoteWorker {
            requests: Some(requests),
            events,
            disconnect,
            reader: Some(reader),
            runtime: Handle::current(),
            closing: Arc::clone(&self.closing),
            closing_complete: Arc::clone(&self.closing_complete),
        }
    }
}

/// A session's receive side, as the reader task sees it: one protocol
/// message per call, until the session ends.
trait MessageSource: Send + 'static {
    /// The next message's bytes, or how the session ended. After an `Err`
    /// the source is finished.
    fn next(&mut self) -> impl Future<Output = Result<Bytes, Ended>> + Send;
}

/// How a session's receive side ended: the evidence a disconnect reports,
/// and the frame error the pending `recv` fails with.
struct Ended {
    disconnect: RemoteDisconnect,
    error: FrameError,
}

impl Ended {
    /// The server ended the session with a status (or close code) of its
    /// own, which says all there is to say: a vanished peer to the turn.
    fn status(disconnect: RemoteDisconnect) -> Self {
        Self {
            disconnect,
            error: FrameError::Truncated,
        }
    }

    /// A transport or protocol failure, recorded as `error`'s text.
    fn failure(error: FrameError) -> Self {
        Self {
            disconnect: RemoteDisconnect::failure(error.to_string()),
            error,
        }
    }
}

/// The largest message a session decoding under `max_decode_bytes` accepts:
/// a frame cannot decode in less than its own size, so a larger one is
/// refused before it is buffered. Never below [`MIN_MESSAGE_LIMIT`], which
/// keeps control events through a tiny budget.
fn message_limit(max_decode_bytes: usize) -> usize {
    max_decode_bytes.clamp(MIN_MESSAGE_LIMIT, MAX_FRAME_LEN as usize)
}

/// The smallest [`message_limit`].
const MIN_MESSAGE_LIMIT: usize = 64 * 1024;

/// Whether `code` is one a server refuses a session's admission with: no room
/// (`RESOURCE_EXHAUSTED`, `UNAVAILABLE`), bad credentials (`UNAUTHENTICATED`,
/// `PERMISSION_DENIED`), or a request it cannot serve (`FAILED_PRECONDITION`).
fn refuses_admission(code: Code) -> bool {
    matches!(
        code,
        Code::ResourceExhausted
            | Code::Unavailable
            | Code::Unauthenticated
            | Code::PermissionDenied
            | Code::FailedPrecondition
    )
}

/// One remote session. See the module docs for the two halves; teardown
/// releases both — see [`RemoteWorker::close`].
pub(crate) struct RemoteWorker {
    /// `send` writes here; `None` after teardown (the closed-session sentinel).
    /// Dropping the last sender ends the send side (a gRPC half-close, a
    /// WebSocket Close frame), the "host is done" signal a server ends the
    /// session on.
    requests: Option<mpsc::Sender<Bytes>>,
    /// Decoded events (or the terminal error) forwarded by the reader task.
    events: mpsc::Receiver<Result<(pb::WorkerEvent, usize), FrameError>>,
    /// The first failure, retained independently of queued protocol events.
    disconnect: Arc<OnceLock<RemoteDisconnect>>,
    /// The task owning the receive side; `None` once `close` detached it.
    reader: Option<JoinHandle<()>>,
    /// The runtime the reader runs on, which carries it through a close even
    /// when the worker is dropped outside any runtime.
    runtime: Handle,
    /// See [`Remote::closing`].
    closing: Arc<Semaphore>,
    closing_complete: Arc<Notify>,
}

impl RemoteWorker {
    /// Sends one request, returning its encoded size. An oversize request is
    /// rejected before any I/O so the session stays in sync; a torn-down
    /// worker fails as [`FrameError::Truncated`].
    ///
    /// A session the server ended under the request is not a failure here:
    /// what it ended with — a final event such as a deadline's exception, then
    /// its status or close code — is on the read side, so the turn goes on to
    /// [`Self::recv`] and reports the server's last word rather than a bare
    /// send failure.
    pub(crate) async fn send(
        &mut self,
        request: &pb::ClientRequest,
        opening: Option<&OpeningRequest<'_>>,
    ) -> Result<usize, FrameError> {
        let body = match opening {
            Some(opening) => encode_to_capped_vec(opening)?,
            None => encode_to_capped_vec(request)?,
        };
        let len = body.len();
        match &self.requests {
            Some(requests) => {
                let _ = requests.send(Bytes::from(body)).await;
                Ok(len)
            }
            None => Err(FrameError::Truncated),
        }
    }

    /// Receives the next decoded event with its wire size. Cancel-safe: a
    /// channel receive consumes nothing when dropped. A torn-down worker or a
    /// finished reader is [`FrameError::Truncated`] — a
    /// [`PoolError::Disconnected`].
    pub(crate) async fn recv(&mut self) -> Result<(pb::WorkerEvent, usize), FrameError> {
        if self.requests.is_none() {
            return Err(FrameError::Truncated);
        }
        match self.events.recv().await {
            Some(result) => result,
            None => Err(FrameError::Truncated),
        }
    }

    /// Copies the first recorded failure before teardown can add a local one.
    pub(crate) fn disconnect(&self) -> Option<Box<RemoteDisconnect>> {
        self.disconnect.get().cloned().map(Box::new)
    }

    /// Ends the session: end the send side and leave the detached reader
    /// holding the receive side open until the server ends its side too
    /// (bounded by [`CLOSE_GRACE`]), then call `on_closed`. Returns at once:
    /// the checkout does not wait for the server's final work, such as parking.
    /// [`crate::Pool::close`] waits for these detached readers.
    ///
    /// Ending the send side is what a *proxied* server sees: a bare reset or
    /// TCP drop may stop at a load balancer, leaving the session (and its
    /// capacity slot) alive until the far side's own idle timeout. Past
    /// [`Remote::closing`]'s cap the session is dropped at once instead.
    pub(crate) fn close(&mut self, on_closed: impl FnOnce() + Send + 'static) {
        self.requests = None;
        // runs `on_closed` however this ends, including a runtime shut down
        // under the detached task
        let closing_complete = Arc::clone(&self.closing_complete);
        let closed = OnDrop(Some(move || {
            on_closed();
            closing_complete.notify_waiters();
        }));
        if let Some(reader) = self.reader.take() {
            let abort = reader.abort_handle();
            match Arc::clone(&self.closing).try_acquire_owned() {
                Ok(permit) => {
                    self.runtime.spawn(async move {
                        let _held = (permit, closed);
                        if timeout(CLOSE_GRACE, reader).await.is_err() {
                            abort.abort();
                        }
                    });
                }
                Err(_) => abort.abort(),
            }
        }
    }
}

/// Backstop for the teardown paths that never call `close` — an abandoned
/// [`crate::Checkout`], a discarded worker: the same goodbye.
impl Drop for RemoteWorker {
    fn drop(&mut self) {
        self.close(|| {});
    }
}

/// Calls its closure when dropped.
struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// `User-Agent` on every session, identifying this crate to the server or
/// relay it dials; tonic appends its own version after it.
const USER_AGENT_VALUE: &str = concat!("monty-pool/", env!("CARGO_PKG_VERSION"));

/// How long a closed session's reader stays up for the server to end its
/// side after ours. The checkout does not wait on it, but a finished
/// checkout's pool slot does (see [`crate::pool::PoolInner::release_worker`]);
/// it also bounds what a peer that stopped reading can hold open.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// Depth of the request channel. The protocol sends one request per turn (plus
/// the odd `AbortFeed`, and `Pong`s from the reader task), so a few slots keep
/// the sender from blocking while the transport drains it; deeper buys nothing.
const REQUEST_CHANNEL_DEPTH: usize = 4;

/// Depth of the reader-task → `recv` channel. One slot, reserved before the
/// poll, bounds an untrusted peer to a single queued event (≤
/// [`MAX_FRAME_LEN`]) per worker. Deeper buys nothing: pings need polling,
/// not buffering, and the server's print backpressure assumes unread data
/// sits in the transport's flow control, not in client memory.
const EVENT_CHANNEL_DEPTH: usize = 1;

/// The reader task: polls the receive side continuously so a `Ping` is
/// answered even while the worker is idle (see the module docs), forwarding
/// each decoded event — or the terminal error — to `recv`.
///
/// Reserving the slot before polling keeps the reader polling (and ponging)
/// rather than blocked holding an event. `pongs` is weak so this task never
/// keeps the send side open after the worker ended it.
async fn read_events(
    mut source: impl MessageSource,
    events: mpsc::Sender<Result<(pb::WorkerEvent, usize), FrameError>>,
    pongs: WeakSender<Bytes>,
    disconnect: Arc<OnceLock<RemoteDisconnect>>,
    max_decode_bytes: usize,
    decode_permits: Arc<Semaphore>,
) {
    let mut pings = PingLimit::new();
    // an Err from reserve means the receiver dropped: the worker is gone
    while let Ok(permit) = events.reserve().await {
        let result = loop {
            let body = match source.next().await {
                Ok(body) => body,
                Err(ended) => {
                    let _ = disconnect.set(ended.disconnect);
                    break Err(ended.error);
                }
            };
            // a large frame decodes under `block_in_place`, which an abort
            // cannot interrupt: hold a permit so the pool runs no more such
            // decodes at once than it has slots, and only for the decode,
            // never across a pong send that may wait
            let decoded = {
                let _permit = if body.len() >= DECODE_OFFLOAD_MIN {
                    decode_permits.acquire().await.ok()
                } else {
                    None
                };
                decode_event(&body, max_decode_bytes)
            };
            match decoded {
                Ok(event) if matches!(event.kind, Some(pb::worker_event::Kind::Ping(_))) => {
                    // no turn deadline covers an idle session, so a ping too
                    // large or too frequent ends it instead
                    if let Err(abuse) = pings.admit(body.len()) {
                        let _ = disconnect.set(RemoteDisconnect::failure(abuse));
                        break Err(FrameError::Truncated);
                    }
                    // answered here, not by the checkout: a ping can land
                    // between turns, when nothing else polls the session
                    if let Some(pongs) = pongs.upgrade() {
                        let _ = pongs.send(pong_body()).await;
                    }
                }
                // a frame this build cannot decode: a protocol failure,
                // recorded as the disconnect's evidence
                Err(err) => {
                    let _ = disconnect.set(RemoteDisconnect::failure(err.to_string()));
                    break Err(err);
                }
                Ok(event) => break Ok((event, body.len())),
            }
        };
        // any error is terminal: deliver it and exit
        let stop = result.is_err();
        permit.send(result);
        if stop {
            return;
        }
    }
    // the worker is gone, so this is a closed session: keep reading until the
    // server ends its side, so ours is seen ahead of any reset
    while source.next().await.is_ok() {}
}

/// Bounds the pings a server may send: each at most [`MAX_PING_BYTES`], and
/// at most [`MAX_PINGS_PER_WINDOW`] in any [`PING_WINDOW`]. A keepalive pings
/// every few seconds; anything past these is a server burning the host's CPU.
struct PingLimit {
    /// Arrival times of the latest pings, oldest first; never more than the cap.
    recent: VecDeque<Instant>,
}

impl PingLimit {
    fn new() -> Self {
        Self {
            recent: VecDeque::with_capacity(MAX_PINGS_PER_WINDOW),
        }
    }

    /// Counts one ping of `len` bytes, or says why the server broke the limit.
    fn admit(&mut self, len: usize) -> Result<(), String> {
        let now = Instant::now();
        // a sliding window: a burst straddling a boundary still counts as one
        while self
            .recent
            .front()
            .is_some_and(|&at| now.duration_since(at) >= PING_WINDOW)
        {
            self.recent.pop_front();
        }
        if len > MAX_PING_BYTES {
            Err(format!(
                "server sent a {len}-byte ping, above the {MAX_PING_BYTES}-byte limit"
            ))
        } else if self.recent.len() >= MAX_PINGS_PER_WINDOW {
            Err(format!(
                "server sent more than {MAX_PINGS_PER_WINDOW} pings in {PING_WINDOW:?}"
            ))
        } else {
            self.recent.push_back(now);
            Ok(())
        }
    }
}

/// The largest ping message accepted; a ping is two bytes on the wire.
const MAX_PING_BYTES: usize = 64;

/// See [`PingLimit`].
const MAX_PINGS_PER_WINDOW: usize = 10;
const PING_WINDOW: Duration = Duration::from_secs(1);

/// The encoded `Pong` request, built once per answer (it is two bytes).
fn pong_body() -> Bytes {
    let pong = pb::ClientRequest {
        kind: Some(pb::client_request::Kind::Pong(pb::Pong {})),
        trace_parent: None,
    };
    Bytes::from(encode_to_capped_vec(&pong).expect("a Pong is far below the frame cap"))
}

impl RemoteDisconnect {
    /// A status the server ended or refused a session with, by its code.
    fn status(code: Code, message: Option<String>, cause: Option<monty_proto::Cause>) -> Self {
        Self {
            status_code: Some(code_name(code)),
            status_message: message,
            cause,
            transport_error: None,
            retry_after: None,
        }
    }

    /// Describes a transport failure for which the server sent no status.
    fn failure(message: impl Into<String>) -> Self {
        Self {
            status_code: None,
            status_message: None,
            cause: None,
            transport_error: Some(message.into()),
            retry_after: None,
        }
    }
}

/// Parses an HTTP delay or date without retaining invalid server metadata,
/// capped at [`MAX_RETRY_AFTER`] so a host timer cannot overflow.
fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let delay = if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        // all digits but too long for a `u64` is still a valid, huge delay
        Duration::from_secs(value.parse().unwrap_or(u64::MAX))
    } else {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .unwrap_or_default()
    };
    Some(delay.min(MAX_RETRY_AFTER))
}

/// The longest `Retry-After` delay reported; a server asking for more gets this.
const MAX_RETRY_AFTER: Duration = Duration::from_hours(1);

/// Records a connection failure for which the server sent no status.
fn connection_failure(url: &str, message: impl Into<String>) -> PoolError {
    PoolError::Connect {
        url: url.to_owned(),
        details: Box::new(RemoteDisconnect::failure(message)),
    }
}

/// The canonical upper-snake name of a gRPC code, as the `grpc-status`
/// documentation and every other implementation spell it. WebSocket
/// refusals and close codes report through the same names.
fn code_name(code: Code) -> &'static str {
    match code {
        Code::Ok => "OK",
        Code::Cancelled => "CANCELLED",
        Code::Unknown => "UNKNOWN",
        Code::InvalidArgument => "INVALID_ARGUMENT",
        Code::DeadlineExceeded => "DEADLINE_EXCEEDED",
        Code::NotFound => "NOT_FOUND",
        Code::AlreadyExists => "ALREADY_EXISTS",
        Code::PermissionDenied => "PERMISSION_DENIED",
        Code::ResourceExhausted => "RESOURCE_EXHAUSTED",
        Code::FailedPrecondition => "FAILED_PRECONDITION",
        Code::Aborted => "ABORTED",
        Code::OutOfRange => "OUT_OF_RANGE",
        Code::Unimplemented => "UNIMPLEMENTED",
        Code::Internal => "INTERNAL",
        Code::Unavailable => "UNAVAILABLE",
        Code::DataLoss => "DATA_LOSS",
        Code::Unauthenticated => "UNAUTHENTICATED",
    }
}

/// Installs the process-level rustls `CryptoProvider` exactly once before the
/// first TLS dial and returns the one in force, for the verifier to check
/// signatures with. rustls 0.23 panics on first TLS use when it can't pick a
/// provider automatically (both `aws-lc-rs` and `ring`, or neither, compiled
/// in), so we name `aws_lc_rs` explicitly. Idempotent via `Once`, and the
/// install error is ignored: another part of the process (e.g. a host
/// embedding the pool) may have already installed a provider, which is fine.
fn install_crypto_provider() -> Arc<CryptoProvider> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = default_provider().install_default();
    });
    CryptoProvider::get_default()
        .expect("a rustls CryptoProvider is installed above")
        .clone()
}

#[cfg(test)]
#[path = "../../tests/unit/remote.rs"]
mod tests;
