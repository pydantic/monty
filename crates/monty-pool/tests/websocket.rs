//! Drives the pool's WebSocket transport against a mock child server: a thread
//! that accepts one WebSocket connection and serves a scripted protocol
//! session. This exercises `Worker::websocket` (the `dial_ws` dial) and the WS
//! send/recv path end-to-end without needing a real remote child.

use std::{
    fs,
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use monty::{AssertMessageAnnotations, MontyObject, PrintStream, ResourceLimits};
use monty_pool::{MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, ReplConfig, TurnEvent};
use monty_proto::{decode_frame, encode_to_capped_vec, pb};
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

#[test]
fn drives_a_session_over_websocket() {
    // Bind before spawning so the port is listening before the pool connects.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || serve_mock_child(&listener));

    let mut config = PoolConfig::websocket(format!("ws://127.0.0.1:{port}"));
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).expect("pool");

    let mut checkout = pool
        .checkout(&ReplConfig {
            script_name: "test.py".to_owned(),
            limits: None,
            type_check: false,
            type_check_stubs: None,
            assert_message_annotations: AssertMessageAnnotations::default(),
        })
        .expect("checkout");

    // The WebSocket worker has no local pid.
    assert_eq!(checkout.pid(), None);

    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut |_, _| {})
        .expect("feed");
    assert!(
        matches!(event, TurnEvent::Complete(MontyObject::Int(42))),
        "got {event:?}"
    );

    checkout.finish().expect("finish");
    server.join().expect("mock child thread");
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

fn no_print(_: PrintStream, _: &str) {}

/// The headline scenario for parent-side mounts: the worker lives on the far
/// side of a WebSocket, yet a mounted read is serviced from the *parent's*
/// filesystem — the mock child emits the `OsCall` and receives the file's
/// contents back in a `ResumeCall` without the caller ever seeing the call.
#[test]
fn mounted_reads_are_serviced_from_the_parent_filesystem() {
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
        // the parent must answer with the mounted file's contents, not
        // surface the call to its caller
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

    let pool = Pool::new(config).expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).expect("checkout");
    let event = checkout
        .feed(
            "unused",
            vec![],
            vec![MountSpec {
                virtual_path: "/mnt".to_owned(),
                host_path: dir.path().to_path_buf(),
                mode: MountSpecMode::ReadOnly,
                write_bytes_limit: None,
                memory_usage_limit: monty_fs::DEFAULT_MEMORY_USAGE_LIMIT,
            }],
            false,
            &mut no_print,
        )
        .expect("feed");
    assert!(
        matches!(&event, TurnEvent::Complete(MontyObject::String(s)) if s == "done"),
        "got {event:?}"
    );
    checkout.finish().expect("finish");
    server.join().expect("mock child thread");
}

/// A malformed `OsCall` payload from a (possibly compromised) child is a
/// protocol violation: the child validates and serializes these calls itself,
/// so a payload it could never legitimately produce (here an invalid open
/// mode) is never serviced. No parent-side I/O happens and the worker is
/// discarded.
#[test]
fn malformed_os_call_is_a_protocol_error() {
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

    let pool = Pool::new(config).expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).expect("checkout");
    let err = checkout
        .feed(
            "unused",
            vec![],
            vec![MountSpec {
                virtual_path: "/mnt".to_owned(),
                host_path: dir.path().to_path_buf(),
                mode: MountSpecMode::ReadOnly,
                write_bytes_limit: None,
                memory_usage_limit: monty_fs::DEFAULT_MEMORY_USAGE_LIMIT,
            }],
            false,
            &mut no_print,
        )
        .expect_err("malformed fs call must fail the feed");
    let PoolError::Protocol(msg) = err else {
        panic!("expected Protocol error, got {err:?}");
    };
    assert_eq!(msg, "invalid OS call payload: invalid file mode \"q\"");
    server.join().expect("mock child thread");
}

/// The parent-side `max_duration` backstop (remaining budget + grace) kills a
/// worker that never answers a feed — the case where the child's own time
/// enforcement has failed. No `request_timeout` is configured, so the
/// backstop is the only armed deadline.
#[test]
fn duration_backstop_kills_an_unresponsive_worker() {
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
        // never reply; wait for the watchdog to kill the connection
        let _ = socket.read();
    });

    let pool = Pool::new(config).expect("pool");
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::new().max_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .expect("checkout");
    let err = checkout
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    // the armed deadline was the remaining budget (≤100ms) plus the grace
    assert!(timeout <= Duration::from_millis(400), "deadline was {timeout:?}");
    server.join().expect("mock child thread");
}

/// A worker cannot extend a turn indefinitely by issuing covered mount calls:
/// each exchange deducts the elapsed worker interval from the turn's
/// allowance, so cumulative worker execution stays bounded by
/// `request_timeout` no matter how many covered calls the worker makes.
#[test]
fn mount_calls_do_not_reset_the_request_deadline() {
    let dir = tempfile::tempdir().unwrap();

    let (listener, mut config) = ws_pool_config();
    config.request_timeout = Some(Duration::from_millis(500));
    let server = thread::spawn(move || {
        let mut socket = accept_ws(&listener);
        assert!(matches!(
            read_request(&mut socket),
            pb::parent_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::child_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::parent_request::Kind::Feed(_)));
        // "run" for 200ms then issue a covered call, repeatedly: with the old
        // per-exchange reset this loop would finish all 20 rounds (each well
        // under the 500ms timeout); the allowance must kill the worker once
        // cumulative execution passes the timeout.
        let mut rounds = 0_u32;
        for call_id in 0..20 {
            thread::sleep(Duration::from_millis(200));
            let call = event_kind(pb::child_event::Kind::OsCall(pb::OsCall {
                call_id,
                call: Some(pb::os_call::Call::Exists("/mnt".to_owned())),
            }));
            let body = encode_to_capped_vec(&call).expect("encode event");
            if socket.send(Message::Binary(body.into())).is_err() {
                break; // killed by the watchdog
            }
            match socket.read() {
                Ok(Message::Binary(_)) => rounds += 1,
                _ => break, // killed by the watchdog
            }
        }
        assert!(rounds >= 1, "at least one covered exchange should be serviced");
        assert!(
            rounds < 20,
            "the watchdog never fired — the deadline was reset per exchange"
        );
    });

    let pool = Pool::new(config).expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).expect("checkout");
    let err = checkout
        .feed(
            "unused",
            vec![],
            vec![MountSpec::new(
                "/mnt".to_owned(),
                dir.path().to_path_buf(),
                MountSpecMode::ReadOnly,
            )],
            false,
            &mut no_print,
        )
        .expect_err("cumulative worker time must exhaust the turn allowance");
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    // the reported timeout is the turn deadline, not a per-exchange residual
    assert_eq!(timeout, Duration::from_millis(500));
    server.join().expect("mock child thread");
}

/// A restored session re-adopts its `max_duration` budget from the timing
/// fields the worker stamps on the `Load` reply, re-arming the parent-side
/// backstop without the parent ever seeing the original `ReplConfig`.
#[test]
fn restored_session_rearms_the_duration_backstop() {
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

    let pool = Pool::new(config).expect("pool");
    let mut checkout = pool.checkout(&ReplConfig::default()).expect("checkout");
    let (event, script_name) = checkout.restore(vec![1, 2, 3], vec![], &mut no_print).expect("restore");
    assert!(event.is_none());
    assert_eq!(script_name.as_deref(), Some("restored.py"));
    let err = checkout
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    assert!(timeout <= Duration::from_millis(400), "deadline was {timeout:?}");
    server.join().expect("mock child thread");
}
