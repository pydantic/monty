//! Drives the pool's WebSocket transport against a mock child server: a thread
//! that accepts one WebSocket connection and serves a scripted protocol
//! session. This exercises `Worker::websocket` (the async dial) and the WS
//! send/recv path end-to-end without needing a real remote child.

use std::{
    fs,
    future::ready,
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use monty_pool::{
    Checkout, MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, PrintFuture, ReplConfig, ResumeValue, TurnEvent,
};
use monty_proto::{WireFunctionCall, decode_frame, encode_to_capped_vec, pb};
use monty_types::{MontyObject, PrintStream, ResourceLimits};
use tokio::task::spawn_blocking;
use tungstenite::{Message, WebSocket};

/// A mock child: accepts one WebSocket connection and answers each request with
/// the obvious turn-ender (`Ok` for control requests, `Complete(42)` for a feed).
fn serve_mock_child(listener: &TcpListener) {
    let (stream, _peer) = listener.accept().expect("accept");
    let mut socket = tungstenite::accept(stream).expect("ws handshake");
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
/// `feed`.
///
/// `turn_raw` used to arm `request_timeout` alone, so a session whose only
/// bound was `max_duration` — the shape a serving relay configures, since it
/// applies limits rather than client timeouts — had no parent-side deadline at
/// all, and a wedged child held the turn indefinitely.
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

/// A raw `Load` re-adopts the dump's duration budget, exactly as `restore`.
///
/// `turn_raw` used to keep the `Configure`-time budget across a `Load`, so a
/// restored session's backstop measured it against the wrong limit — here the
/// checkout's own 5s budget would have left a wedged post-restore turn running
/// for 5.3s instead of being killed at the dump's 100ms + grace.
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

/// A `ShutdownDump` ends a raw turn *and* the connection behind it.
///
/// The frame is handed back — the dump it carries is the driver's to forward —
/// but the serving relay that sent it is gone, so the worker is discarded as on
/// the typed path. It used to be kept, resurfacing later as an unexplained
/// crash on the next turn.
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
        method_call: false,
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
