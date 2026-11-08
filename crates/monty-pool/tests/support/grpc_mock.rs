//! An in-process gRPC server standing in for a remote child, scripted from a
//! plain thread: each session stream the pool opens becomes a [`MockStream`]
//! with blocking `read` / `send` / `close`, so a test spells out the server
//! side of a conversation step by step while the pool under test runs on the
//! tokio runtime. The service is hand-written over `BytesCodec` — the same
//! path and codec the pool dials — so nothing here depends on generated code.

use std::{
    convert::Infallible,
    future::Future,
    mem,
    pin::Pin,
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError},
    },
    task::{Context, Poll},
    thread,
    time::{Duration, Instant},
};

use bytes::Bytes;
use monty_proto::{
    MAX_FRAME_LEN, decode_frame, encode_to_capped_vec,
    grpc::{BytesCodec, SESSION_PATH},
    pb,
};
use rcgen::generate_simple_self_signed;
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{Sender as EventSender, channel as event_channel, error::TrySendError},
        oneshot,
    },
    task::JoinHandle,
};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{
    Request, Response, Status, Streaming,
    body::Body,
    metadata::MetadataMap,
    server::{Grpc, NamedService, StreamingService},
    transport::{Identity, Server, ServerTlsConfig},
};
use tower::Service;

/// How long [`MockServer::accept`] and [`MockStream::read`] wait before
/// declaring the test stuck: generous for CI, far below a hung test's cost.
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// A listening mock server. Dropping it stops the server.
pub struct MockServer {
    url: String,
    /// Streams the service admitted, in order, for scripts to `accept`.
    incoming: mpsc::Receiver<MockStream>,
    /// A status the next stream is refused with, before any event.
    refuse: Arc<Mutex<Option<Status>>>,
    task: JoinHandle<()>,
    shutdown: Option<oneshot::Sender<()>>,
    /// The forwarder in front of the server, when started with one.
    proxy: Option<Proxy>,
}

impl MockServer {
    /// Binds an ephemeral loopback port and serves the session RPC on it.
    pub async fn start() -> Self {
        Self::start_with(None).await
    }

    /// Serves over TLS with a certificate it signed itself, which no trust
    /// store knows: a pool that verifies certificates must refuse it. The
    /// certificate names the address the pool dials, so only trust can fail.
    pub async fn start_self_signed() -> Self {
        let certified = generate_simple_self_signed(["127.0.0.1".to_owned()]).expect("self-signed certificate");
        let identity = Identity::from_pem(certified.cert.pem(), certified.signing_key.serialize_pem());
        Self::start_with(Some(ServerTlsConfig::new().identity(identity))).await
    }

    /// Binds an ephemeral loopback port and serves the session RPC on it,
    /// over TLS when given a server identity.
    async fn start_with(tls: Option<ServerTlsConfig>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, incoming) = mpsc::channel();
        let refuse = Arc::new(Mutex::new(None));
        let service = MockService {
            incoming: tx,
            refuse: Arc::clone(&refuse),
        };
        let scheme = if tls.is_some() { "https" } else { "http" };
        let mut builder = Server::builder();
        if let Some(tls) = tls {
            builder = builder.tls_config(tls).expect("server TLS config");
        }
        let (shutdown, signal) = oneshot::channel();
        let task = tokio::spawn(async move {
            let serve =
                builder
                    .add_service(service)
                    .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                        let _ = signal.await;
                    });
            let _ = serve.await;
        });
        Self {
            url: format!("{scheme}://127.0.0.1:{port}"),
            incoming,
            refuse,
            task,
            shutdown: Some(shutdown),
            proxy: None,
        }
    }

    /// Starts the server behind a byte-level TCP forwarder, so a test can
    /// [`cut`](Self::cut) the connection under an open stream — the one
    /// failure a well-behaved server cannot produce on its own.
    pub async fn start_proxied() -> Self {
        let mut server = Self::start().await;
        let upstream = server.url.trim_start_matches("http://").to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let port = listener.local_addr().expect("addr").port();
        let connections = Arc::new(Mutex::new(Vec::new()));
        let forwarded = Arc::clone(&connections);
        let accept = tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let upstream = upstream.clone();
                let task = tokio::spawn(async move {
                    if let Ok(mut server) = TcpStream::connect(upstream).await {
                        let _ = copy_bidirectional(&mut client, &mut server).await;
                    }
                });
                forwarded.lock().expect("proxy lock").push(task);
            }
        });
        server.url = format!("http://127.0.0.1:{port}");
        server.proxy = Some(Proxy { accept, connections });
        server
    }

    /// Severs every connection through the forwarder (see
    /// [`start_proxied`](Self::start_proxied)): both ends see a bare TCP
    /// close, with no status or stream end sent by anyone.
    pub fn cut(&self) {
        let proxy = self.proxy.as_ref().expect("a proxied server");
        proxy.accept.abort();
        for task in proxy.connections.lock().expect("proxy lock").drain(..) {
            task.abort();
        }
    }

    /// The URL a pool dials to reach this server.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Blocks until the pool opens the next session stream. Panics if none
    /// arrives within [`WAIT_TIMEOUT`]: a test that never gets its stream is
    /// stuck, not slow.
    pub fn accept(&self) -> MockStream {
        match self.incoming.recv_timeout(WAIT_TIMEOUT) {
            Ok(stream) => stream,
            Err(RecvTimeoutError::Timeout) => panic!("no session stream was opened within {WAIT_TIMEOUT:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("the mock server stopped before a stream was opened"),
        }
    }

    /// The next session stream if the pool opens one within `wait`; a test
    /// asserting the pool holds back expects `None`.
    pub fn try_accept(&self, wait: Duration) -> Option<MockStream> {
        self.incoming.recv_timeout(wait).ok()
    }

    /// Refuses the next stream with `status` before admitting it — what a
    /// server at capacity, or one rejecting the caller's credentials, does.
    pub fn refuse_next(&self, status: Status) {
        *self.refuse.lock().expect("refusal lock") = Some(status);
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // graceful shutdown waits for open streams, which a script that
        // finished may well have left open; the test is over either way
        self.task.abort();
        if self.proxy.is_some() {
            self.cut();
        }
    }
}

/// The forwarder's accept loop and the connections it has paired so far.
struct Proxy {
    accept: JoinHandle<()>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

/// The server side of one session stream, driven synchronously. Dropping it
/// ends the stream with an `OK` status, as a server that simply finished.
///
/// A `Configure` carrying a first request reads as the `Configure` and then
/// the request alone, so a script answers the two in turn: an `Ok` to the
/// `Configure` is held back and merged into the request's reply, while any
/// other answer is the refusal and drops the request (see [`Opening`]).
pub struct MockStream {
    /// Request messages from the pool; `None` once the pool half-closed or
    /// cancelled the stream.
    requests: mpsc::Receiver<Option<Bytes>>,
    /// Event messages to the pool; an `Err` ends the stream with that status.
    events: EventSender<Result<Bytes, Status>>,
    /// The request metadata the stream was opened with.
    pub metadata: MetadataMap,
    read_timeout: Duration,
    /// A `Configure` carrying a first request, as it is presented to the script.
    opening: Mutex<Opening>,
}

/// The split of a `Configure` carrying a first request into the `Configure`
/// and the request, and the merge of their two answers into one reply.
#[derive(Default)]
struct Opening {
    /// The request behind a handed-out `Configure`, for the next `read`.
    request: Option<Bytes>,
    /// Set from handing out the `Configure` until the script answers it.
    awaiting_reply: bool,
    /// The `Configure`'s `Ok`, whose fields the request's reply takes on.
    ack: Option<pb::ChildEvent>,
}

impl MockStream {
    /// Blocks for the next request message; `None` once the pool ended its
    /// side. Panics after the read timeout (see [`Self::set_read_timeout`]).
    pub fn read(&mut self) -> Option<Bytes> {
        if let Some(request) = self.opening.get_mut().unwrap().request.take() {
            return Some(request);
        }
        let message = self.read_unsplit()?;
        let opening = self.opening.get_mut().unwrap();
        let mut request = decode_frame::<pb::ParentRequest>(message.as_ref()).expect("decode request");
        let Some(pb::parent_request::Kind::Configure(configure)) = &mut request.kind else {
            return Some(message);
        };
        let first = match configure.first.take() {
            Some(pb::configure::First::Feed(feed)) => pb::parent_request::Kind::Feed(feed),
            Some(pb::configure::First::Load(load)) => pb::parent_request::Kind::Load(load),
            None => return Some(message),
        };
        let first = pb::ParentRequest {
            kind: Some(first),
            ..pb::ParentRequest::default()
        };
        opening.request = Some(encode_to_capped_vec(&first).expect("encode request").into());
        opening.awaiting_reply = true;
        Some(encode_to_capped_vec(&request).expect("encode Configure").into())
    }

    /// Blocks for the next request message as the pool sent it, a `Configure`
    /// carrying a first request included; `None` once the pool ended its side.
    pub fn read_unsplit(&mut self) -> Option<Bytes> {
        match self.requests.recv_timeout(self.read_timeout) {
            Ok(message) => message,
            Err(RecvTimeoutError::Disconnected) => None,
            Err(RecvTimeoutError::Timeout) => panic!("no request arrived within {:?}", self.read_timeout),
        }
    }

    /// Bounds every later `read`: a test asserting the pool stays quiet, or
    /// one that must fail fast rather than hang, lowers it.
    pub fn set_read_timeout(&mut self, timeout: Duration) {
        self.read_timeout = timeout;
    }

    /// Sends one event message. Fails once the pool has dropped the stream.
    /// Runtime-agnostic (a script may run inside its own `block_on`), so a
    /// full channel is retried rather than awaited.
    pub fn send(&self, body: Bytes) -> Result<(), ()> {
        let mut opening = self.opening.lock().unwrap();
        if !opening.awaiting_reply && opening.ack.is_none() {
            drop(opening);
            return self.deliver(Ok(body));
        }
        let mut event = decode_frame::<pb::ChildEvent>(body.as_ref()).expect("decode event");
        if matches!(
            event.kind,
            Some(pb::child_event::Kind::Print(_) | pb::child_event::Kind::Ping(_))
        ) {
            // not a reply: sent as written, since re-encoding drops unknown fields
            drop(opening);
            return self.deliver(Ok(body));
        }
        if mem::take(&mut opening.awaiting_reply) {
            if matches!(event.kind, Some(pb::child_event::Kind::Ok(_))) {
                opening.ack = Some(event);
                return Ok(());
            }
            // the `Configure` was refused, so the request behind it never runs
            opening.request = None;
            if matches!(event.kind, Some(pb::child_event::Kind::Error(_))) {
                event.session_refused = Some(true);
            }
        } else if let Some(ack) = opening.ack.take() {
            event.execution_clock = event.execution_clock.or(ack.execution_clock);
            event.session_id = event.session_id.or(ack.session_id);
            event.max_suspensions = event.max_suspensions.or(ack.max_suspensions);
            event.max_feed_duration_micros = event.max_feed_duration_micros.or(ack.max_feed_duration_micros);
            event.max_turn_duration_micros = event.max_turn_duration_micros.or(ack.max_turn_duration_micros);
            event.max_total_sleep_micros = event.max_total_sleep_micros.or(ack.max_total_sleep_micros);
        }
        drop(opening);
        self.deliver(Ok(encode_to_capped_vec(&event).expect("encode event").into()))
    }

    /// Queues one item for the response stream, retrying a full channel until
    /// [`WAIT_TIMEOUT`]; fails once the pool has dropped the stream.
    fn deliver(&self, mut message: Result<Bytes, Status>) -> Result<(), ()> {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            match self.events.try_send(message) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Closed(_)) => return Err(()),
                Err(TrySendError::Full(returned)) => {
                    assert!(Instant::now() < deadline, "the pool stopped reading events");
                    message = returned;
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    /// Ends the stream: with `status`, as a server dropping the session by
    /// policy; without, as one that finished (an `OK` status).
    pub fn close(self, status: Option<Status>) {
        if let Some(status) = status {
            let _ = self.deliver(Err(status));
        }
    }
}

/// The tower service tonic's server routes `MontySession` to.
#[derive(Clone)]
struct MockService {
    incoming: mpsc::Sender<MockStream>,
    refuse: Arc<Mutex<Option<Status>>>,
}

impl NamedService for MockService {
    const NAME: &'static str = "monty.v1.MontySession";
}

impl Service<http::Request<Body>> for MockService {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        let handler = self.clone();
        Box::pin(async move {
            if request.uri().path() != SESSION_PATH {
                return Ok(Status::unimplemented("not the session RPC").into_http());
            }
            let mut grpc = Grpc::new(BytesCodec)
                .max_decoding_message_size(MAX_FRAME_LEN as usize)
                .max_encoding_message_size(MAX_FRAME_LEN as usize);
            Ok(grpc.streaming(handler, request).await)
        })
    }
}

impl StreamingService<Bytes> for MockService {
    type Response = Bytes;
    type ResponseStream = ReceiverStream<Result<Bytes, Status>>;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Self::ResponseStream>, Status>> + Send>>;

    fn call(&mut self, request: Request<Streaming<Bytes>>) -> Self::Future {
        let refusal = self.refuse.lock().expect("refusal lock").take();
        let incoming = self.incoming.clone();
        Box::pin(async move {
            if let Some(status) = refusal {
                return Err(status);
            }
            let metadata = request.metadata().clone();
            let (events, outbound) = event_channel(8);
            let (requests_tx, requests) = mpsc::channel();
            tokio::spawn(pump_requests(request.into_inner(), requests_tx));
            let stream = MockStream {
                requests,
                events,
                metadata,
                read_timeout: WAIT_TIMEOUT,
                opening: Mutex::new(Opening::default()),
            };
            // a dropped server means the test is over; the stream then just
            // sits unanswered until the pool gives up on it
            let _ = incoming.send(stream);
            Ok(Response::new(ReceiverStream::new(outbound)))
        })
    }
}

/// Forwards the pool's request messages to the script's channel, closing it
/// with `None` when the pool ends (or resets) the stream.
async fn pump_requests(mut inbound: Streaming<Bytes>, requests: mpsc::Sender<Option<Bytes>>) {
    loop {
        if let Ok(Some(body)) = inbound.message().await {
            if requests.send(Some(body)).is_err() {
                return;
            }
        } else {
            let _ = requests.send(None);
            return;
        }
    }
}
