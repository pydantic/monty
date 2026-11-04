//! The remote transport: one gRPC channel per pool, one `MontySession.Session`
//! stream per worker.
//!
//! [`Remote`] owns the HTTP/2 connection (dialed lazily, reconnected by tonic
//! when it drops) and opens a stream per checkout. [`RemoteWorker`] is that
//! stream: requests go down an `mpsc` channel that tonic drains as the
//! request body, and a reader task decodes the response stream into another
//! channel so `recv` is cancel-safe and the stream is polled even while the
//! worker sits idle — which is what lets it answer a server's `Ping` between
//! turns, the liveness signal a relay behind a proxy relies on.

use std::{
    error::Error,
    fmt::Display,
    sync::{Arc, Once, OnceLock},
    time::Duration,
};

use bytes::Bytes;
use h2::Reason;
use http::uri::PathAndQuery;
use monty_proto::{
    FrameError, MAX_FRAME_LEN, encode_to_capped_vec,
    grpc::{BytesCodec, Cause, SESSION_PATH},
    pb,
};
use rustls::crypto::aws_lc_rs::default_provider;
use tokio::{
    runtime::Handle,
    sync::{
        Semaphore,
        mpsc::{self, WeakSender},
    },
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    Code, Extensions, Request, Status, Streaming,
    client::Grpc,
    metadata::{MetadataKey, MetadataMap, MetadataValue},
    transport::{Channel, ClientTlsConfig, Endpoint},
};

use crate::{
    PoolError, RemoteDisconnect,
    worker::{DECODE_OFFLOAD_MIN, decode_event},
};

/// A pool's connection to its remote server. Cheap to keep: the channel
/// connects on the first stream and stays up across checkouts.
pub(crate) struct Remote {
    /// The URL as the caller gave it, for error messages.
    url: String,
    channel: Channel,
    /// Bounds opening a stream: the TCP+TLS dial on a cold channel plus the
    /// server's admission (which, for a relay, includes dialing a worker).
    connect_timeout: Duration,
    /// Caps decodes of large frames at the pool's `max_processes`, across every
    /// reader. A dropped checkout frees its capacity slot at once, while its
    /// reader may be inside a decode an abort cannot interrupt, so without
    /// this a run of drops could stack such decodes past the cap.
    decode_permits: Arc<Semaphore>,
    /// Caps closed streams whose reader is still waiting out [`CLOSE_GRACE`]
    /// at `max_processes`: past it a close resets the stream, so what peers
    /// can buffer on retired streams tracks the pool's size, not its churn.
    closing: Arc<Semaphore>,
}

impl Remote {
    /// Prepares the channel for `url` without connecting; the first stream
    /// dials. A URL tonic cannot parse fails here, so a pool with a bad URL
    /// fails at `Pool::new` rather than at its first checkout.
    pub(crate) fn connect(
        url: &str,
        connect_timeout: Duration,
        keep_alive: Option<Duration>,
        max_processes: usize,
    ) -> Result<Self, PoolError> {
        let spawn_error = |err: &dyn Display| PoolError::Spawn(format!("{url}: {err}"));
        let mut endpoint = Endpoint::from_shared(dial_url(url)?)
            .map_err(|err| spawn_error(&err))?
            .user_agent(USER_AGENT_VALUE)
            .map_err(|err| spawn_error(&err))?
            .connect_timeout(connect_timeout)
            .tcp_nodelay(true)
            // fixed windows, the connection's well above a stream's: adaptive
            // ones set both to one estimate, so a session whose host stopped
            // reading held the whole connection and starved every other session
            .initial_stream_window_size(STREAM_WINDOW_BYTES)
            .initial_connection_window_size(connection_window(max_processes));
        if let Some(interval) = keep_alive {
            // Hop-local liveness while streams are open: a dead TCP peer is
            // noticed without waiting for a turn to time out (the in-band
            // `Ping` covers proxied hops). Never while idle: a gRPC server's
            // default policy counts a ping without a stream as abuse.
            endpoint = endpoint
                .http2_keep_alive_interval(interval)
                .keep_alive_timeout(KEEP_ALIVE_TIMEOUT)
                .keep_alive_while_idle(false);
        }
        if endpoint.uri().scheme_str() == Some("https") {
            install_crypto_provider();
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_webpki_roots())
                .map_err(|err| spawn_error(&err))?;
        }
        Ok(Self {
            url: url.to_owned(),
            channel: endpoint.connect_lazy(),
            connect_timeout,
            decode_permits: Arc::new(Semaphore::new(max_processes.max(1))),
            closing: Arc::new(Semaphore::new(max_processes.max(1))),
        })
    }

    /// Opens one session stream with `metadata` as its request
    /// metadata (see [`crate::CheckoutOptions`]). Returns once the server has
    /// admitted the stream — sent its response headers — so a refusal
    /// (capacity, authentication, no worker) surfaces here as a
    /// [`PoolError::Refused`] carrying the server's status, and a dial that
    /// fails or times out as a [`PoolError::Spawn`], never as a worker that
    /// fails its first turn.
    pub(crate) async fn open(
        &self,
        metadata: &[(String, String)],
        max_decode_bytes: usize,
    ) -> Result<RemoteWorker, PoolError> {
        let url = &self.url;
        let mut map = MetadataMap::new();
        // a malformed name/value is a caller bug: fail the dial
        for (name, value) in metadata {
            let key = MetadataKey::from_bytes(name.as_bytes())
                .map_err(|err| PoolError::Spawn(format!("{url}: metadata entry {name:?}: {err}")))?;
            let value = MetadataValue::try_from(value.as_str())
                .map_err(|err| PoolError::Spawn(format!("{url}: metadata entry {name:?} value: {err}")))?;
            // never indexed: a token must not sit in the connection's HPACK
            // table, which every stream on it shares
            let mut value = value;
            value.set_sensitive(true);
            map.insert(key, value);
        }
        let mut grpc = Grpc::new(self.channel.clone())
            .max_decoding_message_size(MAX_FRAME_LEN as usize)
            .max_encoding_message_size(MAX_FRAME_LEN as usize);
        let deadline = Instant::now() + self.connect_timeout;
        let mut retries = 0;
        let (requests, response) = loop {
            let (requests, request_body) = mpsc::channel::<Bytes>(REQUEST_CHANNEL_DEPTH);
            let request = Request::from_parts(map.clone(), Extensions::default(), ReceiverStream::new(request_body));
            let open = async {
                grpc.ready().await.map_err(|err| Status::from_error(err.into()))?;
                grpc.streaming(request, PathAndQuery::from_static(SESSION_PATH), BytesCodec)
                    .await
            };
            match timeout_at(deadline, open).await {
                Ok(Ok(response)) => break (requests, response),
                // the server closed the connection under the stream before
                // taking it (a GOAWAY as it drains, a refused stream): the
                // channel reconnects, and the same budget covers the retries
                Ok(Err(status)) if retries < OPEN_RETRIES && stream_not_taken(&status) => retries += 1,
                // the server's own answer, kept structured; a status tonic
                // synthesised for a transport failure has a source error
                Ok(Err(status)) if Error::source(&status).is_none() => {
                    return Err(PoolError::Refused {
                        url: url.to_owned(),
                        refusal: Box::new(RemoteDisconnect::from_status(&status)),
                    });
                }
                Ok(Err(status)) => {
                    return Err(PoolError::Spawn(format!("{url}: {}", describe_status(&status))));
                }
                Err(_) => {
                    return Err(PoolError::Spawn(format!(
                        "{url}: connect timed out after {:?}",
                        self.connect_timeout
                    )));
                }
            }
        };
        let (events_tx, events) = mpsc::channel(EVENT_CHANNEL_DEPTH);
        let disconnect = Arc::new(OnceLock::new());
        let reader = tokio::spawn(read_events(
            response.into_inner(),
            events_tx,
            requests.downgrade(),
            Arc::clone(&disconnect),
            max_decode_bytes,
            Arc::clone(&self.decode_permits),
        ));
        Ok(RemoteWorker {
            requests: Some(requests),
            events,
            disconnect,
            reader: Some(reader),
            runtime: Handle::current(),
            closing: Arc::clone(&self.closing),
        })
    }
}

/// How many more times a stream the server never took is opened: once on the
/// connection that was closing, once on the one the channel dials to replace it.
const OPEN_RETRIES: u32 = 2;

/// One session stream. See the module docs for the two halves; teardown
/// releases both — see [`RemoteWorker::close`].
pub(crate) struct RemoteWorker {
    /// `send` writes here; `None` after teardown (the closed-stream sentinel).
    /// Dropping the last sender half-closes the stream, the "host is done"
    /// signal a server ends the session on.
    requests: Option<mpsc::Sender<Bytes>>,
    /// Decoded events (or the terminal error) forwarded by the reader task.
    events: mpsc::Receiver<Result<(pb::ChildEvent, usize), FrameError>>,
    /// The first failure, retained independently of queued protocol events.
    disconnect: Arc<OnceLock<RemoteDisconnect>>,
    /// The task owning the response stream; `None` once `close` detached it.
    reader: Option<JoinHandle<()>>,
    /// The runtime the reader runs on, which carries it through a close even
    /// when the worker is dropped outside any runtime.
    runtime: Handle,
    /// See [`Remote::closing`].
    closing: Arc<Semaphore>,
}

impl RemoteWorker {
    /// Sends one request, returning its encoded size. An oversize request is
    /// rejected before any I/O so the stream stays in sync; a torn-down
    /// worker fails as [`FrameError::Truncated`].
    ///
    /// A stream the server ended under the request is not a failure here:
    /// what it ended with — a final event such as a deadline's exception, then
    /// its status — is on the read side, so the turn goes on to [`Self::recv`]
    /// and reports the server's last word rather than a bare send failure.
    pub(crate) async fn send(&mut self, request: &pb::ParentRequest) -> Result<usize, FrameError> {
        let body = encode_to_capped_vec(request)?;
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
    pub(crate) async fn recv(&mut self) -> Result<(pb::ChildEvent, usize), FrameError> {
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

    /// Ends the stream: half-close the request side and leave the detached
    /// reader holding the response stream open until the server ends its side
    /// (bounded by [`CLOSE_GRACE`]), then call `on_closed`. Returns at once:
    /// what the server does before ending — a relay parks the session — is not
    /// the host's wait.
    ///
    /// The half-close is what a *proxied* server sees: a bare stream reset may
    /// stop at a load balancer, leaving the session (and its capacity slot)
    /// alive until the far side's own idle timeout. Past [`Remote::closing`]'s
    /// cap the stream is reset instead.
    pub(crate) fn close(&mut self, on_closed: impl FnOnce() + Send + 'static) {
        self.requests = None;
        // runs `on_closed` however this ends, including a runtime shut down
        // under the detached task
        let closed = OnDrop(Some(on_closed));
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

/// `User-Agent` on every stream, identifying this crate to the server or
/// relay it dials; tonic appends its own version after it.
const USER_AGENT_VALUE: &str = concat!("monty-pool/", env!("CARGO_PKG_VERSION"));

/// How long an HTTP/2 `PING` may go unanswered before the connection is
/// judged dead. Connection-level, hop-local: it detects a dead TCP peer, not
/// a vanished server beyond a proxy (that is the in-band `Ping`'s job). The
/// interval between pings is `PoolConfig::keep_alive`.
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// A stream's HTTP/2 receive window: a session whose host stopped reading
/// holds at most this much of the connection's.
const STREAM_WINDOW_BYTES: u32 = 2 * 1024 * 1024;

/// The smallest connection window, so a small pool keeps the window it had.
const MIN_CONNECTION_WINDOW_BYTES: u32 = 32 * 1024 * 1024;

/// HTTP/2's largest flow-control window.
const MAX_WINDOW_BYTES: u32 = (1 << 31) - 1;

/// The connection's receive window, shared by every stream on the channel:
/// room for each stream the pool can hold — `max_processes` live and as many
/// closing — to sit on a full stream window with one to spare, so stalled
/// sessions never take the credit a neighbour's reply needs.
fn connection_window(max_processes: usize) -> u32 {
    let streams = max_processes.max(1).saturating_mul(2).saturating_add(1);
    u32::try_from(streams)
        .ok()
        .and_then(|streams| streams.checked_mul(STREAM_WINDOW_BYTES))
        .map_or(MAX_WINDOW_BYTES, |window| window.min(MAX_WINDOW_BYTES))
        .max(MIN_CONNECTION_WINDOW_BYTES)
}

/// How long a closed stream's reader stays up for the server to end its side
/// after the half-close. The checkout does not wait on it, but a finished
/// checkout's pool slot does (see [`crate::pool::PoolInner::release_worker`]);
/// it also bounds what a peer that stopped reading can hold open.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// Depth of the request channel. The protocol sends one request per turn (plus
/// the odd `AbortFeed`, and `Pong`s from the reader task), so a few slots keep
/// the sender from blocking while tonic drains the body; deeper buys nothing.
const REQUEST_CHANNEL_DEPTH: usize = 4;

/// Depth of the reader-task → `recv` channel. One slot, reserved before the
/// poll, bounds an untrusted peer to a single queued event (≤
/// [`MAX_FRAME_LEN`]) per worker. Deeper buys nothing: pings need polling,
/// not buffering, and the server's print backpressure assumes unread data
/// sits in the stream's flow-control window, not in client memory.
const EVENT_CHANNEL_DEPTH: usize = 1;

/// The reader task: polls the response stream continuously so a `Ping`
/// is answered even while the worker is idle (see the module docs),
/// forwarding each decoded event — or the terminal error — to `recv`.
///
/// Reserving the slot before polling keeps the reader polling (and ponging)
/// rather than blocked holding an event. `pongs` is weak so this task never
/// keeps the request body open after the worker half-closed it.
async fn read_events(
    mut stream: Streaming<Bytes>,
    events: mpsc::Sender<Result<(pb::ChildEvent, usize), FrameError>>,
    pongs: WeakSender<Bytes>,
    disconnect: Arc<OnceLock<RemoteDisconnect>>,
    max_decode_bytes: usize,
    decode_permits: Arc<Semaphore>,
) {
    // an Err from reserve means the receiver dropped: the worker is gone
    while let Ok(permit) = events.reserve().await {
        let result = loop {
            match stream.message().await {
                Ok(Some(body)) => {
                    // a large frame decodes under `block_in_place`, which an
                    // abort cannot interrupt: hold a permit so the pool runs no
                    // more such decodes at once than it has slots, and only for
                    // the decode, never across a pong send that may wait
                    let decoded = {
                        let _permit = if body.len() >= DECODE_OFFLOAD_MIN {
                            decode_permits.acquire().await.ok()
                        } else {
                            None
                        };
                        decode_event(&body, max_decode_bytes)
                    };
                    match decoded {
                        Ok(event) if matches!(event.kind, Some(pb::child_event::Kind::Ping(_))) => {
                            // answered here, not by the checkout: a ping can land
                            // between turns, when nothing else polls the stream
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
                }
                // the server ended the stream with an OK status: a session
                // it considers finished, which mid-checkout is still a loss
                Ok(None) => {
                    let _ = disconnect.set(RemoteDisconnect {
                        status_code: Some(code_name(Code::Ok)),
                        status_message: None,
                        cause: None,
                        transport_error: None,
                    });
                    break Err(FrameError::Truncated);
                }
                Err(status) => {
                    let _ = disconnect.set(RemoteDisconnect::from_status(&status));
                    break Err(FrameError::Truncated);
                }
            }
        };
        // any error is terminal: deliver it and exit
        let stop = result.is_err();
        permit.send(result);
        if stop {
            return;
        }
    }
    // the worker is gone, so this is a closed stream: hold it open until the
    // server ends its side, so the half-close is read ahead of any reset
    while let Ok(Some(_)) = stream.message().await {}
}

/// The encoded `Pong` request, built once per answer (it is two bytes).
fn pong_body() -> Bytes {
    let pong = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Pong(pb::Pong {})),
        trace_parent: None,
    };
    Bytes::from(encode_to_capped_vec(&pong).expect("a Pong is far below the frame cap"))
}

impl RemoteDisconnect {
    /// Classifies a status that ended the stream: one the server sent carries
    /// its code, message and cause; one tonic synthesised for a transport
    /// failure (it has a source error) is recorded as that failure instead.
    fn from_status(status: &Status) -> Self {
        if Error::source(status).is_some() {
            Self::failure(describe_status(status))
        } else {
            Self {
                status_code: Some(code_name(status.code())),
                status_message: Some(status.message().to_owned()),
                cause: Cause::of(status),
                transport_error: None,
            }
        }
    }

    /// Describes a transport failure for which the server sent no status.
    fn failure(message: impl Into<String>) -> Self {
        Self {
            status_code: None,
            status_message: None,
            cause: None,
            transport_error: Some(message.into()),
        }
    }
}

/// Whether `status` says the server never took the stream: a `GOAWAY` whose
/// last stream is below it (the server was going away as it was opened), a
/// `REFUSED_STREAM` reset, or hyper cancelling the request because the
/// connection closed before it was sent. HTTP/2 promises such a stream was
/// not processed, so opening it again is safe. h2 reports a `GOAWAY` on a
/// stream only when the stream's id is above the frame's last stream id; a
/// stream the server did process never sees it.
fn stream_not_taken(status: &Status) -> bool {
    let mut source = Error::source(status);
    while let Some(err) = source {
        if let Some(h2) = err.downcast_ref::<h2::Error>() {
            return (h2.is_go_away() && h2.reason() == Some(Reason::NO_ERROR))
                || (h2.is_reset() && h2.reason() == Some(Reason::REFUSED_STREAM));
        }
        if let Some(hyper) = err.downcast_ref::<hyper::Error>()
            && hyper.is_canceled()
        {
            return true;
        }
        source = err.source();
    }
    false
}

/// Renders a status for an error message: its code and message, then the
/// chain of underlying errors tonic attached for a transport failure.
fn describe_status(status: &Status) -> String {
    let mut text = format!("{}: {}", code_name(status.code()), status.message());
    let mut source = Error::source(status);
    while let Some(err) = source {
        text.push_str(": ");
        text.push_str(&err.to_string());
        source = err.source();
    }
    text
}

/// The canonical upper-snake name of a gRPC code, as the `grpc-status`
/// documentation and every other implementation spell it.
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

/// The URL tonic dials: `ws`/`wss` read as `http`/`https`, the scheme matched
/// without regard to case, and any other scheme refused. tonic accepts any
/// scheme and enables TLS only for `https`, so an unrecognised one would dial
/// in plaintext, metadata and all.
fn dial_url(url: &str) -> Result<String, PoolError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| PoolError::Spawn(format!("{url}: invalid URI")))?;
    let normalised = if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("ws") {
        "http"
    } else if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") {
        "https"
    } else {
        return Err(PoolError::Spawn(format!(
            "{url}: unsupported URL scheme {scheme:?}, expected http or https"
        )));
    };
    Ok(format!("{normalised}://{rest}"))
}

/// Installs the process-level rustls `CryptoProvider` exactly once before the
/// first `https://` dial. rustls 0.23 panics on first TLS use when it can't
/// pick a provider automatically (both `aws-lc-rs` and `ring`, or neither,
/// compiled in), so we name `aws_lc_rs` explicitly. Idempotent via `Once`, and
/// the install error is ignored: another part of the process (e.g. a host
/// embedding the pool) may have already installed a provider, which is fine.
fn install_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = default_provider().install_default();
    });
}

#[cfg(test)]
#[path = "../tests/unit/remote.rs"]
mod tests;
