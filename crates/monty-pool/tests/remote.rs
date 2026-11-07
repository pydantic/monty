//! Drives the pool's remote transport against a mock gRPC server: a thread
//! that accepts one session stream and serves a scripted protocol session.
//! This exercises the pool's channel, the per-checkout stream and the
//! send/recv path end-to-end without needing a real remote child.

#[cfg(target_os = "linux")]
mod binary;
#[path = "support/grpc_mock.rs"]
mod grpc_mock;

#[cfg(feature = "telemetry")]
use std::sync::Arc;
use std::{
    fs,
    future::ready,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use bytes::Bytes;
use grpc_mock::{MockServer, MockStream};
use h2::server::handshake;
use http::{Response as HttpResponse, StatusCode};
#[cfg(feature = "telemetry")]
use monty_pool::telemetry::{TelemetryAdapter, configure_telemetry_adapter};
use monty_pool::{
    Checkout, CheckoutOptions, MountSpec, MountSpecMode, Persistence, Pool, PoolConfig, PoolError, PrintFuture,
    RemoteCause, RemoteDisconnect, ReplConfig, ResumeValue, TurnEvent,
};
use monty_proto::{
    MAX_FRAME_LEN, WireFunctionCall, decode_frame, encode_to_capped_vec, grpc::Cause, pb, resume_call_from_proto,
};
// only the Linux CPU-server test names the clock
#[cfg(target_os = "linux")]
use monty_types::ExecutionClock;
use monty_types::{CallArgs, ExtFunctionResult, MontyObject, PrintStream, ResourceLimits, SourceRange};
#[cfg(feature = "telemetry")]
use opentelemetry::trace::{SpanId, TraceId};
#[cfg(feature = "telemetry")]
use opentelemetry_sdk::{logs::SdkLogRecord, trace::SpanData};
use tokio::{net::TcpListener, runtime::Builder, sync::oneshot, task::spawn_blocking, time::timeout};
use tonic::{Code, Status};

/// Closing a remote transport cannot prove that the remote subprocess has exited.
#[tokio::test]
async fn terminating_a_remote_checkout_never_confirms_subprocess_exit() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let child = thread::spawn(move || serve_mock_child(&mock));
    let pool = Pool::new(PoolConfig::remote(&url)).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert!(
        matches!(session.feed("6 * 7", vec![], vec![], false, &mut no_print).await.unwrap(), TurnEvent::Complete(value) if value == MontyObject::int(42))
    );
    assert!(session.terminate().await.is_none());
    spawn_blocking(move || child.join().unwrap()).await.unwrap();
    pool.close().await;
}

/// A mock child: accepts one session stream and answers each request with
/// the obvious turn-ender (`Ok` for control requests, `Complete(42)` for a feed).
fn serve_mock_child(server: &MockServer) {
    let mut socket = server.accept();
    answer_requests(&mut socket);
}

/// Answers each request on an accepted stream with the obvious turn-ender
/// (`Ok` for control requests, `Complete(42)` for a feed) until the peer
/// ends the stream.
fn answer_requests(socket: &mut MockStream) {
    while let Some(data) = socket.read() {
        let request = decode_frame::<pb::ParentRequest>(data.as_ref()).expect("decode request");
        let kind = match request.kind.expect("request kind") {
            pb::parent_request::Kind::Feed(_) => {
                pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42)))
            }
            // Configure / Reset / Shutdown / anything else: acknowledge.
            _ => pb::child_event::Kind::Ok(pb::Ok {}),
        };
        let event = pb::ChildEvent {
            kind: Some(kind),
            ..Default::default()
        };
        let body = encode_to_capped_vec(&event).expect("encode event");
        socket.send(body.into()).expect("send event");
    }
}

/// A mock child that also reports each of `headers`' values (in order, `None`
/// when absent) from the request metadata of each of `connections` streams.
fn serve_capturing_headers(
    server: &MockServer,
    headers: &[&str],
    connections: usize,
    tx: &mpsc::Sender<Vec<Option<String>>>,
) {
    for _ in 0..connections {
        let mut socket = server.accept();
        let seen = headers
            .iter()
            .map(|header| {
                socket
                    .metadata
                    .get(*header)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            })
            .collect();
        tx.send(seen).expect("send captured headers");
        answer_requests(&mut socket);
    }
}

/// A telemetry adapter accepting every record, for tests that only care what
/// an installed adapter makes the pool do (e.g. the headers it dials with).
#[cfg(feature = "telemetry")]
struct AcceptAll;

#[cfg(feature = "telemetry")]
impl TelemetryAdapter for AcceptAll {
    fn start_span(&self, _: &SpanData) -> bool {
        true
    }
    fn end_span(&self, _: &SpanData) -> bool {
        true
    }
    fn emit_log(&self, _: SpanId, _: &SdkLogRecord) -> bool {
        true
    }
    fn disable_root(&self, _: TraceId, _: SpanId) {}
}

/// A discard-everything print callback, coercible to `OnPrint` at each callsite.
fn no_print(_: PrintStream, _: &str) -> PrintFuture {
    Box::pin(ready(()))
}

/// Joins a mock-server thread without blocking the runtime thread.
///
/// A plain `server.join()` inside the test future would block the runtime
/// thread, deadlocking a server that still needs the client side serviced.
async fn join_server(server: thread::JoinHandle<()>) {
    spawn_blocking(move || server.join().expect("mock server thread"))
        .await
        .expect("join task");
}

#[tokio::test]
async fn drives_a_session_over_grpc() {
    // Start before spawning so the server is listening before the pool connects.
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || serve_mock_child(&mock));

    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    let mut checkout = pool
        .checkout(&ReplConfig {
            script_name: "test.py".to_owned(),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");

    // The remote worker has no local pid.
    assert_eq!(checkout.pid(), None);

    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
        "got {event:?}"
    );

    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// Each checkout's `CheckoutOptions::metadata` reach that checkout's
/// own stream's metadata. Remote workers are single-use, so two checkouts
/// are two dials, and a `traceparent` rendered at checkout time — the
/// motivating case — must not land on the wrong one.
#[tokio::test]
async fn metadata_are_per_checkout() {
    let (header_tx, header_rx) = mpsc::channel();
    let (mock, mut config) = mock_pool_config().await;
    let server = thread::spawn(move || serve_capturing_headers(&mock, &["traceparent"], 2, &header_tx));

    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    for trace in ["00-checkout-1", "00-checkout-2"] {
        let repl = ReplConfig {
            script_name: "test.py".to_owned(),
            ..ReplConfig::default()
        };
        let options = CheckoutOptions::default().with_metadata(vec![("traceparent".to_owned(), trace.to_owned())]);
        let mut checkout = pool.checkout_with(&repl, options).await.expect("checkout");
        assert_eq!(
            header_rx.recv().expect("captured headers"),
            vec![Some(trace.to_owned())],
            "each dial must carry the headers of the checkout that made it"
        );
        // the session still works normally with extra headers on the dial
        let event = checkout
            .feed("1 + 1", vec![], vec![], false, &mut no_print)
            .await
            .expect("feed");
        assert!(
            matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
            "got {event:?}"
        );
        checkout.finish().await.expect("finish");
    }
    join_server(server).await;
}

/// Duplicate header names are last-wins, as [`CheckoutOptions`] promises.
#[tokio::test]
async fn duplicate_metadata_are_last_wins() {
    let (header_tx, header_rx) = mpsc::channel();
    let (mock, mut config) = mock_pool_config().await;
    let server = thread::spawn(move || serve_capturing_headers(&mock, &["traceparent"], 1, &header_tx));
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    let options = CheckoutOptions::default().with_metadata(vec![
        ("traceparent".to_owned(), "first".to_owned()),
        ("traceparent".to_owned(), "second".to_owned()),
    ]);
    let checkout = pool
        .checkout_with(&ReplConfig::default(), options)
        .await
        .expect("checkout");
    assert_eq!(
        header_rx.recv().expect("captured headers"),
        vec![Some("second".to_owned())]
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// Every stream carries a `user-agent` naming this crate and its version
/// (tonic appends its own), and a `metadata` entry of the same name is
/// put in front of it, so a caller can identify their own product first.
#[tokio::test]
async fn user_agent_is_set_and_overridable() {
    let (header_tx, header_rx) = mpsc::channel();
    let (mock, mut config) = mock_pool_config().await;
    let server = thread::spawn(move || serve_capturing_headers(&mock, &["user-agent"], 2, &header_tx));
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let agent = header_rx
        .recv()
        .expect("captured headers")
        .remove(0)
        .expect("no user-agent");
    assert!(
        agent.starts_with(&format!("monty-pool/{} tonic/", env!("CARGO_PKG_VERSION"))),
        "got {agent}"
    );
    checkout.finish().await.expect("finish");

    let options = CheckoutOptions::default().with_metadata(vec![("user-agent".to_owned(), "my-app/1.0".to_owned())]);
    let checkout = pool
        .checkout_with(&ReplConfig::default(), options)
        .await
        .expect("checkout");
    let agent = header_rx
        .recv()
        .expect("captured headers")
        .remove(0)
        .expect("no user-agent");
    assert!(agent.starts_with("my-app/1.0 monty-pool/"), "got {agent}");
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A host trace context captured through the telemetry adapter goes out as
/// W3C `traceparent`/`tracestate` on the dial, so a remote worker's spans join
/// the host's trace with no header plumbing by the binding; an explicit
/// `metadata` entry still wins.
#[cfg(feature = "telemetry")]
#[tokio::test]
async fn telemetry_context_is_propagated_on_the_dial() {
    let handle = configure_telemetry_adapter(Arc::new(AcceptAll)).expect("configure");
    let context = || {
        handle
            .context(
                "0af7651916cd43dd8448eb211c80319c",
                "b7ad6b7169203331",
                1,
                "congo=t61rcWkgMzE",
            )
            .expect("context")
    };
    let (header_tx, header_rx) = mpsc::channel();
    let (mock, mut config) = mock_pool_config().await;
    let server = thread::spawn(move || serve_capturing_headers(&mock, &["traceparent", "tracestate"], 2, &header_tx));
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    let options = CheckoutOptions::default().with_telemetry(Some(context()));
    let checkout = pool
        .checkout_with(&ReplConfig::default(), options)
        .await
        .expect("checkout");
    assert_eq!(
        header_rx.recv().expect("captured headers"),
        vec![
            Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned()),
            Some("congo=t61rcWkgMzE".to_owned()),
        ]
    );
    checkout.finish().await.expect("finish");

    let options = CheckoutOptions::default()
        .with_telemetry(Some(context()))
        .with_metadata(vec![("traceparent".to_owned(), "00-explicit".to_owned())]);
    let checkout = pool
        .checkout_with(&ReplConfig::default(), options)
        .await
        .expect("checkout");
    assert_eq!(
        header_rx.recv().expect("captured headers"),
        vec![Some("00-explicit".to_owned()), Some("congo=t61rcWkgMzE".to_owned())]
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A malformed header name or value is validated before any I/O, so no server
/// is needed: the checkout fails with a configuration error instead
/// of the pair being silently dropped.
#[tokio::test]
async fn malformed_metadata_are_configuration_errors() {
    let cases = [
        (
            ("bad name", "v"),
            "http://127.0.0.1:9: metadata entry \"bad name\": invalid gRPC metadata key name",
        ),
        (
            ("traceparent", "line\nbreak"),
            "http://127.0.0.1:9: metadata entry \"traceparent\" value: failed to parse metadata value",
        ),
    ];
    let pool = Pool::new(PoolConfig::remote("http://127.0.0.1:9")).await.expect("pool");
    for ((name, value), expected) in cases {
        let options = CheckoutOptions::default().with_metadata(vec![(name.to_owned(), value.to_owned())]);
        let Err(err) = pool.checkout_with(&ReplConfig::default(), options).await else {
            panic!("malformed header must fail the checkout");
        };
        let PoolError::Configuration(msg) = err else {
            panic!("expected a configuration error, got {err:?}");
        };
        assert_eq!(msg, expected);
    }
}

/// `request_timeout` bounds opening a stream too: a server that takes the TCP
/// connection but never speaks HTTP/2 fails the checkout at that deadline.
#[tokio::test]
async fn request_timeout_bounds_opening_the_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/", listener.local_addr().expect("addr"));
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });

    let mut config = PoolConfig::remote(&url);
    config.request_timeout = Some(Duration::from_millis(200));
    let pool = Pool::new(config).await.expect("pool");
    let start = Instant::now();
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("a server that never answers cannot admit a stream");
    };
    assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
    let message = connect_message(&err);
    assert_eq!(
        message,
        format!("{url}: the server did not accept the session within 200ms (request_timeout)")
    );
    silent.abort();
}

/// An `https://` server is verified against the operating system's trust
/// store, not a root list bundled into the pool: a certificate the server
/// signed itself is refused, and the checkout reports the dial failure.
#[tokio::test]
async fn an_untrusted_server_certificate_fails_the_dial() {
    let server = MockServer::start_self_signed().await;
    let url = server.url();
    let pool = Pool::new(PoolConfig::remote(&url)).await.expect("pool");
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("a self-signed certificate must not be trusted");
    };
    let message = connect_message(&err);
    // Each platform's verifier words the refusal in its own way; tonic's chain
    // then repeats the cause at every layer of the transport error.
    let cause = if cfg!(target_vendor = "apple") {
        "invalid peer certificate: Other(OtherError(\"“rcgen self signed cert” certificate is not trusted: -67843\"))"
    } else {
        "invalid peer certificate: UnknownIssuer"
    };
    assert_eq!(
        message,
        format!("{url}: UNAVAILABLE: {cause}: transport error: {cause}: {cause}")
    );
}

/// A URL tonic cannot parse fails pool creation with the same `{url}: {err}`
/// shape as every other dial failure — before any checkout, since the channel
/// is built once for the pool.
#[tokio::test]
async fn an_unparsable_url_fails_the_pool() {
    let Err(err) = Pool::new(PoolConfig::remote("not a url")).await else {
        panic!("unparsable URL must fail pool creation");
    };
    let msg = connect_message(&err);
    assert_eq!(msg, "not a url: invalid URI");
}

/// Schemes are read without regard to case, `ws`/`wss` as `http`/`https`,
/// and anything else is refused when the pool is made: tonic would dial an
/// unknown scheme in plaintext, with the stream's metadata on it.
#[tokio::test]
async fn url_schemes_are_normalised_and_unknown_ones_refused() {
    for scheme in ["HTTP://", "WS://", "ws://"] {
        let mock = MockServer::start().await;
        let url = mock.url().replace("http://", scheme);
        let server = thread::spawn(move || serve_mock_child(&mock));
        let pool = Pool::new(PoolConfig::remote(&url)).await.expect("pool");
        let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
        let event = checkout
            .feed("1 + 1", vec![], vec![], false, &mut no_print)
            .await
            .expect("feed");
        assert!(matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)));
        checkout.finish().await.expect("finish");
        join_server(server).await;
    }
    for url in ["grpc://127.0.0.1:9", "WSS+tls://127.0.0.1:9"] {
        let Err(err) = Pool::new(PoolConfig::remote(url)).await else {
            panic!("{url} must fail pool creation");
        };
        let msg = connect_message(&err);
        let scheme = url.split_once("://").unwrap().0;
        assert_eq!(
            msg,
            format!("{url}: unsupported URL scheme {scheme:?}, expected http or https")
        );
    }
}

/// tonic dials only the scheme and authority, so a URL naming a path, query or
/// fragment fails pool creation rather than dropping it silently; a bare `/`
/// is the root and fine.
#[tokio::test]
async fn url_paths_are_refused() {
    for url in [
        "http://127.0.0.1:9/relay",
        "http://127.0.0.1:9/?x=1",
        "http://127.0.0.1:9#a",
    ] {
        let Err(err) = Pool::new(PoolConfig::remote(url)).await else {
            panic!("{url} must fail pool creation");
        };
        let msg = connect_message(&err);
        assert_eq!(
            msg,
            format!(
                "{url}: a URL path, query or fragment is not supported; use only the scheme and host, with an optional port"
            )
        );
    }
    for url in ["http://127.0.0.1:9", "http://127.0.0.1:9/"] {
        Pool::new(PoolConfig::remote(url)).await.expect("a root URL is fine");
    }
}

/// A server that refuses the stream before admitting it — at capacity, or
/// rejecting the caller — fails the checkout with its status kept structured,
/// so the caller acts on the code and cause rather than the message, and never
/// with a worker that then fails its first turn.
#[tokio::test]
async fn a_refused_stream_fails_the_checkout() {
    let (mock, config) = mock_pool_config().await;
    mock.refuse_next(Cause::CallerCap.status("caller holds 10 sessions already"));
    let pool = Pool::new(config).await.expect("pool");
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("a refused stream must fail the checkout");
    };
    assert_eq!(
        err.to_string(),
        format!(
            "{} refused the session: RESOURCE_EXHAUSTED (caller_cap): caller holds 10 sessions already",
            mock.url()
        )
    );
    let PoolError::Refused { url, refusal } = err else {
        panic!("expected Refused error, got {err:?}");
    };
    assert_eq!(url, mock.url());
    assert_eq!(refusal.status_code, Some("RESOURCE_EXHAUSTED"));
    assert_eq!(
        refusal.status_message.as_deref(),
        Some("caller holds 10 sessions already")
    );
    assert_eq!(refusal.cause, Some(RemoteCause::CallerCap));
    assert_eq!(refusal.transport_error, None);
    // the refusal consumed no capacity: the next checkout is admitted
    let server = thread::spawn(move || serve_mock_child(&mock));
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// Retry hints are independent of the text and never make credential errors retryable.
#[tokio::test]
async fn refusal_retry_details_are_preserved() {
    for (code, retryable) in [
        (Code::ResourceExhausted, true),
        (Code::Unavailable, true),
        (Code::Unauthenticated, false),
        (Code::PermissionDenied, false),
        (Code::FailedPrecondition, false),
    ] {
        for (header, delay) in [
            (Some("12"), Some(Duration::from_secs(12))),
            (Some("invalid"), None),
            (None, None),
        ] {
            let (mock, config) = mock_pool_config().await;
            let mut status = Status::new(code, "session unavailable");
            if let Some(header) = header {
                status.metadata_mut().insert("retry-after", header.parse().unwrap());
            }
            mock.refuse_next(status);
            let pool = Pool::new(config).await.unwrap();
            let err = pool.checkout(&ReplConfig::default()).await.err().unwrap();
            let PoolError::Refused { refusal, .. } = err else {
                panic!("expected refusal, got {err:?}");
            };
            assert_eq!(refusal.retry_after, delay);
            assert_eq!(refusal.retryable(), retryable);
            pool.close().await;
        }
    }
}

#[test]
fn disconnect_messages_include_the_recorded_reason() {
    let status = RemoteDisconnect {
        status_code: Some("DEADLINE_EXCEEDED"),
        status_message: Some("the turn exceeded its limit".to_owned()),
        cause: Some(RemoteCause::TurnTimeout),
        transport_error: None,
        retry_after: None,
    };
    insta::assert_snapshot!(PoolError::Disconnected {
        context: "waiting for a reply".to_owned(), remote: Some(Box::new(status))
    }.to_string(), @"monty worker connection closed while waiting for a reply: DEADLINE_EXCEEDED (turn_timeout): the turn exceeded its limit");
    let transport = RemoteDisconnect {
        status_code: None,
        status_message: None,
        cause: None,
        transport_error: Some("connection reset by peer".to_owned()),
        retry_after: None,
    };
    insta::assert_snapshot!(PoolError::Disconnected {
        context: "waiting for a reply".to_owned(), remote: Some(Box::new(transport))
    }.to_string(), @"monty worker connection closed while waiting for a reply: connection reset by peer");
}

/// Only an admission code is a refusal: a status such as `UNIMPLEMENTED`, from
/// a URL that is no Monty server, is a misconfiguration, so it fails as `Connect`
/// and a retry loop on `Refused` does not spin on it.
#[tokio::test]
async fn an_unexpected_status_is_a_connect_failure() {
    let (mock, config) = mock_pool_config().await;
    mock.refuse_next(Status::unimplemented("unknown service"));
    let pool = Pool::new(config).await.expect("pool");
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("an unimplemented session RPC must fail the checkout");
    };
    let PoolError::Connect { ref details, .. } = err else {
        panic!("expected Connect error, got {err:?}");
    };
    assert_eq!(details.status_code, Some("UNIMPLEMENTED"));
    let message = err.to_string();
    assert_eq!(message, format!("{}: UNIMPLEMENTED: unknown service", mock.url()));
}

/// An HTTP/2 server without gRPC support, such as a web server's error page,
/// fails the checkout as `Connect`, rather than opening a session whose
/// first turn then dies decoding the page as a message.
#[tokio::test]
async fn a_non_grpc_server_is_a_connect_failure() {
    for (status, content_type, described) in [
        (405, Some("text/html"), "\"text/html\""),
        (200, Some("application/grpc-web"), "\"application/grpc-web\""),
        (200, None, "missing"),
    ] {
        let url = serve_plain_http2(status, content_type).await;
        let mut config = PoolConfig::remote(url.clone());
        config.max_processes = 1;
        let pool = Pool::new(config).await.expect("pool");
        let Err(err) = pool.checkout(&ReplConfig::default()).await else {
            panic!("a {status} {content_type:?} response must fail the checkout");
        };
        let message = connect_message(&err);
        assert_eq!(
            message,
            format!(
                "{url}: not a gRPC server: HTTP {}, the response's content-type is {described}",
                StatusCode::from_u16(status).unwrap()
            )
        );
    }
}

/// Serves h2c on an ephemeral loopback port, answering every request with
/// `status`, `content_type` and a small HTML body, as a web server does; returns its URL.
async fn serve_plain_http2(status: u16, content_type: Option<&'static str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut connection) = handshake(socket).await else {
                    return;
                };
                // polling `accept` also drives the connection's frames
                while let Some(Ok((_request, mut respond))) = connection.accept().await {
                    let mut response = HttpResponse::builder().status(status);
                    if let Some(content_type) = content_type {
                        response = response.header("content-type", content_type);
                    }
                    let response = response.body(()).expect("response");
                    if let Ok(mut body) = respond.send_response(response, false) {
                        let _ = body.send_data(Bytes::from_static(b"<html>Method Not Allowed</html>"), true);
                    }
                }
            });
        }
    });
    url
}

/// `Debug` for [`CheckoutOptions`] names each metadata entry, never its value.
#[test]
fn metadata_debug_is_redacted() {
    let options =
        CheckoutOptions::default().with_metadata(vec![("authorization".to_owned(), "Bearer secret".to_owned())]);
    assert_eq!(
        format!("{options:?}"),
        r#"CheckoutOptions { metadata: ["authorization"], .. }"#
    );
}

/// Starts a mock server and returns it with a remote pool config pointing at it.
async fn mock_pool_config() -> (MockServer, PoolConfig) {
    let mock = MockServer::start().await;
    let mut config = PoolConfig::remote(mock.url());
    config.max_processes = 1;
    (mock, config)
}

/// Reads one `ParentRequest` from the mock child's stream.
fn read_request(socket: &mut MockStream) -> pb::parent_request::Kind {
    let Some(data) = socket.read() else {
        panic!("expected a request message");
    };
    decode_frame::<pb::ParentRequest>(data.as_ref())
        .expect("decode request")
        .kind
        .expect("request kind")
}

/// Sends one event from the mock child.
fn send_event(socket: &mut MockStream, event: &pb::ChildEvent) {
    let body = encode_to_capped_vec(event).expect("encode event");
    socket.send(body.into()).expect("send event");
}

fn event_kind(kind: pb::child_event::Kind) -> pb::ChildEvent {
    pb::ChildEvent {
        kind: Some(kind),
        ..Default::default()
    }
}

/// The headline scenario for parent-side mounts: the worker lives on the far
/// side of a gRPC stream, yet a mounted read is serviced from the *parent's*
/// filesystem — the mock child emits the `OsCall` and `resume_from_mounts`
/// answers it with the file's contents, no host path ever crossing the wire.
#[tokio::test]
async fn mounted_reads_are_serviced_from_the_parent_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "parent-side bytes").unwrap();

    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::OsCall(pb::OsCall {
                call_id: 7,
                values: None,
                allow_eager_await: false,
                position: Some((&position()).into()),
                call: Some(pb::os_call::Call::ReadText("/mnt/data.txt".to_owned())),
            })),
        );
        // the parent answers with the mounted file's contents
        let pb::parent_request::Kind::ResumeCall(resume) = read_request(&mut socket) else {
            panic!("expected ResumeCall");
        };
        assert_eq!(resume.call_id, 7);
        let ExtFunctionResult::Return(value) = resume_call_from_proto(resume).expect("valid result") else {
            panic!("expected a ReturnValue result");
        };
        assert_eq!(value, MontyObject::string("parent-side bytes".to_owned()));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Complete(pb::Complete::from(
                MontyObject::string("done".to_owned()),
            ))),
        );
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let event = checkout
        .feed(
            "unused",
            vec![],
            vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()],
            false,
            &mut no_print,
        )
        .await
        .expect("feed");
    assert!(matches!(event, TurnEvent::OsCall { .. }), "got {event:?}");
    let event = checkout
        .resume_from_mounts(&mut no_print)
        .await
        .expect("mount servicing")
        .expect("the mount covers /mnt/data.txt");
    assert!(
        matches!(&event, TurnEvent::Complete(v) if *v == MontyObject::string("done".to_owned())),
        "got {event:?}"
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A child claiming `allow_eager_await` on a call no future may answer is
/// not believed: the event does not expose it, and an eager `ResumeFutures`
/// is refused, so only an ordinary answer resumes the call.
#[tokio::test]
async fn eager_bit_on_a_non_future_os_call_is_dropped() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::OsCall(pb::OsCall {
                call_id: 7,
                values: None,
                allow_eager_await: true,
                position: Some((&position()).into()),
                call: Some(pb::os_call::Call::ReadText("/mnt/data.txt".to_owned())),
            })),
        );
        let pb::parent_request::Kind::ResumeCall(resume) = read_request(&mut socket) else {
            panic!("expected ResumeCall");
        };
        assert_eq!(resume.call_id, 7);
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Complete(pb::Complete::from(
                MontyObject::string("done"),
            ))),
        );
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let event = checkout
        .feed("unused", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(
            event,
            TurnEvent::OsCall {
                allow_eager_await: false,
                ..
            }
        ),
        "got {event:?}"
    );
    let refused = checkout
        .resume_futures(vec![(7, ResumeValue::Return(MontyObject::none()))], &mut no_print)
        .await;
    assert!(
        matches!(&refused, Err(PoolError::Protocol(message)) if message == "no suspended futures to resume"),
        "got {refused:?}"
    );
    let event = checkout
        .resume(ResumeValue::Return(MontyObject::string("body")), &mut no_print)
        .await
        .expect("resume");
    assert!(
        matches!(&event, TurnEvent::Complete(v) if *v == MontyObject::string("done")),
        "got {event:?}"
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A malformed `OsCall` payload from a (possibly compromised) child is a
/// protocol violation: the child validates and serializes these calls itself,
/// so a payload it could never legitimately produce (here an invalid open
/// mode) is never serviced. No parent-side I/O happens and the worker is
/// discarded.
#[tokio::test]
async fn malformed_os_call_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "never read").unwrap();

    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        // "q" is not an open() mode — must not convert, let alone dispatch
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::OsCall(pb::OsCall {
                call_id: 3,
                values: None,
                allow_eager_await: false,
                position: Some((&position()).into()),
                call: Some(pb::os_call::Call::Open(pb::os_call::Open {
                    path: "/mnt/data.txt".to_owned(),
                    mode: "q".to_owned(),
                })),
            })),
        );
        // the parent discards the worker instead of answering; wait for EOF
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed(
            "unused",
            vec![],
            vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()],
            false,
            &mut no_print,
        )
        .await
        .expect_err("malformed fs call must fail the feed");
    let PoolError::Protocol(msg) = err else {
        panic!("expected Protocol error, got {err:?}");
    };
    assert_eq!(msg, "invalid OS call payload: invalid file mode \"q\"");
    join_server(server).await;
}

/// The parent-side `max_feed_duration` backstop (remaining budget + grace)
/// kills a worker that never answers a feed — the case where the child's own
/// time enforcement has failed. No `request_timeout` is configured, so the
/// backstop is the only armed deadline.
#[tokio::test]
async fn duration_backstop_kills_an_unresponsive_worker() {
    let (mock, mut config) = mock_pool_config().await;
    config.feed_duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        // never reply; wait for the deadline to kill the connection
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let err = checkout
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    // the armed deadline was the remaining budget (≤100ms) plus the grace
    assert!(timeout <= Duration::from_millis(400), "deadline was {timeout:?}");
    join_server(server).await;
}

/// The same backstop arms on the raw path, which a relay drives instead of
/// `feed`. `turn_raw` used to arm `request_timeout` alone, so a session whose
/// only bound was `max_feed_duration` had no parent-side deadline at all.
#[tokio::test]
async fn duration_backstop_arms_on_the_raw_path() {
    let (mock, mut config) = mock_pool_config().await;
    config.feed_duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        // never reply; only the backstop can end this turn
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "while True:\n    pass".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let err = checkout.turn_raw(&request, &mut on_event).await.unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    assert!(timeout <= Duration::from_millis(400), "deadline was {timeout:?}");
    join_server(server).await;
}

/// A raw `Load` re-adopts the dump's duration budget, exactly as `restore` —
/// it used to keep the `Configure`-time budget, so a restored session's
/// backstop measured turns against the wrong limit.
#[tokio::test]
async fn a_raw_load_adopts_the_dumps_duration_budget() {
    let (mock, mut config) = mock_pool_config().await;
    config.feed_duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Load(_)));
        // an idle-dump `Load` reply, stamped with the dump's own limits
        send_event(
            &mut socket,
            &pb::ChildEvent {
                total_execution_micros: 0,
                max_suspensions: None,
                restored_script_name: None,
                session_id: None,
                execution_clock: None,
                feed_suspended: None,
                feed_execution_micros: 0,
                max_feed_duration_micros: Some(100_000),
                max_turn_duration_micros: None,
                max_total_sleep_micros: None,
                kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
            },
        );
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        // never reply; only the backstop can end this turn
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_secs(5))),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let load = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Load(pb::Load {
            state: vec![1, 2, 3].into(),
        })),
        ..pb::ParentRequest::default()
    };
    checkout.turn_raw(&load, &mut on_event).await.expect("load");
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "while True:\n    pass".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let err = checkout.turn_raw(&feed, &mut on_event).await.unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    // the dump's 100ms budget plus the grace — not the Configure-time 5s
    assert_eq!(timeout, Duration::from_millis(400));
    join_server(server).await;
}

/// Lifecycle requests are refused on the raw path without reaching the
/// worker: forwarding a client's `Reset`/`Configure` would let it disarm the
/// operator-chosen resource limits. The session stays live, which the mock
/// server proves by seeing the `Feed` as its very next frame.
#[tokio::test]
async fn lifecycle_requests_are_refused_on_the_raw_path() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        // the refused requests never arrive: the next frame is the Feed
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(2)))),
        );
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let lifecycle_kinds = [
        None,
        Some(pb::parent_request::Kind::Configure(pb::Configure::default())),
        Some(pb::parent_request::Kind::Reset(pb::Reset {})),
        Some(pb::parent_request::Kind::Shutdown(pb::Shutdown {})),
    ];
    for kind in lifecycle_kinds {
        let request = pb::ParentRequest {
            kind,
            ..pb::ParentRequest::default()
        };
        let err = checkout.turn_raw(&request, &mut on_event).await.unwrap_err();
        assert!(matches!(err, PoolError::Protocol(_)), "got {err:?}");
    }
    // the session survived every refusal
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&feed, &mut on_event).await.expect("feed");
    assert!(
        matches!(event.kind, Some(pb::child_event::Kind::Complete(_))),
        "got {:?}",
        event.kind
    );
    join_server(server).await;
}

/// An oversize raw `Load` keeps the session's duration budget: the frame is
/// rejected pre-send with the session live, so the budget cleared in
/// anticipation of the dump's limits must be put back.
#[tokio::test]
async fn an_oversize_raw_load_keeps_the_duration_budget() {
    let (mock, mut config) = mock_pool_config().await;
    config.feed_duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        // the oversize Load never arrives: the next frame is the Feed, which
        // is never answered so only the backstop can end it
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let load = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Load(pb::Load {
            state: vec![0; MAX_FRAME_LEN as usize + 1].into(),
        })),
        ..pb::ParentRequest::default()
    };
    let err = checkout.turn_raw(&load, &mut on_event).await.unwrap_err();
    assert!(matches!(err, PoolError::Runtime(_)), "got {err:?}");
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "while True:\n    pass".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let err = checkout.turn_raw(&feed, &mut on_event).await.unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    // the Configure-time budget plus the grace still arms — it was not lost
    // to the rejected Load
    assert_eq!(timeout, Duration::from_millis(400));
    join_server(server).await;
}

/// A `ShutdownDump` ends a raw turn *and* the connection behind it: the frame
/// (and its dump) is handed back for the driver to forward, but the gone
/// relay's worker is discarded rather than kept until the next failure.
#[tokio::test]
async fn a_shutdown_dump_on_the_raw_path_discards_the_worker() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Shutdown(pb::ShutdownDump {
                dump: Some(b"relay-signed state".to_vec().into()),
            })),
        );
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let event = checkout
        .turn_raw(&request, &mut on_event)
        .await
        .expect("the shutdown frame is the turn-ender");
    let Some(pb::child_event::Kind::Shutdown(shutdown)) = event.kind else {
        panic!("expected the ShutdownDump frame back, got {:?}", event.kind);
    };
    assert_eq!(shutdown.dump.as_deref(), Some(b"relay-signed state".as_slice()));
    // the connection was discarded with the frame
    let err = checkout.turn_raw(&request, &mut on_event).await.unwrap_err();
    assert!(matches!(err, PoolError::Finished), "got {err:?}");
    join_server(server).await;
}

/// A single turn is still bounded by `request_timeout` even when the worker is
/// making mount-coverable calls: the OS call surfaces rather than being
/// serviced inside the turn, so a worker that simply runs too long before
/// announcing it is killed by the deadline exactly as without mounts.
///
/// Each resume starts a new deadline; `max_feed_duration` bounds worker time
/// across a loop of calls within one feed. Neither limit covers the host I/O
/// itself; see docs/filesystem.md#io-timeouts-and-cancellation.
#[tokio::test]
async fn a_mounted_feed_turn_is_still_bounded_by_the_request_timeout() {
    let dir = tempfile::tempdir().unwrap();

    let (mock, mut config) = mock_pool_config().await;
    config.request_timeout = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        // never announce anything: the turn must be killed by the deadline
        thread::sleep(Duration::from_secs(2));
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed(
            "unused",
            vec![],
            vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()],
            false,
            &mut no_print,
        )
        .await
        .expect_err("the turn must exhaust the request timeout");
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    assert_eq!(timeout, Duration::from_millis(300));
    drop(checkout);
    join_server(server).await;
}

/// A restored session re-adopts its `max_feed_duration` budget from the timing
/// fields the worker stamps on the `Load` reply, re-arming the parent-side
/// backstop without the parent ever seeing the original `ReplConfig`.
#[tokio::test]
async fn restored_session_rearms_the_duration_backstop() {
    let (mock, mut config) = mock_pool_config().await;
    config.feed_duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Load(_)));
        // an idle restore: Ok stamped with the dump's budget/consumed time
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
                restored_script_name: Some("restored.py".to_owned()),
                session_id: None,
                execution_clock: None,
                feed_suspended: None,
                total_execution_micros: 0,
                max_suspensions: None,
                feed_execution_micros: 0,
                max_feed_duration_micros: Some(100_000),
                max_turn_duration_micros: None,
                max_total_sleep_micros: None,
            },
        );
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        // never reply; the re-adopted backstop must fire
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let (event, script_name) = checkout
        .restore(vec![1, 2, 3], vec![], &mut no_print)
        .await
        .expect("restore");
    assert!(event.is_none());
    assert_eq!(script_name.as_deref(), Some("restored.py"));
    let err = checkout
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    assert!(timeout <= Duration::from_millis(400), "deadline was {timeout:?}");
    join_server(server).await;
}

/// Serves suspensions until the parent responds with the expected `AbortFeed`.
fn serve_endless_suspensions(socket: &mut MockStream, expected_calls: u32) {
    let function_call = |call_id: u32| {
        event_kind(pb::child_event::Kind::FunctionCall(WireFunctionCall::new(
            "fetch".to_owned(),
            CallArgs::new(),
            call_id,
            None,
            false,
            position(),
        )))
    };
    assert!(matches!(read_request(socket), pb::parent_request::Kind::Feed(_)));
    send_event(socket, &function_call(1));
    for call_id in 2..=expected_calls {
        assert!(matches!(read_request(socket), pb::parent_request::Kind::ResumeCall(_)));
        send_event(socket, &function_call(call_id));
    }
    let pb::parent_request::Kind::AbortFeed(abort) = read_request(socket) else {
        panic!("expected AbortFeed");
    };
    let exception = abort.exception.expect("abort carries the exception");
    assert_eq!(exception.exc_type, "RuntimeError");
    assert_eq!(
        exception.message.as_deref(),
        Some(&*format!("suspension limit {} exceeded", expected_calls - 1))
    );
    send_event(
        socket,
        &event_kind(pb::child_event::Kind::Error(pb::Error {
            exception: Some(exception),
        })),
    );
}

/// The parent replaces an over-budget suspension with `AbortFeed`.
#[tokio::test]
async fn suspension_limit_is_enforced_by_the_parent() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        serve_endless_suspensions(&mut socket, 3);
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_suspensions(2)),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut event = checkout
        .feed("fetch()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));
    event = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .expect("second suspension");
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("suspension limit 2 exceeded"));
    drop(checkout);
    join_server(server).await;
}

/// A child that answers `AbortFeed` with another suspension is discarded
/// rather than aborted again: servicing or re-aborting it would let a
/// compromised worker run past the budget for as long as the turn deadline.
#[tokio::test]
async fn a_suspension_answering_an_abort_is_a_protocol_violation() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "fetch()");
        send_kind(&mut socket, function_call(1));
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::ResumeCall(_)
        ));
        send_kind(&mut socket, function_call(2));
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::AbortFeed(_)
        ));
        send_kind(&mut socket, function_call(3));
        // no second AbortFeed: the parent hangs up instead
        assert!(try_read_request(&mut socket).is_none());
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_suspensions(1)),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let event = checkout
        .feed("fetch()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Protocol(msg) = err else {
        panic!("expected Protocol, got {err:?}");
    };
    assert_eq!(msg, "worker answered AbortFeed with something other than an Error");
    assert!(matches!(
        checkout.feed("1", vec![], vec![], false, &mut no_print).await,
        Err(PoolError::Finished)
    ));
    join_server(server).await;
}

/// An over-budget `OsCall` is validated before the abort shortcut, so a
/// malformed payload discards the worker as it would on the typed path
/// instead of being aborted and reported as a runtime error.
#[tokio::test]
async fn a_malformed_over_budget_os_call_is_a_protocol_violation() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "open('x')");
        // the event shrinks the limit to zero itself, so this suspension is
        // over budget on arrival
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(pb::child_event::Kind::OsCall(pb::OsCall {
                    call_id: 1,
                    values: None,
                    call: None,
                    allow_eager_await: false,
                    position: Some((&position()).into()),
                })),
                max_suspensions: Some(0),
                ..Default::default()
            },
        );
        // no AbortFeed: the parent hangs up instead
        assert!(try_read_request(&mut socket).is_none());
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed("open('x')", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Protocol(msg) = err else {
        panic!("expected Protocol, got {err:?}");
    };
    assert_eq!(msg, "invalid OS call payload: missing required field OsCall.call");
    join_server(server).await;
}

/// Enforces suspension limits when a relay uses the raw path.
#[tokio::test]
async fn suspension_limit_is_enforced_on_the_raw_path() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        serve_endless_suspensions(&mut socket, 2);
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_suspensions(1)),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "fetch()".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&feed, &mut on_event).await.expect("feed");
    assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: 1,
            ..pb::ResumeCall::from(MontyObject::none())
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&resume, &mut on_event).await.expect("aborted turn");
    let Some(pb::child_event::Kind::Error(error)) = event.kind else {
        panic!("expected the abort's Error, got {event:?}");
    };
    assert_eq!(
        error.exception.and_then(|e| e.message).as_deref(),
        Some("suspension limit 1 exceeded")
    );
    drop(checkout);
    join_server(server).await;
}

/// A raw `Load` the child refuses (a session already exists) must not reset
/// the suspension count: the reply's stamped limit describes the session that
/// is still live, so its budget stays as it was.
#[tokio::test]
async fn rejected_raw_load_keeps_the_suspension_count() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        let function_call = |call_id: u32| {
            event_kind(pb::child_event::Kind::FunctionCall(WireFunctionCall::new(
                "fetch".to_owned(),
                CallArgs::new(),
                call_id,
                None,
                false,
                position(),
            )))
        };
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(&mut socket, &function_call(1));
        // the child's refusal: an ordinary Error, stamped like every reply
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Load(_)));
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(pb::child_event::Kind::Error(pb::Error {
                    exception: Some(pb::RaisedException {
                        exc_type: "RuntimeError".to_owned(),
                        message: Some("protocol violation: Load requires a session that has not started".to_owned()),
                        traceback: vec![].into(),
                        data: None,
                    }),
                })),
                max_suspensions: Some(1),
                ..Default::default()
            },
        );
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::ResumeCall(_)
        ));
        send_event(&mut socket, &function_call(2));
        let pb::parent_request::Kind::AbortFeed(abort) = read_request(&mut socket) else {
            panic!("expected AbortFeed");
        };
        let exception = abort.exception.expect("abort carries the exception");
        assert_eq!(exception.message.as_deref(), Some("suspension limit 1 exceeded"));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Error(pb::Error {
                exception: Some(exception),
            })),
        );
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_suspensions(1)),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "fetch()".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&feed, &mut on_event).await.expect("feed");
    assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
    let load = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Load(pb::Load {
            state: vec![1, 2, 3].into(),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&load, &mut on_event).await.expect("refused load");
    assert!(matches!(event.kind, Some(pb::child_event::Kind::Error(_))));
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: 1,
            ..pb::ResumeCall::from(MontyObject::none())
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&resume, &mut on_event).await.expect("aborted turn");
    let Some(pb::child_event::Kind::Error(error)) = event.kind else {
        panic!("expected the abort's Error, got {event:?}");
    };
    assert_eq!(
        error.exception.and_then(|e| e.message).as_deref(),
        Some("suspension limit 1 exceeded")
    );
    drop(checkout);
    join_server(server).await;
}

/// The checkout's configured `max_suspensions` caps whatever a `Load` reply
/// reports: a larger reported limit is clamped, and an omitted one leaves
/// the configured limit in force, so a compromised worker cannot lift the
/// parent's ceiling by lying about the dump.
#[tokio::test]
async fn configured_suspension_limit_caps_a_restored_one() {
    for reported in [Some(5), None] {
        let (mock, config) = mock_pool_config().await;
        let server = thread::spawn(move || {
            let mut socket = mock.accept();
            assert!(matches!(
                read_request(&mut socket),
                pb::parent_request::Kind::Configure(_)
            ));
            send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
            assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Load(_)));
            send_event(
                &mut socket,
                &pb::ChildEvent {
                    kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
                    max_suspensions: reported,
                    ..Default::default()
                },
            );
            serve_endless_suspensions(&mut socket, 2);
            let _ = socket.read();
        });

        let pool = Pool::new(config).await.expect("pool");
        let mut checkout = pool
            .checkout(&ReplConfig {
                limits: Some(ResourceLimits::default().max_suspensions(1)),
                ..ReplConfig::default()
            })
            .await
            .expect("checkout");
        let (event, _) = checkout
            .restore(vec![1, 2, 3], vec![], &mut no_print)
            .await
            .expect("restore");
        assert!(event.is_none());
        let event = checkout
            .feed("fetch()", vec![], vec![], false, &mut no_print)
            .await
            .expect("feed");
        assert!(matches!(event, TurnEvent::FunctionCall { .. }));
        let err = checkout
            .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
            .await
            .unwrap_err();
        let PoolError::Runtime(exc) = err else {
            panic!("expected Runtime, got {err:?}");
        };
        assert_eq!(
            exc.message(),
            Some("suspension limit 1 exceeded"),
            "reported {reported:?}"
        );
        drop(checkout);
        join_server(server).await;
    }
}

/// A checkout with no limits configured still enforces the 1000 default.
#[tokio::test]
async fn suspension_limit_defaults_to_one_thousand() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        serve_endless_suspensions(&mut socket, 1001);
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let mut event = checkout
        .feed("fetch()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    for _ in 1..1000 {
        assert!(matches!(event, TurnEvent::FunctionCall { .. }));
        event = checkout
            .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
            .await
            .expect("resume");
    }
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("suspension limit 1000 exceeded"));
    drop(checkout);
    join_server(server).await;
}

/// A suspended dump whose re-announced suspension is already over its limit
/// is aborted at once, and the dump's limit still sticks for later turns:
/// adoption is decided by the first reply, not by the abort's `Error`.
#[tokio::test]
async fn aborted_restored_suspension_keeps_the_dump_limit() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        let function_call =
            |call_id: u32| WireFunctionCall::new("fetch".to_owned(), CallArgs::new(), call_id, None, false, position());
        let abort_reply = |socket: &mut MockStream| {
            let pb::parent_request::Kind::AbortFeed(abort) = read_request(socket) else {
                panic!("expected AbortFeed");
            };
            let exception = abort.exception.expect("abort carries the exception");
            assert_eq!(exception.message.as_deref(), Some("suspension limit 0 exceeded"));
            send_event(
                socket,
                &event_kind(pb::child_event::Kind::Error(pb::Error {
                    exception: Some(exception),
                })),
            );
        };
        // the dump's limit is 0, so its re-announced suspension is over budget
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Load(_)));
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(pb::child_event::Kind::FunctionCall(function_call(1))),
                max_suspensions: Some(0),
                ..Default::default()
            },
        );
        abort_reply(&mut socket);
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::FunctionCall(function_call(2))),
        );
        abort_reply(&mut socket);
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_suspensions(1)),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let load = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Load(pb::Load {
            state: vec![1, 2, 3].into(),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&load, &mut on_event).await.expect("aborted restore");
    assert!(
        matches!(event.kind, Some(pb::child_event::Kind::Error(_))),
        "got {event:?}"
    );
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "fetch()".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&feed, &mut on_event).await.expect("aborted feed");
    let Some(pb::child_event::Kind::Error(error)) = event.kind else {
        panic!("expected the abort's Error, got {event:?}");
    };
    assert_eq!(
        error.exception.and_then(|e| e.message).as_deref(),
        Some("suspension limit 0 exceeded")
    );
    drop(checkout);
    join_server(server).await;
}

/// Restores `max_suspensions` from the worker's `Load` reply.
#[tokio::test]
async fn restored_session_readopts_the_suspension_limit() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Load(_)));
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
                max_suspensions: Some(1),
                ..Default::default()
            },
        );
        serve_endless_suspensions(&mut socket, 2);
        let _ = socket.read();
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let (event, _) = checkout
        .restore(vec![1, 2, 3], vec![], &mut no_print)
        .await
        .expect("restore");
    assert!(event.is_none());
    let event = checkout
        .feed("fetch()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("suspension limit 1 exceeded"));
    drop(checkout);
    join_server(server).await;
}

// === server shutdown and disconnects ===
//
// These tests script the server side precisely, so they assert exactly what a
// client sees when a server hands back a `ShutdownDump` or simply drops the
// connection — and that a shutdown dump can be restored by hand.

/// Reads the next request message; `None` once the client ended the stream.
fn try_read_request(socket: &mut MockStream) -> Option<pb::ParentRequest> {
    socket
        .read()
        .map(|data| decode_frame::<pb::ParentRequest>(data.as_ref()).expect("decode request"))
}

/// Sends one `ChildEvent` with the given kind (timing fields zeroed).
fn send_kind(socket: &mut MockStream, kind: pb::child_event::Kind) {
    send_event(socket, &event_kind(kind));
}

/// Shorthand for the `Ok` acknowledgement event.
fn ok_event() -> pb::child_event::Kind {
    pb::child_event::Kind::Ok(pb::Ok {})
}

/// Builds a `ShutdownDump` turn-ender.
fn shutdown(dump: Option<&[u8]>) -> pb::child_event::Kind {
    pb::child_event::Kind::Shutdown(pb::ShutdownDump {
        dump: dump.map(|bytes| bytes.to_vec().into()),
    })
}

/// Builds a `FunctionCall` suspension with the given call id.
fn function_call(call_id: u32) -> pb::child_event::Kind {
    pb::child_event::Kind::FunctionCall(WireFunctionCall::new(
        "ext".to_owned(),
        CallArgs::new(),
        call_id,
        None,
        false,
        position(),
    ))
}

/// Asserts the next request is `Configure` and acknowledges it.
fn expect_configure(socket: &mut MockStream) {
    let request = try_read_request(socket).expect("configure");
    assert!(
        matches!(request.kind, Some(pb::parent_request::Kind::Configure(_))),
        "expected Configure, got {request:?}"
    );
    send_kind(socket, ok_event());
}

/// Asserts the next request is `Feed` of `code`.
fn expect_feed(socket: &mut MockStream, code: &str) {
    let request = try_read_request(socket).expect("feed");
    match request.kind {
        Some(pb::parent_request::Kind::Feed(feed)) => assert_eq!(feed.code, code),
        other => panic!("expected Feed, got {other:?}"),
    }
}

/// Asserts the next request is `Load` and checks its state bytes.
fn expect_load(socket: &mut MockStream, state: &[u8]) {
    let request = try_read_request(socket).expect("load");
    match request.kind {
        Some(pb::parent_request::Kind::Load(load)) => assert_eq!(load.state, state),
        other => panic!("expected Load, got {other:?}"),
    }
}

/// A pool + checkout against the mock at `url` with a 10s turn timeout.
async fn remote_checkout(url: &str) -> (Pool, Checkout) {
    let pool = remote_pool(url).await;
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    (pool, checkout)
}

/// A single-worker pool against the mock at `url`.
async fn remote_pool(url: &str) -> Pool {
    let mut config = PoolConfig::remote(url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    Pool::new(config).await.expect("pool")
}

#[tokio::test]
async fn shutdown_hands_back_a_restorable_dump() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        // conn 1: a draining server intercepts the feed and replies with the dump
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        send_kind(&mut socket, shutdown(Some(b"fake-dump")));
        // conn 2: the client restores the dump by hand and re-runs the feed
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"fake-dump");
        send_kind(&mut socket, ok_event());
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let pool = remote_pool(&url).await;
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("a draining server must not run the feed");
    let PoolError::Shutdown { dump: Some(dump) } = err else {
        panic!("expected Shutdown with a dump, got {err:?}");
    };
    assert_eq!(dump, b"fake-dump");
    // a relay that stores nothing names no session
    assert_eq!(checkout.session_id(), None);

    // the checkout's worker is gone; a fresh one restores the dump and the
    // never-executed feed can simply be re-run
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("second checkout");
    let (event, _name) = checkout.restore(dump, vec![], &mut no_print).await.expect("restore");
    assert!(event.is_none(), "an idle dump has no suspension to re-announce");
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed on the restored session");
    assert!(
        matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
        "got {event:?}"
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn shutdown_during_a_suspension_carries_the_suspended_dump() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(7));
        let resume = try_read_request(&mut socket).expect("resume");
        assert!(
            matches!(&resume.kind, Some(pb::parent_request::Kind::ResumeCall(rc)) if rc.call_id == 7),
            "expected ResumeCall(7), got {resume:?}"
        );
        send_kind(&mut socket, shutdown(Some(b"susp-dump")));
        // the restored session re-announces the suspension the resume answered
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"susp-dump");
        send_kind(&mut socket, function_call(7));
        while try_read_request(&mut socket).is_some() {}
    });

    let pool = remote_pool(&url).await;
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let event = checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::FunctionCall { call_id: 7, .. }),
        "got {event:?}"
    );
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::int(5)), &mut no_print)
        .await
        .expect_err("a draining server must not run the resume");
    let PoolError::Shutdown { dump: Some(dump) } = err else {
        panic!("expected Shutdown with a dump, got {err:?}");
    };
    assert_eq!(dump, b"susp-dump");

    // restoring re-announces the suspension, so the host can answer it again
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("second checkout");
    let (event, _name) = checkout.restore(dump, vec![], &mut no_print).await.expect("restore");
    assert!(
        matches!(event, Some(TurnEvent::FunctionCall { call_id: 7, .. })),
        "got {event:?}"
    );
    // closes the socket, which ends the mock's trailing read loop
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn shutdown_without_a_session_carries_no_dump() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        // a server that began draining before this session ever ran anything
        let mut socket = mock.accept();
        let request = try_read_request(&mut socket).expect("configure");
        assert!(matches!(request.kind, Some(pb::parent_request::Kind::Configure(_))));
        send_kind(&mut socket, shutdown(None));
    });

    // the session opens with its first request, so that is where a draining
    // server's refusal surfaces
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("the first feed against a draining server must fail");
    assert!(matches!(err, PoolError::Shutdown { dump: None }), "got {err:?}");
    join_server(server).await;
}

/// The first request goes out behind the `Configure` in the same write: a
/// server sees both before it has answered either, so opening a session and
/// running its first feed is one round trip.
#[tokio::test]
async fn the_first_request_is_pipelined_behind_configure() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        socket.set_read_timeout(Duration::from_secs(5));
        let request = try_read_request(&mut socket).expect("configure");
        assert!(matches!(request.kind, Some(pb::parent_request::Kind::Configure(_))));
        // nothing has been answered yet, and the feed is already here
        expect_feed(&mut socket, "1 + 1");
        send_kind(&mut socket, ok_event());
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(matches!(event, TurnEvent::Complete(ref value) if *value == MontyObject::int(42)));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A refused `Configure` is the first request's error, and ends the session:
/// the feed pipelined behind it never ran, and the worker is gone.
#[tokio::test]
async fn a_refused_configure_fails_the_first_request_and_ends_the_session() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        socket.set_read_timeout(Duration::from_secs(5));
        let request = try_read_request(&mut socket).expect("configure");
        assert!(matches!(request.kind, Some(pb::parent_request::Kind::Configure(_))));
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Error(pb::Error {
                exception: Some(pb::RaisedException {
                    exc_type: "RuntimeError".to_owned(),
                    message: Some("protocol violation: invalid os_policy".to_owned()),
                    traceback: vec![].into(),
                    data: None,
                }),
            }),
        );
        // the client ends the stream without answering the feed's reply
        assert!(socket.read().is_none(), "a refused session must not continue");
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("the refusal is the feed's error");
    let PoolError::Runtime(exc) = err else {
        panic!("got {err:?}");
    };
    assert_eq!(exc.message(), Some("protocol violation: invalid os_policy"));
    let err = checkout
        .feed("2 + 2", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("the session is over");
    assert!(matches!(err, PoolError::Finished), "got {err:?}");
    join_server(server).await;
}

// ---- session IDs -----------------------------------------------------------
//
// A storing relay names the session in `ChildEvent.session_id`; the client
// treats it as opaque bytes and sends `persistence` and `profile` through.

/// Sends one `ChildEvent` with the given kind, naming the session `session_id`.
fn send_with_session_id(socket: &mut MockStream, kind: pb::child_event::Kind, session_id: &[u8]) {
    send_event(
        socket,
        &pb::ChildEvent {
            kind: Some(kind),
            session_id: Some(session_id.to_vec().into()),
            ..Default::default()
        },
    );
}

#[tokio::test]
async fn configure_carries_persistence() {
    let cases = [
        (Persistence::ServerDefault, pb::Persistence::Unspecified),
        (Persistence::Ephemeral, pb::Persistence::Ephemeral),
        (Persistence::Stored, pb::Persistence::Stored),
    ];
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        for (_, expected) in cases {
            let mut socket = mock.accept();
            let request = try_read_request(&mut socket).expect("configure");
            let Some(pb::parent_request::Kind::Configure(configure)) = request.kind else {
                panic!("expected Configure, got {request:?}");
            };
            assert_eq!(configure.persistence(), expected);
            send_kind(&mut socket, ok_event());
            // the feed that carried the Configure out
            expect_feed(&mut socket, "1");
            send_kind(
                &mut socket,
                pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
            );
            while try_read_request(&mut socket).is_some() {}
        }
    });

    let pool = remote_pool(&url).await;
    for (persistence, _) in cases {
        let repl = ReplConfig {
            persistence,
            ..ReplConfig::default()
        };
        let mut checkout = pool.checkout(&repl).await.expect("checkout");
        checkout
            .feed("1", vec![], vec![], false, &mut no_print)
            .await
            .expect("feed");
        checkout.finish().await.expect("finish");
    }
    join_server(server).await;
}

#[tokio::test]
async fn configure_carries_profile() {
    let cases = [None, Some("gpu".to_owned()), Some(String::new())];
    let expected = cases.clone();
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        for expected in expected {
            let mut socket = mock.accept();
            let request = try_read_request(&mut socket).expect("configure");
            let Some(pb::parent_request::Kind::Configure(configure)) = request.kind else {
                panic!("expected Configure, got {request:?}");
            };
            assert_eq!(configure.profile, expected);
            send_kind(&mut socket, ok_event());
            // the feed that carried the Configure out
            expect_feed(&mut socket, "1");
            send_kind(
                &mut socket,
                pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
            );
            while try_read_request(&mut socket).is_some() {}
        }
    });

    let pool = remote_pool(&url).await;
    for profile in cases {
        let repl = ReplConfig {
            profile,
            ..ReplConfig::default()
        };
        let mut checkout = pool.checkout(&repl).await.expect("checkout");
        checkout
            .feed("1", vec![], vec![], false, &mut no_print)
            .await
            .expect("feed");
        checkout.finish().await.expect("finish");
    }
    join_server(server).await;
}

#[tokio::test]
async fn session_id_is_read_from_the_configure_reply() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        try_read_request(&mut socket).expect("configure");
        send_with_session_id(&mut socket, ok_event(), b"sess-1");
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    // the first request carries the Configure out, so the ID arrives with it;
    // later replies carry no ID, and the session keeps the one it has
    assert_eq!(checkout.session_id(), None);
    checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert_eq!(checkout.session_id(), Some(&b"sess-1"[..]));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn restore_adopts_the_new_session_id() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        try_read_request(&mut socket).expect("configure");
        send_with_session_id(&mut socket, ok_event(), b"sess-new");
        expect_load(&mut socket, b"sess-1");
        // loading an ID starts a new session; a suspended one re-announces its
        // call, which carries the new ID
        send_with_session_id(&mut socket, function_call(7), b"sess-2");
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let (event, _name) = checkout
        .restore(b"sess-1".to_vec(), vec![], &mut no_print)
        .await
        .expect("restore");
    assert!(
        matches!(event, Some(TurnEvent::FunctionCall { call_id: 7, .. })),
        "got {event:?}"
    );
    assert_eq!(checkout.session_id(), Some(&b"sess-2"[..]));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn restore_clears_a_stale_session_id() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        try_read_request(&mut socket).expect("configure");
        send_with_session_id(&mut socket, ok_event(), b"sess-1");
        // restoring dump bytes: the reply names no session
        expect_load(&mut socket, b"dump-bytes");
        send_kind(&mut socket, ok_event());
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    // the Configure reply names `sess-1` on the way in, and the Load reply
    // behind it, naming nothing, replaces it
    checkout
        .restore(b"dump-bytes".to_vec(), vec![], &mut no_print)
        .await
        .expect("restore");
    assert_eq!(checkout.session_id(), None);
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A load the worker refuses leaves the session it already had, which a
/// storing relay still names by the ID from `Configure`: the checkout must not
/// forget it, or it would decline to resume a session the relay can hand back.
#[tokio::test]
async fn refused_restore_keeps_the_session_id() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_load(&mut socket, b"bad-dump");
        // the refusal: an ordinary Error, which names no session
        send_kind(
            &mut socket,
            pb::child_event::Kind::Error(pb::Error {
                exception: Some(pb::RaisedException {
                    exc_type: "RuntimeError".to_owned(),
                    message: Some("unknown session".to_owned()),
                    traceback: vec![].into(),
                    data: None,
                }),
            }),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let refused = checkout.restore(b"bad-dump".to_vec(), vec![], &mut no_print).await;
    assert!(matches!(refused, Err(PoolError::Runtime(_))), "{refused:?}");
    assert_eq!(checkout.session_id(), Some(&b"sess-1"[..]));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

// ---- auto-resume -------------------------------------------------------------
//
// A storing relay that drains names the session in `ShutdownDump`; the checkout
// redials, loads its ID into a new session and re-sends the request the relay
// did not run.

/// Acknowledges the next request, which must be `Configure`, naming the session `id`.
fn configure_with_session_id(socket: &mut MockStream, id: &[u8]) {
    let request = try_read_request(socket).expect("configure");
    assert!(
        matches!(request.kind, Some(pb::parent_request::Kind::Configure(_))),
        "expected Configure, got {request:?}"
    );
    send_with_session_id(socket, ok_event(), id);
}

/// Asserts the next request answers call `call_id`.
fn expect_resume_call(socket: &mut MockStream, call_id: u32) {
    let request = try_read_request(socket).expect("resume");
    assert!(
        matches!(&request.kind, Some(pb::parent_request::Kind::ResumeCall(rc)) if rc.call_id == call_id),
        "expected ResumeCall({call_id}), got {request:?}"
    );
}

/// Sends a `Complete(42)` turn-ender.
fn send_complete(socket: &mut MockStream) {
    send_kind(
        socket,
        pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
    );
}

#[tokio::test]
async fn shutdown_resumes_transparently() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "1 + 1");
        // the drain names what to load, which need not be the session's own ID
        send_kind(&mut socket, shutdown(Some(b"parked-1")));
        // the checkout redials, reloads the session and re-sends the feed
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"parked-1");
        send_with_session_id(&mut socket, ok_event(), b"sess-2");
        expect_feed(&mut socket, "1 + 1");
        send_complete(&mut socket);
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("the drain is invisible to the caller");
    assert!(
        matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
        "got {event:?}"
    );
    // the reload started a new session under the ID its reply carried
    assert_eq!(checkout.session_id(), Some(&b"sess-2"[..]));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// The redial re-sends the session's `Configure`, so the resumed session keeps
/// the profile the first one asked for.
#[tokio::test]
async fn resume_resends_the_profile() {
    let (profile_tx, profile_rx) = mpsc::channel();
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        let request = try_read_request(&mut socket).expect("first configure");
        let Some(pb::parent_request::Kind::Configure(configure)) = request.kind else {
            panic!("expected Configure, got {request:?}");
        };
        profile_tx.send(configure.profile).expect("first profile");
        send_with_session_id(&mut socket, ok_event(), b"sess-1");
        expect_feed(&mut socket, "1 + 1");
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        let request = try_read_request(&mut socket).expect("second configure");
        let Some(pb::parent_request::Kind::Configure(configure)) = request.kind else {
            panic!("expected Configure, got {request:?}");
        };
        profile_tx.send(configure.profile).expect("second profile");
        send_kind(&mut socket, ok_event());
        expect_load(&mut socket, b"sess-1");
        send_with_session_id(&mut socket, ok_event(), b"sess-2");
        expect_feed(&mut socket, "1 + 1");
        send_complete(&mut socket);
        while try_read_request(&mut socket).is_some() {}
    });

    let repl = ReplConfig {
        profile: Some("gpu".to_owned()),
        ..ReplConfig::default()
    };
    let pool = remote_pool(&url).await;
    let mut checkout = pool.checkout(&repl).await.expect("checkout");
    checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("the drain is invisible to the caller");
    assert_eq!(checkout.session_id(), Some(&b"sess-2"[..]));
    checkout.finish().await.expect("finish");
    join_server(server).await;
    let profiles: Vec<_> = profile_rx.iter().collect();
    assert_eq!(profiles, [Some("gpu".to_owned()), Some("gpu".to_owned())]);
}

#[tokio::test]
async fn shutdown_while_suspended_resumes_after_the_same_call_is_reannounced() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(7));
        expect_resume_call(&mut socket, 7);
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        send_with_session_id(&mut socket, function_call(7), b"sess-2");
        expect_resume_call(&mut socket, 7);
        send_complete(&mut socket);
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let event = checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::FunctionCall { call_id: 7, .. }),
        "got {event:?}"
    );
    let event = checkout
        .resume(ResumeValue::Return(MontyObject::int(5)), &mut no_print)
        .await
        .expect("the drain is invisible to the caller");
    assert!(
        matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
        "got {event:?}"
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn resume_rejects_a_different_reannounced_call() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(7));
        expect_resume_call(&mut socket, 7);
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        // not the call the drained session was waiting on
        send_with_session_id(&mut socket, function_call(8), b"sess-2");
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::int(5)), &mut no_print)
        .await
        .expect_err("a mismatched session must not be resumed");
    let PoolError::Shutdown { dump } = err else {
        panic!("expected the original Shutdown, got {err:?}");
    };
    assert_eq!(dump.as_deref(), Some(&b"sess-1"[..]));
    assert!(matches!(
        checkout.feed("1", vec![], vec![], false, &mut no_print).await,
        Err(PoolError::Finished)
    ));
    join_server(server).await;
}

#[tokio::test]
async fn resume_rejects_a_reannounced_call_with_another_name() {
    // the same call id naming a different function is another session
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(7));
        expect_resume_call(&mut socket, 7);
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        send_with_session_id(
            &mut socket,
            pb::child_event::Kind::FunctionCall(WireFunctionCall::new(
                "other".to_owned(),
                CallArgs::new(),
                7,
                None,
                false,
                position(),
            )),
            b"sess-2",
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::int(5)), &mut no_print)
        .await
        .expect_err("a mismatched session must not be resumed");
    assert!(matches!(err, PoolError::Shutdown { .. }), "got {err:?}");
    join_server(server).await;
}

#[tokio::test]
async fn an_aborted_load_reply_still_names_the_session() {
    // a re-announced suspension over budget is aborted before the caller
    // sees it, but the ID its event carried names the session that lives on
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_load(&mut socket, b"sess-1");
        send_with_session_id(&mut socket, function_call(7), b"sess-2");
        let request = try_read_request(&mut socket).expect("abort");
        let Some(pb::parent_request::Kind::AbortFeed(abort)) = request.kind else {
            panic!("expected AbortFeed, got {request:?}");
        };
        send_kind(
            &mut socket,
            pb::child_event::Kind::Error(pb::Error {
                exception: abort.exception,
            }),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let pool = remote_pool(&url).await;
    let repl = ReplConfig {
        limits: Some(ResourceLimits::default().max_suspensions(0)),
        ..ReplConfig::default()
    };
    let mut checkout = pool.checkout(&repl).await.expect("checkout");
    let err = checkout
        .restore(b"sess-1".to_vec(), vec![], &mut no_print)
        .await
        .expect_err("the re-announced call is over the limit");
    assert!(matches!(err, PoolError::Runtime(_)), "got {err:?}");
    assert_eq!(checkout.session_id(), Some(&b"sess-2"[..]));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn resume_rejects_a_reannounced_lookup_on_another_object() {
    // the same name on a different receiver is another lookup
    let lookup = |object: u8| {
        pb::child_event::Kind::NameLookup(pb::NameLookup {
            name: "x".to_owned(),
            object_id: Some(pb::Uuid {
                data: vec![object; 16].into(),
            }),
            position: Some((&position()).into()),
        })
    };
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "obj.x");
        send_kind(&mut socket, lookup(1));
        let request = try_read_request(&mut socket).expect("resume");
        assert!(
            matches!(request.kind, Some(pb::parent_request::Kind::ResumeNameLookup(_))),
            "expected ResumeNameLookup, got {request:?}"
        );
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        send_with_session_id(&mut socket, lookup(2), b"sess-2");
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let event = checkout
        .feed("obj.x", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(matches!(event, TurnEvent::NameLookup { .. }), "got {event:?}");
    let err = checkout
        .resume_name_lookup(MontyObject::int(1), &mut no_print)
        .await
        .expect_err("a mismatched session must not be resumed");
    assert!(matches!(err, PoolError::Shutdown { .. }), "got {err:?}");
    join_server(server).await;
}

#[tokio::test]
async fn resumed_session_keeps_its_duration_backstop() {
    // the reload's `Load` reply reports no limits, as a worker hiding them
    // would; the configured feed budget still bounds the resumed feed
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        send_with_session_id(&mut socket, ok_event(), b"sess-2");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(7));
        expect_resume_call(&mut socket, 7);
        // never answered: the backstop (100ms budget plus 100ms grace) must end
        // the turn, and the client closes the connection when it does
        while try_read_request(&mut socket).is_some() {}
    });

    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    config.feed_duration_limit_grace = Some(Duration::from_millis(100));
    let pool = Pool::new(config).await.expect("pool");
    let repl = ReplConfig {
        limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
        ..ReplConfig::default()
    };
    let mut checkout = pool.checkout(&repl).await.expect("checkout");
    checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::int(5)), &mut no_print)
        .await
        .expect_err("the configured feed budget survives the reload");
    // the backstop's deadline, not the 10s request timeout
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    assert_eq!(timeout, Duration::from_millis(200));
    join_server(server).await;
}

#[tokio::test]
async fn resumed_turn_takes_the_reloaded_backstop() {
    // the session was configured without limits; the `Load` reply reports the
    // dump's, which must bound the re-sent turn rather than the deadline
    // computed before the shutdown
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(7));
        expect_resume_call(&mut socket, 7);
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(function_call(7)),
                session_id: Some(b"sess-2".to_vec().into()),
                max_feed_duration_micros: Some(100_000),
                ..Default::default()
            },
        );
        // never answered: the reloaded 100ms budget plus 100ms grace ends the turn
        expect_resume_call(&mut socket, 7);
        while try_read_request(&mut socket).is_some() {}
    });

    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    config.feed_duration_limit_grace = Some(Duration::from_millis(100));
    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::int(5)), &mut no_print)
        .await
        .expect_err("the reloaded feed budget bounds the re-sent turn");
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    assert_eq!(timeout, Duration::from_millis(200));
    join_server(server).await;
}

#[tokio::test]
async fn resume_refused_returns_the_original_shutdown() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "1 + 1");
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        // e.g. the store no longer holds the session
        send_kind(
            &mut socket,
            pb::child_event::Kind::FatalError(pb::FatalError {
                message: "unknown session".to_owned(),
            }),
        );
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("a refused resume surfaces the drain");
    let PoolError::Shutdown { dump } = err else {
        panic!("expected the original Shutdown, got {err:?}");
    };
    assert_eq!(dump.as_deref(), Some(&b"sess-1"[..]));
    join_server(server).await;
}

#[tokio::test]
async fn auto_resume_disabled_returns_the_shutdown() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "1 + 1");
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
    });

    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    config.auto_resume = false;
    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("without auto-resume the drain surfaces");
    let PoolError::Shutdown { dump } = err else {
        panic!("expected Shutdown, got {err:?}");
    };
    assert_eq!(dump.as_deref(), Some(&b"sess-1"[..]));
    join_server(server).await;
}

#[tokio::test]
async fn shutdown_without_state_is_not_resumed() {
    // a drain naming nothing to load has nothing to resume from, whatever ID
    // the session holds: the relay could not park it
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "1 + 1");
        send_kind(&mut socket, shutdown(None));
        // no redial: a second accept would hang the server thread
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("nothing to load");
    let PoolError::Shutdown { dump } = err else {
        panic!("expected Shutdown, got {err:?}");
    };
    assert_eq!(dump, None);
    join_server(server).await;
}

#[tokio::test]
async fn resumed_feed_restarts_the_feed_clock() {
    // the `Load` reply reports the dump's last feed; the re-sent `Feed` starts
    // a new one, so that time must not shorten its backstop
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        // the previous feed used the whole budget
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(ok_event()),
                session_id: Some(b"sess-2".to_vec().into()),
                feed_execution_micros: 10_000_000,
                max_feed_duration_micros: Some(10_000_000),
                ..Default::default()
            },
        );
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(7));
        // well inside the restarted feed's backstop (10s budget plus 100ms
        // grace), but past the 100ms grace that is all the dump's feed time
        // would leave, however slow the machine
        expect_resume_call(&mut socket, 7);
        thread::sleep(Duration::from_millis(500));
        send_complete(&mut socket);
        while try_read_request(&mut socket).is_some() {}
    });

    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    config.feed_duration_limit_grace = Some(Duration::from_millis(100));
    let pool = Pool::new(config).await.expect("pool");
    let repl = ReplConfig {
        limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_secs(10))),
        ..ReplConfig::default()
    };
    let mut checkout = pool.checkout(&repl).await.expect("checkout");
    let event = checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::FunctionCall { call_id: 7, .. }),
        "got {event:?}"
    );
    let event = checkout
        .resume(ResumeValue::Return(MontyObject::int(5)), &mut no_print)
        .await
        .expect("the resumed feed has its whole budget");
    assert!(
        matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
        "got {event:?}"
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn resumed_session_keeps_its_suspension_count() {
    // the host counts suspensions; a drain must not restart the count
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "ext()");
        send_kind(&mut socket, function_call(1));
        expect_resume_call(&mut socket, 1);
        send_kind(&mut socket, function_call(2));
        expect_resume_call(&mut socket, 2);
        send_kind(&mut socket, shutdown(Some(b"sess-1")));
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_load(&mut socket, b"sess-1");
        send_with_session_id(&mut socket, function_call(2), b"sess-2");
        expect_resume_call(&mut socket, 2);
        // the third suspension of the session, one past the limit
        send_kind(&mut socket, function_call(3));
        let request = try_read_request(&mut socket).expect("abort");
        let Some(pb::parent_request::Kind::AbortFeed(abort)) = request.kind else {
            panic!("expected AbortFeed, got {request:?}");
        };
        // the child echoes the abort's exception as the feed's error
        send_kind(
            &mut socket,
            pb::child_event::Kind::Error(pb::Error {
                exception: abort.exception,
            }),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let pool = remote_pool(&url).await;
    let repl = ReplConfig {
        limits: Some(ResourceLimits::default().max_suspensions(2)),
        ..ReplConfig::default()
    };
    let mut checkout = pool.checkout(&repl).await.expect("checkout");
    checkout
        .feed("ext()", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    checkout
        .resume(ResumeValue::Return(MontyObject::int(0)), &mut no_print)
        .await
        .expect("second suspension");
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::int(0)), &mut no_print)
        .await
        .expect_err("the third suspension is over the limit");
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("suspension limit 2 exceeded"));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn bare_disconnect_is_never_resumed() {
    // only `ShutdownDump` says the request did not run; a closed socket does not
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        configure_with_session_id(&mut socket, b"sess-1");
        expect_feed(&mut socket, "1 + 1");
        socket.close(None);
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("a finished stream must fail the turn");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, Some("OK"));
    assert_eq!(details.status_message, None);
    assert_eq!(details.cause, None);
    assert_eq!(details.transport_error, None);
    join_server(server).await;
}

/// A server's status survives teardown without implying that the request did not run.
#[tokio::test]
async fn disconnect_preserves_peer_close_details() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        let mut status = Cause::Capacity.status("session capacity exhausted");
        status.metadata_mut().insert("retry-after", "12".parse().unwrap());
        socket.close(Some(status));
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("disconnect");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, Some("RESOURCE_EXHAUSTED"));
    assert_eq!(details.status_message.as_deref(), Some("session capacity exhausted"));
    assert_eq!(details.cause, Some(RemoteCause::Capacity));
    assert_eq!(details.transport_error, None);
    // retry hints belong to refusals: the session may already have run
    assert_eq!(details.retry_after, None);
    join_server(server).await;
}

/// A status without a `monty-cause` trailer — one from a proxy, or a server
/// policy this build does not know — keeps its code and message with no cause.
#[tokio::test]
async fn disconnect_preserves_an_unattributed_status() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        socket.close(Some(Status::internal("relay fell over")));
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("disconnect");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, Some("INTERNAL"));
    assert_eq!(details.status_message.as_deref(), Some("relay fell over"));
    assert_eq!(details.cause, None);
    assert_eq!(details.transport_error, None);
    join_server(server).await;
}

/// A status ending the stream while the checkout is idle is what the next
/// request reports: the request goes into the ended stream and the turn reads
/// the status the reader recorded, never a bare send failure.
#[tokio::test]
async fn status_ending_an_idle_stream_is_reported_by_the_next_request() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let socket = mock.accept();
        socket.close(Some(Cause::IdleTimeout.status("no request within the idle timeout")));
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    join_server(server).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("an ended stream must fail the turn");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, Some("DEADLINE_EXCEEDED"));
    assert_eq!(details.cause, Some(RemoteCause::IdleTimeout));
    assert_eq!(details.transport_error, None);
}

/// A server that reports the worker ran out of memory ends the session with
/// the same `MemoryError` a local out-of-memory kill raises.
#[tokio::test]
async fn out_of_memory_status_is_a_memory_error() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "x = 'a' * 10**9");
        socket.close(Some(Cause::OutOfMemory.status("worker exceeded its memory limit")));
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("x = 'a' * 10**9", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("out of memory");
    let PoolError::Runtime(exc) = err else {
        panic!("got {err:?}");
    };
    assert_eq!(
        exc.to_string(),
        "MemoryError: the worker exceeded its memory limit and was terminated"
    );
    assert!(checkout.worker_id().is_none(), "the worker is gone");
    join_server(server).await;
}

/// A draining server that had no state to hand back for an idle session ends
/// it with `server_shutdown`: the request did not run, and there is no dump.
#[tokio::test]
async fn server_shutdown_status_is_a_shutdown_without_state() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        socket.close(Some(Cause::ServerShutdown.status("server is draining")));
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("shutdown");
    assert!(matches!(err, PoolError::Shutdown { dump: None }), "got {err:?}");
    join_server(server).await;
}

/// A stream the server reset without any status retains the transport error
/// rather than a fabricated status.
#[tokio::test]
async fn disconnect_preserves_a_reset_stream() {
    let mock = MockServer::start_proxied().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        // the connection is cut under the open stream: no status, no stream end
        mock.cut();
        expect_end(&mut socket);
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("disconnect");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, None);
    assert_eq!(details.cause, None);
    assert!(details.transport_error.is_some(), "got {details:?}");
    join_server(server).await;
}

/// The load-bearing keepalive check: a `Ping` sent while the client sits idle
/// (no turn in flight, so no `recv` polling the stream) is still answered,
/// because the background reader task polls the stream continuously.
/// Without that task the `Pong` only goes out on the next turn — and a
/// keepalive server would have dropped the session as vanished long before.
#[tokio::test]
async fn pings_are_answered_while_idle() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        // the client is idle from the moment the stream opens: nothing goes
        // out until its first request
        send_kind(&mut socket, pb::child_event::Kind::Ping(pb::Ping {}));
        // bound the wait so a missing pong fails the test rather than hanging it
        socket.set_read_timeout(Duration::from_secs(5));
        let request = try_read_request(&mut socket).expect("a Pong while the client is idle");
        assert!(
            matches!(request.kind, Some(pb::parent_request::Kind::Pong(_))),
            "expected a Pong, got {request:?}"
        );
    });

    let pool = Pool::new(config).await.expect("pool");
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    // go idle: hold the checkout without driving a turn while the server
    // pings; joining the server is what proves the pong arrived
    join_server(server).await;
    drop(checkout);
}

/// A ping above the size limit ends the stream: no turn deadline covers an
/// idle stream, so only the reader bounds what a server sends on it.
#[tokio::test]
async fn an_oversized_ping_ends_the_stream() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        // a ping padded with 100 bytes of unknown field 111, which decoding skips
        let mut body = encode_to_capped_vec(&event_kind(pb::child_event::Kind::Ping(pb::Ping {}))).expect("encode");
        body.extend([0xfa, 0x06, 100]);
        body.extend([0; 100]);
        socket.send(Bytes::from(body)).expect("send the ping");
        drain_requests(&mut socket);
    });

    let details = disconnect_after_a_feed(config).await;
    assert_eq!(
        details.transport_error.as_deref(),
        Some("server sent a 105-byte ping, above the 64-byte limit")
    );
    join_server(server).await;
}

/// A server pinging far faster than any keepalive ends the stream.
#[tokio::test]
async fn a_ping_flood_ends_the_stream() {
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        for _ in 0..11 {
            if socket
                .send(Bytes::from(
                    encode_to_capped_vec(&event_kind(pb::child_event::Kind::Ping(pb::Ping {}))).expect("encode"),
                ))
                .is_err()
            {
                break;
            }
        }
        drain_requests(&mut socket);
    });

    let details = disconnect_after_a_feed(config).await;
    assert_eq!(
        details.transport_error.as_deref(),
        Some("server sent more than 10 pings in 1s")
    );
    join_server(server).await;
}

/// Checks out from `config`, feeds once, and returns the disconnect that ended
/// the stream.
async fn disconnect_after_a_feed(config: PoolConfig) -> Box<monty_pool::RemoteDisconnect> {
    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed("1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("the stream must end");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    details
}

/// Reads until the pool ends its side, ignoring what it sends.
fn drain_requests(socket: &mut MockStream) {
    while socket.read().is_some() {}
}

/// A message above the decode budget is refused before tonic buffers it, and
/// fails the turn as a protocol error naming both sizes.
#[tokio::test]
async fn a_message_above_the_decode_budget_is_refused_unbuffered() {
    let (mock, mut config) = mock_pool_config().await;
    config.max_decode_bytes = 100_000;
    let complete = event_kind(pb::child_event::Kind::Complete(pb::Complete::from(
        MontyObject::string("x".repeat(200_000)),
    )));
    let body = encode_to_capped_vec(&complete).expect("encode");
    let len = body.len();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1");
        let _ = socket.send(Bytes::from(body));
        drain_requests(&mut socket);
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed("1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("the result is over budget");
    let PoolError::Protocol(message) = err else {
        panic!("got {err:?}");
    };
    assert_eq!(
        message,
        format!("invalid payload from worker: frame of {len} bytes exceeds maximum of 100000 bytes")
    );
    drop(checkout);
    join_server(server).await;
}

// === teardown says goodbye ===

/// Reads until the client ends its side of the stream, panicking on a request.
///
/// Behind a proxy the half-close is the *only* signal the far side gets: some
/// load balancers forward it but never propagate a bare stream reset, so a
/// teardown that only drops the stream leaves the remote session alive until
/// its own idle timeout.
fn expect_end(socket: &mut MockStream) {
    if let Some(data) = socket.read() {
        panic!("expected the stream to end, got a {}-byte request", data.len());
    }
}

#[tokio::test]
async fn finishing_a_checkout_sends_a_close_frame() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
        );
        expect_end(&mut socket);
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn an_abandoned_checkout_sends_a_close_frame() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_end(&mut socket);
    });

    let (_pool, checkout) = remote_checkout(&url).await;
    // dropping a checkout is a synchronous teardown, so the goodbye is sent by
    // a detached task rather than awaited here
    drop(checkout);
    join_server(server).await;
}

#[tokio::test]
async fn a_timed_out_turn_sends_a_close_frame() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "while True:\n    pass");
        // never reply: the client's deadline must end the session, and the
        // abandoned-session case is exactly when the server most wants the slot
        expect_end(&mut socket);
    });

    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_millis(200));
    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("the turn must time out");
    assert!(matches!(err, PoolError::Timeout { .. }), "got {err:?}");
    join_server(server).await;
}

/// `finish()` never waits on the server: the half-close is queued and the
/// checkout returns, so a server that takes its time ending the stream (a relay
/// parks the session first) adds nothing to the host's teardown.
#[tokio::test]
async fn finishing_a_checkout_does_not_wait_for_the_server_to_end_the_stream() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_end(&mut socket);
        // the half-close has landed; take a while to end the stream in turn
        thread::sleep(Duration::from_millis(500));
        drop(socket);
    });

    let (_pool, checkout) = remote_checkout(&url).await;
    let start = Instant::now();
    checkout.finish().await.expect("finish");
    assert!(
        start.elapsed() < Duration::from_millis(250),
        "finish waited {:?} for the server",
        start.elapsed()
    );
    join_server(server).await;
}

/// Pool teardown keeps connection credentials alive until a departed stream finishes closing.
#[tokio::test]
async fn closing_a_pool_waits_for_departed_streams() {
    for abandon in [false, true] {
        let mock = MockServer::start().await;
        let url = mock.url();
        let (ended, end_seen) = oneshot::channel();
        let (release, wait_for_release) = mpsc::channel::<()>();
        let server = thread::spawn(move || {
            let mut socket = mock.accept();
            expect_end(&mut socket);
            ended.send(()).unwrap();
            wait_for_release.recv().unwrap();
        });
        let (pool, checkout) = remote_checkout(&url).await;
        if abandon {
            drop(checkout);
        } else {
            checkout.finish().await.unwrap();
        }
        end_seen.await.unwrap();
        let mut first_close = Box::pin(pool.close());
        let mut second_close = Box::pin(pool.close());
        assert!(futures_util::poll!(first_close.as_mut()).is_pending());
        assert!(futures_util::poll!(second_close.as_mut()).is_pending());
        release.send(()).unwrap();
        tokio::join!(first_close, second_close);
        join_server(server).await;
    }
}

/// A peer that never ends its response cannot hold pool teardown beyond the close grace.
#[tokio::test]
async fn closing_a_pool_keeps_the_remote_close_bounded() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let (ended, end_seen) = oneshot::channel();
    let (release, wait_for_release) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_end(&mut socket);
        ended.send(()).unwrap();
        wait_for_release.recv().unwrap();
    });
    let (pool, checkout) = remote_checkout(&url).await;
    assert!(futures_util::poll!(Box::pin(pool.close()).as_mut()).is_ready());
    checkout.finish().await.unwrap();
    end_seen.await.unwrap();
    let mut close = Box::pin(pool.close());
    assert!(futures_util::poll!(close.as_mut()).is_pending());
    timeout(Duration::from_secs(3), close)
        .await
        .expect("the existing close grace bounds an unresponsive peer");
    release.send(()).unwrap();
    join_server(server).await;
}

/// `finish()` against a peer that stopped reading must neither block nor leak
/// the worker's capacity slot.
///
/// The socket is wedged first: one oversized feed the mock server never drains
/// fills the buffers, and every later write queues behind it. The goodbye
/// queues too, in the detached reader, while the slot comes back at once.
///
/// Not run on Windows: dynamic send buffering grows the loopback socket's
/// kernel buffer past the wedge, so the feed never blocks (the client socket
/// is dialled inside `connect_async`, so its `SO_SNDBUF` — the only way to
/// disable the growth — is out of reach). Do not "fix" this by enlarging the
/// wedge, which only moves the flake.
#[cfg(not(windows))]
#[tokio::test]
async fn finishing_against_a_wedged_socket_does_not_leak_capacity() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let (release, wait_for_release) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        // never read again: the client's writes now back up in the socket
        wait_for_release.recv().expect("release");
    });

    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.checkout_timeout = Some(Duration::from_millis(200));
    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");

    let wedge = "#".repeat(32 * 1024 * 1024);
    let mut on_print = no_print;
    let feed = checkout.feed(&wedge, vec![], vec![], false, &mut on_print);
    assert!(
        timeout(Duration::from_secs(1), feed).await.is_err(),
        "the unread feed must block, wedging the socket"
    );
    timeout(Duration::from_millis(200), checkout.finish())
        .await
        .expect("the goodbye must not block behind the wedged socket")
        .expect("finish");

    release.send(()).expect("release the server");
    join_server(server).await;

    // The slot came back: the next checkout gets as far as dialling instead
    // of reporting no capacity. The mock is shutting down, so the dial fails
    // (`Spawn`) or, if it still admitted the stream, the stream ends at once
    // (`Disconnected`); only `Exhausted` would be the leak.
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("the mock is shutting down, so a checkout cannot succeed");
    };
    assert!(!matches!(err, PoolError::Exhausted), "the slot leaked: {err:?}");
}

/// A finished checkout's slot comes back once the server has ended the stream,
/// not at once: a full pool's next checkout must not reach a server still
/// counting the last one, which a relay's per-caller cap would refuse.
#[tokio::test]
async fn a_full_pool_reuses_a_slot_once_the_server_ends_the_stream() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut first = mock.accept();
        expect_end(&mut first);
        let early = mock.try_accept(Duration::from_millis(300));
        assert!(
            early.is_none(),
            "the next stream opened before the server ended this one"
        );
        drop(first);
        let mut second = mock.accept();
        expect_end(&mut second);
    });

    let (pool, checkout) = remote_checkout(&url).await;
    checkout.finish().await.expect("finish");
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("second checkout");
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A checkout dropped outside any runtime, after the `block_on` that made it
/// returned, still half-closes on the runtime its stream was opened on rather
/// than resetting the stream under a server still ending it.
#[test]
fn a_checkout_dropped_outside_its_runtime_still_half_closes() {
    let runtime = Builder::new_multi_thread().enable_all().build().expect("runtime");
    let mock = runtime.block_on(MockServer::start());
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_end(&mut socket);
        assert!(stays_open(&socket), "the stream was reset");
    });

    let (pool, checkout) = runtime.block_on(remote_checkout(&url));
    drop(checkout);
    server.join().expect("server thread");
    drop(pool);
}

/// Closed streams waiting on their server are capped at `max_processes`: past
/// that a close resets its stream, so peers that never end theirs cannot pile
/// up buffered streams as fast as checkouts are abandoned.
#[tokio::test]
async fn closes_past_the_pool_size_reset_their_streams() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut first = mock.accept();
        expect_end(&mut first);
        let mut second = mock.accept();
        expect_end(&mut second);
        // neither is ended here: the first holds the pool's one closing slot
        let (first_open, second_open) = (stays_open(&first), stays_open(&second));
        assert!(first_open, "the first close must wait for the server");
        assert!(!second_open, "the close past the cap must reset its stream");
    });

    let (pool, checkout) = remote_checkout(&url).await;
    drop(checkout);
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("second checkout");
    drop(checkout);
    join_server(server).await;
}

/// Whether the pool still holds `socket`'s stream a moment after it ended its
/// side: a reset stream drops the response, so a send then fails.
fn stays_open(socket: &MockStream) -> bool {
    thread::sleep(Duration::from_millis(200));
    socket.send(Bytes::new()).is_ok()
}

#[tokio::test]
async fn a_dropped_connection_is_a_disconnect() {
    // Every server policy action other than shutdown (idle/session/turn
    // timeout, capacity) ends the stream with a status. The client cannot tell
    // the sandbox's fate from that, and says so: `Disconnected`, never
    // `Crashed` — the remote process is not ours to reap.
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
        );
        // the server drops the session while the client sits idle, then exits
        socket.close(Some(Cause::IdleTimeout.status("idle timeout")));
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
        "got {event:?}"
    );
    // joining first guarantees the server side is fully torn down before the
    // next feed — the failure mode this test pins
    join_server(server).await;
    let err = checkout
        .feed("x", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("a closed connection must fail the turn");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, Some("DEADLINE_EXCEEDED"));
    assert_eq!(details.status_message.as_deref(), Some("idle timeout"));
    assert_eq!(details.cause, Some(RemoteCause::IdleTimeout));
    assert_eq!(details.transport_error, None);
}

/// The suspension position every hand-built event carries.
fn position() -> SourceRange {
    SourceRange {
        filename: "main.py".to_owned(),
        start: 0,
        end: 7,
    }
}

/// Discard the worker so an invalid `TypingError` cannot falsely end a suspended feed.
#[tokio::test]
async fn a_typing_error_reply_to_dump_is_a_protocol_violation() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        try_read_request(&mut socket).expect("configure");
        send_kind(&mut socket, ok_event());
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Dump(_)));
        send_kind(
            &mut socket,
            pb::child_event::Kind::TypingError(pb::TypingError {
                diagnostics: "main.py:1:1: error[unresolved-import]".to_owned(),
            }),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout.dump().await.unwrap_err();
    assert!(matches!(err, PoolError::Protocol(_)), "got {err:?}");
    assert_eq!(
        err.to_string(),
        "monty worker protocol error: TypingError reply to a request that is not a type-checked Feed"
    );
    assert!(checkout.worker_id().is_none(), "the worker must be discarded");
    join_server(server).await;
}

#[tokio::test]
async fn a_typing_error_reply_to_a_skipped_feed_is_a_protocol_violation() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        try_read_request(&mut socket).expect("configure");
        send_kind(&mut socket, ok_event());
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Feed(feed) if feed.skip_type_check
        ));
        send_kind(
            &mut socket,
            pb::child_event::Kind::TypingError(pb::TypingError {
                diagnostics: "main.py:1:1: error[unresolved-import]".to_owned(),
            }),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("import nope", vec![], vec![], true, &mut no_print)
        .await
        .unwrap_err();
    assert!(matches!(err, PoolError::Protocol(_)), "got {err:?}");
    assert_eq!(
        err.to_string(),
        "monty worker protocol error: TypingError reply to a request that is not a type-checked Feed"
    );
    assert!(checkout.worker_id().is_none(), "the worker must be discarded");
    join_server(server).await;
}

/// Budget exhaustion discards the connection without replaying the feed.
#[tokio::test]
async fn a_host_decode_limit_rejects_a_result_and_discards_the_connection() {
    let (mock, mut config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        socket.set_read_timeout(Duration::from_secs(5));
        expect_configure(&mut socket);
        expect_feed(&mut socket, "42");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
        );
        expect_end(&mut socket);
    });
    config.max_decode_bytes = 0;
    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let error = checkout
        .feed("42", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("decode budget");
    assert!(matches!(error, PoolError::Protocol(_)));
    assert!(checkout.worker_id().is_none());
    drop(checkout);
    drop(pool);
    join_server(server).await;
}

/// The server selects CPU mode and enforces it beside the child.
#[tokio::test]
#[cfg(target_os = "linux")]
async fn server_selects_cpu_clock_and_enforces_it_beside_the_remote_child() {
    let binary = binary::monty_binary();
    let (mock, mut config) = mock_pool_config().await;
    let (tx, rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        let pb::parent_request::Kind::Configure(configure) = read_request(&mut socket) else {
            panic!("expected Configure")
        };
        assert_eq!(configure.execution_clock, None);
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut config = PoolConfig::subprocess(binary);
            config.execution_clock = ExecutionClock::Cpu;
            config.min_processes = 0;
            config.max_processes = 1;
            config.request_timeout = Some(Duration::from_secs(2));
            config.feed_duration_limit_grace = Some(Duration::ZERO);
            config.turn_duration_limit_grace = Some(Duration::ZERO);
            let pool = Pool::new(config).await.unwrap();
            let mut child = pool
                .checkout(&ReplConfig {
                    limits: configure.limits.map(ResourceLimits::from),
                    ..ReplConfig::default()
                })
                .await
                .unwrap();
            let ok = pb::ChildEvent {
                kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
                execution_clock: Some(pb::ExecutionClock::Cpu as i32),
                ..Default::default()
            };
            send_event(&mut socket, &ok);
            let request = try_read_request(&mut socket).expect("Feed");
            let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
            let outcome = child.turn_raw(&request, &mut on_event).await;
            tx.send(matches!(outcome, Err(PoolError::CpuLimit { .. }))).unwrap();
            assert_eq!(
                child.pid(),
                None,
                "CPU exhaustion must discard and reap the server's child"
            );
            drop(child);
            pool.close().await;
        });
        socket.close(None);
    });
    config.request_timeout = Some(Duration::from_secs(5));
    let pool = Pool::new(config).await.unwrap();
    let mut remote = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(50))),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    assert!(
        remote
            .feed("while True: pass", vec![], vec![], false, &mut no_print)
            .await
            .is_err()
    );
    drop(remote);
    pool.close().await;
    join_server(server).await;
    assert!(rx.recv().unwrap(), "the server must enforce the independent CPU limit");
}

#[tokio::test]
async fn invalid_server_clock_configuration_is_rejected_before_execution() {
    for (clock, deadline) in [
        (Some(42), Some(Duration::from_secs(2))),
        (Some(pb::ExecutionClock::Cpu as i32), None),
        (Some(pb::ExecutionClock::Cpu as i32), Some(Duration::MAX)),
    ] {
        let (mock, mut config) = mock_pool_config().await;
        let server = thread::spawn(move || {
            let mut socket = mock.accept();
            socket.set_read_timeout(Duration::from_secs(5));
            assert!(matches!(
                read_request(&mut socket),
                pb::parent_request::Kind::Configure(_)
            ));
            let event = pb::ChildEvent {
                kind: Some(pb::child_event::Kind::Ok(pb::Ok {})),
                execution_clock: clock,
                ..Default::default()
            };
            send_event(&mut socket, &event);
            // the feed was pipelined behind the Configure, so it is already
            // here; the client must end the stream rather than answer it
            assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
            assert!(
                socket.read().is_none(),
                "invalid clock metadata must not execute requests"
            );
        });
        config.request_timeout = deadline;
        let pool = Pool::new(config).await.unwrap();
        let mut checkout = pool
            .checkout(&ReplConfig {
                limits: Some(ResourceLimits::default()),
                ..ReplConfig::default()
            })
            .await
            .expect("checkout");
        let err = checkout
            .feed("1 + 1", vec![], vec![], false, &mut no_print)
            .await
            .expect_err("invalid configuration must fail");
        if clock == Some(42) {
            assert!(matches!(err, PoolError::Protocol(_)));
            insta::assert_snapshot!(err.to_string(), @"monty worker protocol error: worker reported an unknown execution clock");
        } else {
            assert!(matches!(err, PoolError::Configuration(_)));
            insta::allow_duplicates! {
                insta::assert_snapshot!(err.to_string(), @"CPU execution clock requires a finite client request_timeout");
            }
        }
        drop(checkout);
        pool.close().await;
        join_server(server).await;
    }
}

/// A refused first `Load` restores the budget it snapshotted, which must keep
/// the clock the `Configure` reply chose: a CPU-clock server's slow feed is not
/// cut short by a wall-time backstop.
#[tokio::test]
async fn a_refused_first_load_keeps_the_server_clock() {
    let (mock, mut config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        let mut configured = event_kind(ok_event());
        configured.execution_clock = Some(pb::ExecutionClock::Cpu as i32);
        send_event(&mut socket, &configured);
        expect_load(&mut socket, b"bad-dump");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Error(pb::Error {
                exception: Some(pb::RaisedException {
                    exc_type: "RuntimeError".to_owned(),
                    message: Some("unknown session".to_owned()),
                    traceback: vec![].into(),
                    data: None,
                }),
            }),
        );
        expect_feed(&mut socket, "42");
        // well past the 50ms feed limit a wall clock would enforce here
        thread::sleep(Duration::from_millis(150));
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
        );
        let _ = socket.read();
    });
    config.request_timeout = Some(Duration::from_secs(2));
    config.feed_duration_limit_grace = Some(Duration::ZERO);
    let pool = Pool::new(config).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(50))),
            ..Default::default()
        })
        .await
        .unwrap();
    let refused = session.restore(b"bad-dump".to_vec(), vec![], &mut no_print).await;
    assert!(matches!(refused, Err(PoolError::Runtime(_))), "{refused:?}");
    let event = session.feed("42", vec![], vec![], false, &mut no_print).await.unwrap();
    assert!(matches!(event, TurnEvent::Complete(ref result) if *result == MontyObject::int(42)));
    drop(session);
    pool.close().await;
    join_server(server).await;
}

/// Checks that a remote dial failure is typed and returns its full message.
fn connect_message(err: &PoolError) -> String {
    assert!(matches!(err, PoolError::Connect { .. }), "got {err:?}");
    err.to_string()
}

#[tokio::test]
async fn remote_clock_is_reported_by_the_server_without_a_client_preference() {
    for clock in [None, Some(pb::ExecutionClock::Wall), Some(pb::ExecutionClock::Cpu)] {
        let (mock, mut config) = mock_pool_config().await;
        let server = thread::spawn(move || {
            let mut socket = mock.accept();
            let pb::parent_request::Kind::Configure(configure) = read_request(&mut socket) else {
                panic!("expected Configure")
            };
            assert_eq!(configure.execution_clock, None);
            let mut configured = event_kind(ok_event());
            configured.execution_clock = clock.map(|clock| clock as i32);
            send_event(&mut socket, &configured);
            assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
            if clock == Some(pb::ExecutionClock::Cpu) {
                thread::sleep(Duration::from_millis(150));
            }
            send_kind(
                &mut socket,
                pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
            );
            let _ = socket.read();
        });
        config.request_timeout = Some(Duration::from_secs(2));
        config.feed_duration_limit_grace = Some(Duration::ZERO);
        let pool = Pool::new(config).await.unwrap();
        let mut session = pool
            .checkout(&ReplConfig {
                limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(50))),
                ..Default::default()
            })
            .await
            .unwrap();
        let event = session.feed("42", vec![], vec![], false, &mut no_print).await.unwrap();
        assert!(matches!(event, TurnEvent::Complete(ref result) if *result == MontyObject::int(42)));
        drop(session);
        pool.close().await;
        join_server(server).await;
    }
}
