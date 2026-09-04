//! Drives the pool's WebSocket transport against a mock child server: a thread
//! that accepts one WebSocket connection and serves a scripted protocol
//! session. This exercises `Worker::websocket` (the async dial) and the WS
//! send/recv path end-to-end without needing a real remote child.

#[cfg(feature = "telemetry")]
use std::sync::Arc;
use std::{
    fs,
    future::ready,
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};

#[cfg(feature = "telemetry")]
use monty_pool::telemetry::{TelemetryAdapter, configure_telemetry_adapter};
use monty_pool::{
    Checkout, CheckoutOptions, MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, PrintFuture, ReplConfig,
    ResumeValue, TurnEvent,
};
use monty_proto::{MAX_FRAME_LEN, WireFunctionCall, WireObject, decode_frame, encode_to_capped_vec, pb};
use monty_types::{MontyObject, PrintStream, ResourceLimits};
#[cfg(feature = "telemetry")]
use opentelemetry::trace::{SpanId, TraceId};
#[cfg(feature = "telemetry")]
use opentelemetry_sdk::{logs::SdkLogRecord, trace::SpanData};
use tokio::task::spawn_blocking;
#[cfg(not(windows))]
use tokio::time::timeout;
use tungstenite::{
    Message, WebSocket,
    handshake::server::{Request, Response},
};

/// A mock child: accepts one WebSocket connection and answers each request with
/// the obvious turn-ender (`Ok` for control requests, `Complete(42)` for a feed).
fn serve_mock_child(listener: &TcpListener) {
    let (stream, _peer) = listener.accept().expect("accept");
    let mut socket = tungstenite::accept(stream).expect("ws handshake");
    answer_requests(&mut socket);
}

/// Answers each request on an accepted socket with the obvious turn-ender
/// (`Ok` for control requests, `Complete(42)` for a feed) until the peer
/// disconnects.
fn answer_requests(socket: &mut WebSocket<TcpStream>) {
    while let Ok(Message::Binary(data)) = socket.read() {
        let request = decode_frame::<pb::ParentRequest>(data.as_ref()).expect("decode request");
        let kind = match request.kind.expect("request kind") {
            pb::parent_request::Kind::Feed(_) => pb::child_event::Kind::Complete(pb::Complete {
                value: Some(MontyObject::Int(42).into()),
            }),
            // Configure / Reset / Shutdown / anything else: acknowledge.
            _ => pb::child_event::Kind::Ok(pb::Ok {}),
        };
        let event = pb::ChildEvent {
            kind: Some(kind),
            ..Default::default()
        };
        let body = encode_to_capped_vec(&event).expect("encode event");
        socket.send(Message::Binary(body.into())).expect("send event");
    }
}

/// A mock child that also reports each of `headers`' values (in order, `None`
/// when absent) from each of `connections` upgrade requests — the handshake
/// callback is where a server reads them.
fn serve_capturing_headers(
    listener: &TcpListener,
    headers: &[&str],
    connections: usize,
    tx: &mpsc::Sender<Vec<Option<String>>>,
) {
    for _ in 0..connections {
        let (stream, _peer) = listener.accept().expect("accept");
        // the Err type (an ErrorResponse) is tungstenite's callback contract
        #[expect(clippy::result_large_err)]
        let callback = |req: &Request, resp: Response| {
            let seen = headers
                .iter()
                .map(|header| {
                    req.headers()
                        .get(*header)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned)
                })
                .collect();
            tx.send(seen).expect("send captured headers");
            Ok(resp)
        };
        let mut socket = tungstenite::accept_hdr(stream, callback).expect("ws handshake");
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
async fn drives_a_session_over_websocket() {
    // Bind before spawning so the port is listening before the pool connects.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || serve_mock_child(&listener));

    let mut config = PoolConfig::websocket(format!("ws://127.0.0.1:{port}"));
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

    // The WebSocket worker has no local pid.
    assert_eq!(checkout.pid(), None);

    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::Complete(MontyObject::Int(42))),
        "got {event:?}"
    );

    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// Each checkout's `CheckoutOptions::connect_headers` reach that checkout's
/// own upgrade request. WebSocket workers are single-use, so two checkouts
/// are two dials, and a `traceparent` rendered at checkout time — the
/// motivating case — must not land on the wrong one.
#[tokio::test]
async fn connect_headers_are_per_checkout() {
    let (header_tx, header_rx) = mpsc::channel();
    let (listener, mut config) = ws_pool_config();
    let server = thread::spawn(move || serve_capturing_headers(&listener, &["traceparent"], 2, &header_tx));

    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    for trace in ["00-checkout-1", "00-checkout-2"] {
        let repl = ReplConfig {
            script_name: "test.py".to_owned(),
            ..ReplConfig::default()
        };
        let options =
            CheckoutOptions::default().with_connect_headers(vec![("traceparent".to_owned(), trace.to_owned())]);
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
            matches!(event, TurnEvent::Complete(MontyObject::Int(42))),
            "got {event:?}"
        );
        checkout.finish().await.expect("finish");
    }
    join_server(server).await;
}

/// Duplicate header names are last-wins, as [`CheckoutOptions`] promises.
#[tokio::test]
async fn duplicate_connect_headers_are_last_wins() {
    let (header_tx, header_rx) = mpsc::channel();
    let (listener, mut config) = ws_pool_config();
    let server = thread::spawn(move || serve_capturing_headers(&listener, &["traceparent"], 1, &header_tx));
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    let options = CheckoutOptions::default().with_connect_headers(vec![
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

/// Every dial carries a `User-Agent` naming this crate and its version, and a
/// `connect_headers` entry of the same name replaces it rather than being
/// appended, so a caller can identify their own product instead.
#[tokio::test]
async fn user_agent_is_set_and_overridable() {
    let (header_tx, header_rx) = mpsc::channel();
    let (listener, mut config) = ws_pool_config();
    let server = thread::spawn(move || serve_capturing_headers(&listener, &["user-agent"], 2, &header_tx));
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");

    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    assert_eq!(
        header_rx.recv().expect("captured headers"),
        vec![Some(format!("monty-pool/{}", env!("CARGO_PKG_VERSION")))]
    );
    checkout.finish().await.expect("finish");

    let options =
        CheckoutOptions::default().with_connect_headers(vec![("User-Agent".to_owned(), "my-app/1.0".to_owned())]);
    let checkout = pool
        .checkout_with(&ReplConfig::default(), options)
        .await
        .expect("checkout");
    assert_eq!(
        header_rx.recv().expect("captured headers"),
        vec![Some("my-app/1.0".to_owned())]
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

/// A host trace context captured through the telemetry adapter goes out as
/// W3C `traceparent`/`tracestate` on the dial, so a remote worker's spans join
/// the host's trace with no header plumbing by the binding; an explicit
/// `connect_headers` entry still wins.
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
    let (listener, mut config) = ws_pool_config();
    let server =
        thread::spawn(move || serve_capturing_headers(&listener, &["traceparent", "tracestate"], 2, &header_tx));
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
        .with_connect_headers(vec![("traceparent".to_owned(), "00-explicit".to_owned())]);
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
/// is needed: the checkout fails loudly with the promised dial error instead
/// of the pair being silently dropped.
#[tokio::test]
async fn malformed_connect_headers_fail_the_dial() {
    let cases = [
        (
            ("bad name", "v"),
            "ws://127.0.0.1:9: connect header \"bad name\": invalid HTTP header name",
        ),
        (
            ("traceparent", "line\nbreak"),
            "ws://127.0.0.1:9: connect header \"traceparent\" value: failed to parse header value",
        ),
    ];
    let pool = Pool::new(PoolConfig::websocket("ws://127.0.0.1:9"))
        .await
        .expect("pool");
    for ((name, value), expected) in cases {
        let options = CheckoutOptions::default().with_connect_headers(vec![(name.to_owned(), value.to_owned())]);
        let Err(err) = pool.checkout_with(&ReplConfig::default(), options).await else {
            panic!("malformed header must fail the dial");
        };
        let PoolError::Spawn(msg) = err else {
            panic!("expected Spawn error, got {err:?}");
        };
        assert_eq!(msg, expected);
    }
}

/// A URL that cannot form an upgrade request fails the dial with the same
/// `{url}: {err}` shape as every other dial failure.
#[tokio::test]
async fn an_unparsable_url_fails_the_dial() {
    let pool = Pool::new(PoolConfig::websocket("not a url")).await.expect("pool");
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("unparsable URL must fail the dial");
    };
    let PoolError::Spawn(msg) = err else {
        panic!("expected Spawn error, got {err:?}");
    };
    assert_eq!(msg, "not a url: HTTP format error: invalid uri character");
}

/// `Debug` for [`CheckoutOptions`] names each connect header, never its value.
#[test]
fn connect_headers_debug_is_redacted() {
    let options =
        CheckoutOptions::default().with_connect_headers(vec![("authorization".to_owned(), "Bearer secret".to_owned())]);
    assert_eq!(
        format!("{options:?}"),
        r#"CheckoutOptions { connect_headers: ["authorization"], .. }"#
    );
}

/// Binds a listener and returns it with a websocket pool config pointing at it.
fn ws_pool_config() -> (TcpListener, PoolConfig) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let mut config = PoolConfig::websocket(format!("ws://127.0.0.1:{port}"));
    config.max_processes = 1;
    (listener, config)
}

/// Accepts one connection and returns the accepted socket.
fn accept_ws(listener: &TcpListener) -> WebSocket<TcpStream> {
    let (stream, _peer) = listener.accept().expect("accept");
    tungstenite::accept(stream).expect("ws handshake")
}

/// Reads one framed `ParentRequest` from the mock child's socket.
fn read_request(socket: &mut WebSocket<TcpStream>) -> pb::parent_request::Kind {
    let Ok(Message::Binary(data)) = socket.read() else {
        panic!("expected a binary request frame");
    };
    decode_frame::<pb::ParentRequest>(data.as_ref())
        .expect("decode request")
        .kind
        .expect("request kind")
}

/// Sends one event from the mock child.
fn send_event(socket: &mut WebSocket<TcpStream>, event: &pb::ChildEvent) {
    let body = encode_to_capped_vec(event).expect("encode event");
    socket.send(Message::Binary(body.into())).expect("send event");
}

fn event_kind(kind: pb::child_event::Kind) -> pb::ChildEvent {
    pb::ChildEvent {
        kind: Some(kind),
        ..Default::default()
    }
}

/// The headline scenario for parent-side mounts: the worker lives on the far
/// side of a WebSocket, yet a mounted read is serviced from the *parent's*
/// filesystem — the mock child emits the `OsCall` and `resume_from_mounts`
/// answers it with the file's contents, no host path ever crossing the wire.
#[tokio::test]
async fn mounted_reads_are_serviced_from_the_parent_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "parent-side bytes").unwrap();

    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
                call: Some(pb::os_call::Call::ReadText("/mnt/data.txt".to_owned())),
            })),
        );
        // the parent answers with the mounted file's contents
        let pb::parent_request::Kind::ResumeCall(resume) = read_request(&mut socket) else {
            panic!("expected ResumeCall");
        };
        assert_eq!(resume.call_id, 7);
        let Some(pb::ext_function_result::Kind::ReturnValue(value)) = resume.result.and_then(|r| r.kind) else {
            panic!("expected a ReturnValue result");
        };
        let value = value.into_object().expect("valid value");
        assert_eq!(value, MontyObject::String("parent-side bytes".to_owned()));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Complete(pb::Complete {
                value: Some(MontyObject::String("done".to_owned()).into()),
            })),
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
        matches!(&event, TurnEvent::Complete(MontyObject::String(s)) if s == "done"),
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

    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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

/// The parent-side `max_duration` backstop (remaining budget + grace) kills a
/// worker that never answers a feed — the case where the child's own time
/// enforcement has failed. No `request_timeout` is configured, so the
/// backstop is the only armed deadline.
#[tokio::test]
async fn duration_backstop_kills_an_unresponsive_worker() {
    let (listener, mut config) = ws_pool_config();
    config.duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
            limits: Some(ResourceLimits::default().max_duration(Duration::from_millis(100))),
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
/// only bound was `max_duration` had no parent-side deadline at all.
#[tokio::test]
async fn duration_backstop_arms_on_the_raw_path() {
    let (listener, mut config) = ws_pool_config();
    config.duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
            limits: Some(ResourceLimits::default().max_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "while True:\n    pass".to_owned(),
            inputs: vec![],
            skip_type_check: false,
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
    let (listener, mut config) = ws_pool_config();
    config.duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
                max_duration_micros: Some(100_000),
                max_suspensions: None,
                restored_script_name: None,
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
            limits: Some(ResourceLimits::default().max_duration(Duration::from_secs(5))),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let load = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Load(pb::Load { state: vec![1, 2, 3] })),
        ..pb::ParentRequest::default()
    };
    checkout.turn_raw(&load, &mut on_event).await.expect("load");
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "while True:\n    pass".to_owned(),
            inputs: vec![],
            skip_type_check: false,
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        // the refused requests never arrive: the next frame is the Feed
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Complete(pb::Complete {
                value: Some(MontyObject::Int(2).into()),
            })),
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
            inputs: vec![],
            skip_type_check: false,
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
    let (listener, mut config) = ws_pool_config();
    config.duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
            limits: Some(ResourceLimits::default().max_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .expect("checkout");
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let load = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Load(pb::Load {
            state: vec![0; MAX_FRAME_LEN as usize + 1],
        })),
        ..pb::ParentRequest::default()
    };
    let err = checkout.turn_raw(&load, &mut on_event).await.unwrap_err();
    assert!(matches!(err, PoolError::Runtime(_)), "got {err:?}");
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "while True:\n    pass".to_owned(),
            inputs: vec![],
            skip_type_check: false,
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        send_event(
            &mut socket,
            &event_kind(pb::child_event::Kind::Shutdown(pb::ShutdownDump {
                dump: Some(b"relay-signed state".to_vec()),
            })),
        );
    });

    let pool = Pool::new(config).await.expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![],
            skip_type_check: false,
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
/// Servicing a covered call is now a separate turn with its own deadline (see
/// "Mount I/O is not covered by `request_timeout`" in
/// limitations/pool-architecture.md), so a *loop* of covered calls is bounded
/// by `max_duration`, not by `request_timeout`.
#[tokio::test]
async fn a_mounted_feed_turn_is_still_bounded_by_the_request_timeout() {
    let dir = tempfile::tempdir().unwrap();

    let (listener, mut config) = ws_pool_config();
    config.request_timeout = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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

/// A restored session re-adopts its `max_duration` budget from the timing
/// fields the worker stamps on the `Load` reply, re-arming the parent-side
/// backstop without the parent ever seeing the original `ReplConfig`.
#[tokio::test]
async fn restored_session_rearms_the_duration_backstop() {
    let (listener, mut config) = ws_pool_config();
    config.duration_limit_grace = Some(Duration::from_millis(300));
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
                total_execution_micros: 0,
                max_duration_micros: Some(100_000),
                max_suspensions: None,
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
fn serve_endless_suspensions(socket: &mut WebSocket<TcpStream>, expected_calls: u32) {
    let function_call = |call_id: u32| {
        event_kind(pb::child_event::Kind::FunctionCall(WireFunctionCall {
            function_name: "fetch".to_owned(),
            args: vec![],
            kwargs: vec![],
            call_id,
            object_id: None,
        }))
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
        .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
        .await
        .expect("second suspension");
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
        .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_feed(&mut socket, "open('x')");
        // the event shrinks the limit to zero itself, so this suspension is
        // over budget on arrival
        send_event(
            &mut socket,
            &pb::ChildEvent {
                kind: Some(pb::child_event::Kind::OsCall(pb::OsCall { call_id: 1, call: None })),
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
    assert_eq!(msg, "OsCall event with no call");
    join_server(server).await;
}

/// Enforces suspension limits when a relay uses the raw path.
#[tokio::test]
async fn suspension_limit_is_enforced_on_the_raw_path() {
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
            inputs: vec![],
            skip_type_check: false,
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&feed, &mut on_event).await.expect("feed");
    assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: 1,
            result: Some(pb::ExtFunctionResult {
                kind: Some(pb::ext_function_result::Kind::ReturnValue(WireObject::new(
                    MontyObject::None,
                ))),
            }),
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        let function_call = |call_id: u32| {
            event_kind(pb::child_event::Kind::FunctionCall(WireFunctionCall {
                function_name: "fetch".to_owned(),
                args: vec![],
                kwargs: vec![],
                call_id,
                object_id: None,
            }))
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
                        traceback: vec![],
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
            inputs: vec![],
            skip_type_check: false,
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&feed, &mut on_event).await.expect("feed");
    assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
    let load = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Load(pb::Load { state: vec![1, 2, 3] })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&load, &mut on_event).await.expect("refused load");
    assert!(matches!(event.kind, Some(pb::child_event::Kind::Error(_))));
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: 1,
            result: Some(pb::ExtFunctionResult {
                kind: Some(pb::ext_function_result::Kind::ReturnValue(WireObject::new(
                    MontyObject::None,
                ))),
            }),
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
        let (listener, config) = ws_pool_config();
        let server = thread::spawn(move || {
            let mut socket = accept_ws(&listener);
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
            .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
            .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
            .await
            .expect("resume");
    }
    let err = checkout
        .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        let function_call = |call_id: u32| WireFunctionCall {
            function_name: "fetch".to_owned(),
            args: vec![],
            kwargs: vec![],
            call_id,
            object_id: None,
        };
        let abort_reply = |socket: &mut WebSocket<TcpStream>| {
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
        kind: Some(pb::parent_request::Kind::Load(pb::Load { state: vec![1, 2, 3] })),
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
            inputs: vec![],
            skip_type_check: false,
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
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
        .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
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

/// Reads the next request frame; `None` once the client closed the socket.
fn try_read_request(socket: &mut WebSocket<TcpStream>) -> Option<pb::ParentRequest> {
    loop {
        match socket.read() {
            Ok(Message::Binary(data)) => {
                return Some(decode_frame::<pb::ParentRequest>(data.as_ref()).expect("decode request"));
            }
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(_) | Err(_) => return None,
        }
    }
}

/// Sends one `ChildEvent` with the given kind (timing fields zeroed).
fn send_kind(socket: &mut WebSocket<TcpStream>, kind: pb::child_event::Kind) {
    send_event(socket, &event_kind(kind));
}

/// Shorthand for the `Ok` acknowledgement event.
fn ok_event() -> pb::child_event::Kind {
    pb::child_event::Kind::Ok(pb::Ok {})
}

/// Builds a `ShutdownDump` turn-ender.
fn shutdown(dump: Option<&[u8]>) -> pb::child_event::Kind {
    pb::child_event::Kind::Shutdown(pb::ShutdownDump {
        dump: dump.map(<[u8]>::to_vec),
    })
}

/// Builds a `FunctionCall` suspension with the given call id.
fn function_call(call_id: u32) -> pb::child_event::Kind {
    pb::child_event::Kind::FunctionCall(WireFunctionCall {
        function_name: "ext".to_owned(),
        args: vec![],
        kwargs: vec![],
        call_id,
        object_id: None,
    })
}

/// Asserts the next request is `Configure` and acknowledges it.
fn expect_configure(socket: &mut WebSocket<TcpStream>) {
    let request = try_read_request(socket).expect("configure");
    assert!(
        matches!(request.kind, Some(pb::parent_request::Kind::Configure(_))),
        "expected Configure, got {request:?}"
    );
    send_kind(socket, ok_event());
}

/// Asserts the next request is `Feed` of `code`.
fn expect_feed(socket: &mut WebSocket<TcpStream>, code: &str) {
    let request = try_read_request(socket).expect("feed");
    match request.kind {
        Some(pb::parent_request::Kind::Feed(feed)) => assert_eq!(feed.code, code),
        other => panic!("expected Feed, got {other:?}"),
    }
}

/// Asserts the next request is `Load` and checks its state bytes.
fn expect_load(socket: &mut WebSocket<TcpStream>, state: &[u8]) {
    let request = try_read_request(socket).expect("load");
    match request.kind {
        Some(pb::parent_request::Kind::Load(load)) => assert_eq!(load.state, state),
        other => panic!("expected Load, got {other:?}"),
    }
}

/// A pool + checkout against `ws://127.0.0.1:{port}` with a 10s turn timeout.
async fn websocket_checkout(port: u16) -> (Pool, Checkout) {
    let pool = websocket_pool(port).await;
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    (pool, checkout)
}

/// A single-worker pool against `ws://127.0.0.1:{port}`.
async fn websocket_pool(port: u16) -> Pool {
    let mut config = PoolConfig::websocket(format!("ws://127.0.0.1:{port}"));
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    Pool::new(config).await.expect("pool")
}

#[tokio::test]
async fn shutdown_hands_back_a_restorable_dump() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || {
        // conn 1: a draining server intercepts the feed and replies with the dump
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        send_kind(&mut socket, shutdown(Some(b"fake-dump")));
        // conn 2: the client restores the dump by hand and re-runs the feed
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_load(&mut socket, b"fake-dump");
        send_kind(&mut socket, ok_event());
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete {
                value: Some(MontyObject::Int(42).into()),
            }),
        );
        while try_read_request(&mut socket).is_some() {}
    });

    let pool = websocket_pool(port).await;
    let mut checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("a draining server must not run the feed");
    let PoolError::Shutdown { dump: Some(dump) } = err else {
        panic!("expected Shutdown with a dump, got {err:?}");
    };
    assert_eq!(dump, b"fake-dump");

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
        matches!(event, TurnEvent::Complete(MontyObject::Int(42))),
        "got {event:?}"
    );
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn shutdown_during_a_suspension_carries_the_suspended_dump() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
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
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_load(&mut socket, b"susp-dump");
        send_kind(&mut socket, function_call(7));
        while try_read_request(&mut socket).is_some() {}
    });

    let pool = websocket_pool(port).await;
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
        .resume(ResumeValue::Return(MontyObject::Int(5)), &mut no_print)
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
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || {
        // a server that began draining before this session ever ran anything
        let mut socket = accept_ws(&listener);
        let request = try_read_request(&mut socket).expect("configure");
        assert!(matches!(request.kind, Some(pb::parent_request::Kind::Configure(_))));
        send_kind(&mut socket, shutdown(None));
    });

    let pool = websocket_pool(port).await;
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("checkout against a draining server must fail");
    };
    assert!(matches!(err, PoolError::Shutdown { dump: None }), "got {err:?}");
    join_server(server).await;
}

/// The load-bearing keepalive check: a ping sent while the client sits idle
/// (no turn in flight, so no `recv` polling the stream) is still answered,
/// because the background reader task polls the read half continuously.
/// Without that task the pong only goes out on the next turn — and a
/// keepalive server would have dropped the session as vanished long before.
#[tokio::test]
async fn pings_are_answered_while_idle() {
    let (listener, config) = ws_pool_config();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        // the client is now idle: nothing in flight, nothing awaited
        socket.send(Message::Ping(b"live".as_slice().into())).expect("ping");
        // bound the wait so a missing pong fails the test rather than hanging it
        socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        match socket.read() {
            Ok(Message::Pong(payload)) => assert_eq!(payload.as_ref(), b"live"),
            other => panic!("expected a Pong while the client is idle, got {other:?}"),
        }
    });

    let pool = Pool::new(config).await.expect("pool");
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    // go idle: hold the checkout without driving a turn while the server
    // pings; joining the server is what proves the pong arrived
    join_server(server).await;
    drop(checkout);
}

// === teardown says goodbye ===

/// Reads until the client's Close frame, panicking on anything else.
///
/// Behind a proxy this is the *only* signal the far side gets: some load
/// balancers forward a Close frame but never propagate a bare TCP disconnect
/// upstream, so a teardown that only drops the socket leaves the remote session
/// alive until its own idle timeout.
fn expect_close(socket: &mut WebSocket<TcpStream>) {
    loop {
        match socket.read() {
            Ok(Message::Close(_)) => return,
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            other => panic!("expected a Close frame, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn finishing_a_checkout_sends_a_close_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete {
                value: Some(MontyObject::Int(42).into()),
            }),
        );
        expect_close(&mut socket);
    });

    let (_pool, mut checkout) = websocket_checkout(port).await;
    checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn an_abandoned_checkout_sends_a_close_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_close(&mut socket);
    });

    let (_pool, checkout) = websocket_checkout(port).await;
    // dropping a checkout is a synchronous teardown, so the goodbye is sent by
    // a detached task rather than awaited here
    drop(checkout);
    join_server(server).await;
}

#[tokio::test]
async fn a_timed_out_turn_sends_a_close_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_feed(&mut socket, "while True:\n    pass");
        // never reply: the client's deadline must end the session, and the
        // abandoned-session case is exactly when the server most wants the slot
        expect_close(&mut socket);
    });

    let mut config = PoolConfig::websocket(format!("ws://127.0.0.1:{port}"));
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

/// Cancelling `finish()` mid-goodbye must not leak the worker's capacity slot.
///
/// The goodbye only *pends* against a peer that stopped reading, so the socket
/// is wedged first: one oversized feed the mock server never drains fills the
/// buffers, and every later write queues behind it. The worker is already out
/// of the `Checkout` by then, so without a `CapacityGuard` across the await
/// nothing releases its slot and the pool shrinks by one for good.
///
/// Not run on Windows: dynamic send buffering grows the loopback socket's
/// kernel buffer past the wedge, the goodbye never pends, and the cancel
/// window this test needs cannot be forced open (the client socket is dialled
/// inside `connect_async`, so its `SO_SNDBUF` — the only way to disable the
/// growth — is out of reach). The guarded logic is platform-independent; do
/// not "fix" this by enlarging the wedge, which only moves the flake.
#[cfg(not(windows))]
#[tokio::test]
async fn cancelling_finish_does_not_leak_capacity() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (release, wait_for_release) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        // never read again: the client's writes now back up in the socket
        wait_for_release.recv().expect("release");
    });

    let mut config = PoolConfig::websocket(format!("ws://127.0.0.1:{port}"));
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
    assert!(
        timeout(Duration::from_millis(200), checkout.finish()).await.is_err(),
        "the goodbye must block behind the wedged socket, cancelling inside the window"
    );

    release.send(()).expect("release the server");
    join_server(server).await;

    // the slot came back: the next checkout gets as far as dialling (and fails
    // to connect, the listener being gone) instead of reporting no capacity
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("the listener is gone, so a checkout cannot succeed");
    };
    assert!(matches!(err, PoolError::Spawn(_)), "got {err:?}");
}

#[tokio::test]
async fn a_dropped_connection_is_a_disconnect() {
    // Every server policy action other than shutdown (idle/session/turn
    // timeout, capacity) is just a closed socket. The client cannot tell those
    // from a dead sandbox, and says so: `Disconnected`, never `Crashed` — the
    // remote process is not ours to reap.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::child_event::Kind::Complete(pb::Complete {
                value: Some(MontyObject::Int(42).into()),
            }),
        );
        // the server drops the session while the client sits idle, then exits
        let _ = socket.close(None);
    });

    let (_pool, mut checkout) = websocket_checkout(port).await;
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::Complete(MontyObject::Int(42))),
        "got {event:?}"
    );
    // joining first guarantees the server side is fully torn down before the
    // next feed — the failure mode this test pins
    join_server(server).await;
    let err = checkout
        .feed("x", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("a closed connection must fail the turn");
    assert!(matches!(err, PoolError::Disconnected { .. }), "got {err:?}");
}
