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
    collections::VecDeque,
    error::Error,
    fmt::Display,
    sync::{Arc, Once, OnceLock},
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use h2::Reason;
use http::{Response, StatusCode, uri::PathAndQuery};
use monty_proto::{
    FrameError, MAX_FRAME_LEN, encode_to_capped_vec,
    grpc::{BytesCodec, Cause, SESSION_PATH},
    pb,
};
use rustls::crypto::{CryptoProvider, aws_lc_rs::default_provider};
use rustls_platform_verifier::Verifier;
use tokio::{
    runtime::Handle,
    sync::{
        Notify, Semaphore,
        mpsc::{self, WeakSender},
    },
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    Code, Extensions, Request, Status, Streaming,
    body::Body,
    client::Grpc,
    metadata::{MetadataKey, MetadataMap, MetadataValue},
    transport::{Channel, ClientTlsConfig, Endpoint},
};
use tower::ServiceExt;

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
    dial_timeout: Duration,
    /// Caps decodes of large frames at the pool's `max_processes`, across every
    /// reader. A dropped checkout frees its capacity slot at once, while its
    /// reader may be inside a decode an abort cannot interrupt, so without
    /// this a run of drops could stack such decodes past the cap.
    decode_permits: Arc<Semaphore>,
    /// Caps closed streams whose reader is still waiting out [`CLOSE_GRACE`]
    /// at `max_processes`: past it a close resets the stream, so what peers
    /// can buffer on retired streams tracks the pool's size, not its churn.
    closing: Arc<Semaphore>,
    closing_complete: Arc<Notify>,
}

impl Remote {
    /// Wait for departed streams, without waiting for checkouts that are still live.
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

    /// Prepares the channel for `url` without connecting; the first stream
    /// dials. A URL tonic cannot parse fails here, so a pool with a bad URL
    /// fails at `Pool::new` rather than at its first checkout.
    pub(crate) fn connect(
        url: &str,
        dial_timeout: Duration,
        keep_alive: Option<Duration>,
        max_processes: usize,
    ) -> Result<Self, PoolError> {
        let connect_error = |err: &dyn Display| connection_failure(url, err.to_string());
        let mut endpoint = Endpoint::from_shared(dial_url(url)?)
            .map_err(|err| connect_error(&err))?
            .user_agent(USER_AGENT_VALUE)
            .map_err(|err| connect_error(&err))?
            .connect_timeout(dial_timeout)
            .tcp_nodelay(true)
            // fixed windows, the connection's well above a stream's: adaptive
            // ones set both to one estimate, so a session whose host stopped
            // reading held the whole connection and starved every other session
            .initial_stream_window_size(stream_window(max_processes))
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
            // The server certificate is checked against the OS trust store:
            // macOS and Windows verify it themselves, revocation included; Linux
            // reads the distro CA bundle now, and a host with none fails here.
            let verifier = Verifier::new(install_crypto_provider()).map_err(|err| connect_error(&err))?;
            endpoint = endpoint
                .tls_config_with_verifier(ClientTlsConfig::new(), Arc::new(verifier))
                .map_err(|err| connect_error(&err))?;
        }
        Ok(Self {
            url: url.to_owned(),
            channel: endpoint.connect_lazy(),
            dial_timeout,
            decode_permits: Arc::new(Semaphore::new(max_processes.max(1))),
            closing: Arc::new(Semaphore::new(max_processes.max(1))),
            closing_complete: Arc::new(Notify::new()),
        })
    }

    /// Opens one session stream with `metadata` as its request
    /// metadata (see [`crate::CheckoutOptions`]). Returns once the server has
    /// admitted the stream — sent its response headers — so a refusal
    /// (capacity, authentication, no worker) surfaces here as a
    /// [`PoolError::Refused`] carrying the server's status, and a dial that
    /// fails, times out or reaches no gRPC server as a [`PoolError::Connect`],
    /// never as a worker that fails its first turn.
    pub(crate) async fn open(
        &self,
        metadata: &[(String, String)],
        max_decode_bytes: usize,
    ) -> Result<RemoteWorker, PoolError> {
        let url = &self.url;
        let mut map = MetadataMap::new();
        for (name, value) in metadata {
            let key = MetadataKey::from_bytes(name.as_bytes())
                .map_err(|err| PoolError::Configuration(format!("{url}: metadata entry {name:?}: {err}")))?;
            let value = MetadataValue::try_from(value.as_str())
                .map_err(|err| PoolError::Configuration(format!("{url}: metadata entry {name:?} value: {err}")))?;
            // never indexed: a token must not sit in the connection's HPACK
            // table, which every stream on it shares
            let mut value = value;
            value.set_sensitive(true);
            map.insert(key, value);
        }
        let channel = self.channel.clone().map_response(|mut response: Response<Body>| {
            let status = response.status();
            response.extensions_mut().insert(status);
            response
        });
        let mut grpc = Grpc::new(channel)
            .max_decoding_message_size(message_limit(max_decode_bytes))
            .max_encoding_message_size(MAX_FRAME_LEN as usize);
        // an unrepresentable timeout (`Duration::MAX`) leaves the dial unbounded
        let deadline = Instant::now().checked_add(self.dial_timeout);
        let mut retries = 0;
        let (requests, response) = loop {
            let (requests, request_body) = mpsc::channel::<Bytes>(REQUEST_CHANNEL_DEPTH);
            let request = Request::from_parts(map.clone(), Extensions::default(), ReceiverStream::new(request_body));
            let open = async {
                grpc.ready().await.map_err(|err| Status::from_error(err.into()))?;
                grpc.streaming(request, PathAndQuery::from_static(SESSION_PATH), BytesCodec)
                    .await
            };
            let opened = match deadline {
                Some(deadline) => timeout_at(deadline, open).await,
                None => Ok(open.await),
            };
            match opened {
                // tonic opens a stream on any response without a `grpc-status`,
                // so a plain web server's error page would only fail later, mid-
                // session, as an undecodable body: refuse it here as a misconfiguration
                Ok(Ok(response)) if !is_grpc_response(response.metadata()) => {
                    return Err(connection_failure(
                        url,
                        format!(
                            "not a gRPC server: HTTP {}, the response's content-type is {}",
                            response
                                .extensions()
                                .get::<StatusCode>()
                                .expect("HTTP status is recorded above"),
                            describe_content_type(response.metadata())
                        ),
                    ));
                }
                Ok(Ok(response)) => break (requests, response),
                // the server closed the connection under the stream before
                // taking it (a GOAWAY as it drains, a refused stream): the
                // channel reconnects, and the same budget covers the retries
                Ok(Err(status)) if retries < OPEN_RETRIES && stream_not_taken(&status) => retries += 1,
                // the server's own admission decision, kept structured; a
                // status tonic synthesised for a transport failure has a
                // source error, and any other code (`UNIMPLEMENTED` from a URL
                // that is no Monty server) is a misconfiguration, not a refusal
                Ok(Err(status)) if Error::source(&status).is_none() && refuses_admission(status.code()) => {
                    return Err(PoolError::Refused {
                        url: url.to_owned(),
                        refusal: Box::new(RemoteDisconnect::from_status(&status)),
                    });
                }
                Ok(Err(status)) => {
                    let details = RemoteDisconnect::from_connect_status(url, &status);
                    return Err(PoolError::Connect {
                        url: url.to_owned(),
                        details: Box::new(details),
                    });
                }
                Err(_) => {
                    return Err(connection_failure(
                        url,
                        format!(
                            "the server did not accept the session within {:?} (request_timeout)",
                            self.dial_timeout
                        ),
                    ));
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
            closing_complete: Arc::clone(&self.closing_complete),
        })
    }
}

/// The largest message tonic assembles for a stream decoding under
/// `max_decode_bytes`: a frame cannot decode in less than its own size, so a
/// larger one is refused before it is buffered. Never below
/// [`MIN_MESSAGE_LIMIT`], which keeps control events through a tiny budget.
fn message_limit(max_decode_bytes: usize) -> usize {
    max_decode_bytes.clamp(MIN_MESSAGE_LIMIT, MAX_FRAME_LEN as usize)
}

/// The smallest [`message_limit`].
const MIN_MESSAGE_LIMIT: usize = 64 * 1024;

/// Whether `code` is one a server refuses a stream's admission with: no room
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

/// Whether a response's headers are a gRPC server's: the protocol requires a
/// `content-type` of `application/grpc`, optionally with a `+format` or
/// `;params` suffix, on every response.
fn is_grpc_response(metadata: &MetadataMap) -> bool {
    metadata
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
        .is_some_and(|value| {
            value
                .strip_prefix(GRPC_CONTENT_TYPE)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['+', ';']))
        })
}

/// A response's `content-type` for an error message: quoted, or `missing`.
fn describe_content_type(metadata: &MetadataMap) -> String {
    metadata.get(CONTENT_TYPE).map_or_else(
        || "missing".to_owned(),
        |value| format!("{:?}", value.to_str().unwrap_or("<non-ASCII>")),
    )
}

/// The `content-type` header, as tonic's metadata names it.
const CONTENT_TYPE: &str = "content-type";

/// The media type every gRPC response carries, before any suffix.
const GRPC_CONTENT_TYPE: &str = "application/grpc";

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
    closing_complete: Arc<Notify>,
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
    /// the checkout does not wait for the server's final work, such as parking.
    /// [`crate::Pool::close`] waits for these detached readers.
    ///
    /// The half-close is what a *proxied* server sees: a bare stream reset may
    /// stop at a load balancer, leaving the session (and its capacity slot)
    /// alive until the far side's own idle timeout. Past [`Remote::closing`]'s
    /// cap the stream is reset instead.
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

/// `User-Agent` on every stream, identifying this crate to the server or
/// relay it dials; tonic appends its own version after it.
const USER_AGENT_VALUE: &str = concat!("monty-pool/", env!("CARGO_PKG_VERSION"));

/// How long an HTTP/2 `PING` may go unanswered before the connection is
/// judged dead. Connection-level, hop-local: it detects a dead TCP peer, not
/// a vanished server beyond a proxy (that is the in-band `Ping`'s job). The
/// interval between pings is `PoolConfig::keep_alive`.
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// A stream's HTTP/2 receive window, unless the pool is too large for every
/// stream to hold this much (see [`stream_window`]).
const STREAM_WINDOW_BYTES: u32 = 2 * 1024 * 1024;

/// The smallest stream window: HTTP/2's default.
const MIN_STREAM_WINDOW_BYTES: u32 = 65_535;

/// The smallest connection window, so a small pool keeps the window it had.
const MIN_CONNECTION_WINDOW_BYTES: u32 = 32 * 1024 * 1024;

/// HTTP/2's largest flow-control window.
const MAX_WINDOW_BYTES: u32 = (1 << 31) - 1;

/// How many streams the connection window leaves room for: `max_processes`
/// live, as many closing, and one to spare for a neighbour's reply.
fn window_streams(max_processes: usize) -> u32 {
    let streams = max_processes.max(1).saturating_mul(2).saturating_add(1);
    u32::try_from(streams).unwrap_or(u32::MAX)
}

/// A stream's receive window: [`STREAM_WINDOW_BYTES`], shrunk for a pool so
/// large that every stream stalled on it would overrun HTTP/2's largest
/// connection window. Below [`MIN_STREAM_WINDOW_BYTES`] it stops shrinking, so
/// a pool past about 16,000 processes can again be starved by stalled streams.
fn stream_window(max_processes: usize) -> u32 {
    (MAX_WINDOW_BYTES / window_streams(max_processes)).clamp(MIN_STREAM_WINDOW_BYTES, STREAM_WINDOW_BYTES)
}

/// The connection's receive window, shared by every stream on the channel:
/// room for each of [`window_streams`] to sit on a full [`stream_window`], so
/// stalled sessions never take the credit a neighbour's reply needs.
fn connection_window(max_processes: usize) -> u32 {
    window_streams(max_processes)
        .checked_mul(stream_window(max_processes))
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
    let mut pings = PingLimit::new();
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
                            // no turn deadline covers an idle stream, so a ping
                            // too large or too frequent ends it instead
                            if let Err(abuse) = pings.admit(body.len()) {
                                let _ = disconnect.set(RemoteDisconnect::failure(abuse));
                                break Err(FrameError::Truncated);
                            }
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
                        retry_after: None,
                    });
                    break Err(FrameError::Truncated);
                }
                Err(status) => {
                    if let Some(err) = oversized_message(&status) {
                        let _ = disconnect.set(RemoteDisconnect::failure(err.to_string()));
                        break Err(err);
                    }
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

/// The frame error for tonic refusing a message above [`message_limit`],
/// which it reports as a local `OUT_OF_RANGE` status naming both sizes.
fn oversized_message(status: &Status) -> Option<FrameError> {
    if status.code() != Code::OutOfRange {
        return None;
    }
    let sizes = status
        .message()
        .strip_prefix("Error, decoded message length too large: found ")?
        .strip_suffix(" bytes")?;
    let (len, max) = sizes.split_once(" bytes, the limit is: ")?;
    Some(FrameError::FrameTooLarge {
        len: len.parse().unwrap_or(u32::MAX),
        max: max.parse().ok()?,
    })
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
    /// Adds a TLS hint to an HTTP/2 framing failure on a plaintext URL.
    fn from_connect_status(url: &str, status: &Status) -> Self {
        let mut details = Self::from_status(status);
        if url
            .split_once("://")
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("ws"))
            && let Some(error) = &mut details.transport_error
        {
            let mut source = Error::source(status);
            while let Some(cause) = source {
                if cause
                    .downcast_ref::<h2::Error>()
                    .is_some_and(|err| err.reason() == Some(Reason::FRAME_SIZE_ERROR))
                {
                    error.push_str("; the server may require https:// instead of http://");
                    break;
                }
                source = cause.source();
            }
        }
        details
    }

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
                retry_after: status
                    .metadata()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| retry_after(value, SystemTime::now())),
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
            retry_after: None,
        }
    }
}

/// Parses an HTTP delay or date without retaining invalid server metadata.
fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        value.parse().ok().map(Duration::from_secs)
    } else {
        httpdate::parse_http_date(value)
            .ok()
            .map(|date| date.duration_since(now).unwrap_or_default())
    }
}

/// Records a connection failure for which the server sent no gRPC status.
fn connection_failure(url: &str, message: impl Into<String>) -> PoolError {
    PoolError::Connect {
        url: url.to_owned(),
        details: Box::new(RemoteDisconnect::failure(message)),
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
/// in plaintext, metadata and all. A path (other than `/`), query or fragment
/// is refused too: tonic keeps only the scheme and authority, so it would be
/// dropped without a word.
fn dial_url(url: &str) -> Result<String, PoolError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| connection_failure(url, "invalid URI"))?;
    let normalised = if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("ws") {
        "http"
    } else if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") {
        "https"
    } else {
        return Err(connection_failure(
            url,
            format!("unsupported URL scheme {scheme:?}, expected http or https"),
        ));
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if !matches!(&rest[authority_end..], "" | "/") {
        return Err(connection_failure(
            url,
            "a URL path, query or fragment is not supported; use only the scheme and host, with an optional port",
        ));
    }
    Ok(format!("{normalised}://{rest}"))
}

/// Installs the process-level rustls `CryptoProvider` exactly once before the
/// first `https://` dial and returns the one in force, for the verifier to
/// check signatures with. rustls 0.23 panics on first TLS use when it can't
/// pick a provider automatically (both `aws-lc-rs` and `ring`, or neither,
/// compiled in), so we name `aws_lc_rs` explicitly. Idempotent via `Once`, and
/// the install error is ignored: another part of the process (e.g. a host
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
#[path = "../tests/unit/remote.rs"]
mod tests;
