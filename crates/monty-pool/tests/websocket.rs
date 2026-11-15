//! Drives the pool's WebSocket remote transport (`ws://` URLs) against a mock
//! server: a thread that accepts one connection and serves a scripted session.
//! Covers dialing, refusals, how a server's Close frame reaches the caller,
//! liveness, and teardown; the session logic both transports share is covered
//! over gRPC in `remote.rs`.

#[path = "support/opening.rs"]
mod opening;
#[path = "support/ws_mock.rs"]
mod ws_mock;

use std::{
    future::{pending, ready},
    thread,
    time::{Duration, Instant},
};

use bytes::Bytes;
use monty_pool::{
    Checkout, CheckoutOptions, Pool, PoolConfig, PoolError, PrintFuture, RemoteCause, ReplConfig, TurnEvent,
};
use monty_proto::{Cause, decode_frame, encode_to_capped_vec, pb};
use monty_types::{MontyObject, PrintStream};
use tokio::{net::TcpListener, task::spawn_blocking};
use ws_mock::{MockServer, MockSocket};

#[tokio::test]
async fn drives_a_session_over_websocket() {
    let mock = MockServer::start().await;
    let url = format!("{}/relay/session?tenant=a", mock.url());
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        // dialed verbatim: a relay may route on the path and query
        assert_eq!(socket.path, "/relay/session?tenant=a");
        answer_requests(&mut socket);
    });
    let (pool, mut checkout) = remote_checkout(&url).await;
    for _ in 0..2 {
        let event = checkout
            .feed("6 * 7", vec![], vec![], false, &mut no_print)
            .await
            .expect("feed");
        assert!(
            matches!(event, TurnEvent::Complete(ref v) if *v == MontyObject::int(42)),
            "got {event:?}"
        );
    }
    checkout.finish().await.expect("finish");
    join_server(server).await;
    pool.close().await;
}

/// Schemes are read without regard to case, and `ws://` never falls back to gRPC.
#[tokio::test]
async fn ws_schemes_are_case_insensitive() {
    for scheme in ["WS://", "Ws://"] {
        let mock = MockServer::start().await;
        let url = mock.url().replace("ws://", scheme);
        let server = thread::spawn(move || answer_requests(&mut mock.accept()));
        let (_pool, mut checkout) = remote_checkout(&url).await;
        checkout
            .feed("1", vec![], vec![], false, &mut no_print)
            .await
            .expect("feed");
        checkout.finish().await.expect("finish");
        join_server(server).await;
    }
}

#[tokio::test]
async fn metadata_and_user_agent_are_upgrade_headers() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        let header = |name: &str| socket.headers.get(name).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(header("authorization").as_deref(), Some("Bearer t0k3n"));
        assert!(
            header("user-agent").is_some_and(|ua| ua.starts_with("monty-pool/")),
            "user agent missing"
        );
        answer_requests(&mut socket);
    });
    let pool = remote_pool(&url).await;
    let options =
        CheckoutOptions::default().with_metadata(vec![("authorization".to_owned(), "Bearer t0k3n".to_owned())]);
    let mut checkout = pool
        .checkout_with(&ReplConfig::default(), options)
        .await
        .expect("checkout");
    checkout
        .feed("1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn a_refused_upgrade_is_a_structured_refusal() {
    let mock = MockServer::start().await;
    mock.refuse_next(503, &[("monty-cause", "capacity"), ("retry-after", "3")], "at capacity");
    let pool = remote_pool(&mock.url()).await;
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("the refusal must fail the checkout");
    };
    let PoolError::Refused { refusal, .. } = &err else {
        panic!("got {err:?}");
    };
    assert_eq!(refusal.status_code, Some("RESOURCE_EXHAUSTED"));
    assert_eq!(refusal.cause, Some(RemoteCause::Capacity));
    assert_eq!(refusal.retry_after, Some(Duration::from_secs(3)));
    assert!(refusal.retryable());
    insta::assert_snapshot!(err.to_string().replace(&mock.url(), "URL"), @"URL refused the session: RESOURCE_EXHAUSTED (capacity): HTTP 503 Service Unavailable: at capacity");
}

#[tokio::test]
async fn an_unauthenticated_upgrade_is_refused_without_a_cause() {
    let mock = MockServer::start().await;
    mock.refuse_next(401, &[], "");
    let pool = remote_pool(&mock.url()).await;
    let Err(PoolError::Refused { refusal, .. }) = pool.checkout(&ReplConfig::default()).await else {
        panic!("expected a refusal");
    };
    assert_eq!(refusal.status_code, Some("UNAUTHENTICATED"));
    assert_eq!(refusal.cause, None);
    assert!(!refusal.retryable());
}

/// A 404 says the URL is no Monty server, not that one refused the session.
#[tokio::test]
async fn a_not_found_upgrade_is_a_connect_failure() {
    let mock = MockServer::start().await;
    mock.refuse_next(404, &[], "no such route");
    let pool = remote_pool(&mock.url()).await;
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("a 404 must fail the checkout");
    };
    assert!(matches!(err, PoolError::Connect { .. }), "got {err:?}");
    insta::assert_snapshot!(err.to_string().replace(&mock.url(), "URL"), @"URL: the server refused the WebSocket upgrade: HTTP 404 Not Found: no such route");
}

#[tokio::test]
async fn a_policy_close_mid_turn_reports_its_cause() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "while True: pass");
        socket.close(Cause::TurnTimeout.close_code(), "turn exceeded 30s");
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("while True: pass", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("a closed session must fail the turn");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = &err
    else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, Some("DEADLINE_EXCEEDED"));
    assert_eq!(details.cause, Some(RemoteCause::TurnTimeout));
    assert_eq!(details.status_message.as_deref(), Some("turn exceeded 30s"));
    assert_eq!(details.transport_error, None);
    insta::assert_snapshot!(err.to_string(), @"monty worker connection closed while waiting for a reply: DEADLINE_EXCEEDED (turn_timeout): turn exceeded 30s");
    join_server(server).await;
}

#[tokio::test]
async fn an_out_of_memory_close_is_a_memory_error() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "x = 'a' * 10**9");
        socket.close(Cause::OutOfMemory.close_code(), "");
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

#[tokio::test]
async fn a_server_shutdown_close_is_a_shutdown_without_state() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        socket.close(Cause::ServerShutdown.close_code(), "server is draining");
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let err = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("shutdown");
    assert!(matches!(err, PoolError::Shutdown { dump: None }), "got {err:?}");
    join_server(server).await;
}

#[tokio::test]
async fn a_close_while_idle_is_reported_by_the_next_request() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let socket = mock.accept();
        socket.close(Cause::IdleTimeout.close_code(), "idle");
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    join_server(server).await;
    let details = expect_disconnect(&mut checkout).await;
    assert_eq!(details.status_code, Some("DEADLINE_EXCEEDED"));
    assert_eq!(details.cause, Some(RemoteCause::IdleTimeout));
}

/// The server closes as soon as the first request lands, so the next send
/// races its Close frame; the reason it gave must still be what is reported,
/// never a local "connection closed" from the failed send.
#[tokio::test]
async fn a_close_racing_a_send_still_reports_the_close_frame() {
    for _ in 0..20 {
        let mock = MockServer::start().await;
        let url = mock.url();
        let server = thread::spawn(move || {
            let mut socket = mock.accept();
            expect_configure(&mut socket);
            send_complete(&mut socket);
            let _ = socket.read();
            socket.close(Cause::SessionTimeout.close_code(), "session lifetime");
        });
        let (_pool, mut checkout) = remote_checkout(&url).await;
        checkout
            .feed("1", vec![], vec![], false, &mut no_print)
            .await
            .expect("first feed");
        let mut details = None;
        // keep sending until the close lands, however the race falls
        for _ in 0..100 {
            match checkout.feed("1", vec![], vec![], false, &mut no_print).await {
                Err(PoolError::Disconnected { remote, .. }) => {
                    details = remote;
                    break;
                }
                Err(err) => panic!("got {err:?}"),
                Ok(_) => {}
            }
        }
        let details = details.expect("the close was reported");
        assert_eq!(details.cause, Some(RemoteCause::SessionTimeout), "got {details:?}");
        join_server(server).await;
    }
}

#[tokio::test]
async fn a_final_event_before_the_close_is_delivered() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1 + 1");
        send_complete(&mut socket);
        socket.close(1000, "");
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("the reply before the close");
    assert!(matches!(event, TurnEvent::Complete(_)), "got {event:?}");
    join_server(server).await;
    let details = expect_disconnect(&mut checkout).await;
    assert_eq!(details.status_code, Some("OK"));
}

#[tokio::test]
async fn an_unknown_close_code_has_no_cause() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1");
        socket.close(4999, "");
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let details = expect_disconnect(&mut checkout).await;
    assert_eq!(details.status_code, Some("UNKNOWN"));
    assert_eq!(details.cause, None);
    assert_eq!(details.status_message.as_deref(), Some("close code 4999"));
    join_server(server).await;
}

#[tokio::test]
async fn a_dropped_connection_is_a_transport_failure() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1");
        socket.cut();
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let details = expect_disconnect(&mut checkout).await;
    assert_eq!(details.status_code, None);
    assert!(details.transport_error.is_some(), "got {details:?}");
    join_server(server).await;
}

#[tokio::test]
async fn a_text_message_is_a_protocol_failure() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1");
        socket.send_text("hello");
        while socket.read().is_some() {}
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let details = expect_disconnect(&mut checkout).await;
    assert_eq!(
        details.transport_error.as_deref(),
        Some("unexpected non-binary WebSocket message")
    );
    join_server(server).await;
}

/// Both liveness signals are answered while the checkout is idle: an in-band
/// `Ping` with a `Pong` request, a WebSocket ping frame with a pong frame.
#[tokio::test]
async fn pings_are_answered_while_idle() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        send_kind(&mut socket, pb::worker_event::Kind::Ping(pb::Ping {}));
        socket.set_read_timeout(Duration::from_secs(5));
        let request = read_request(&mut socket);
        assert!(matches!(request, pb::client_request::Kind::Pong(_)), "got {request:?}");
        socket.ping();
        let deadline = Instant::now() + Duration::from_secs(5);
        while socket.pongs() == 0 {
            assert!(Instant::now() < deadline, "no pong frame");
            thread::sleep(Duration::from_millis(5));
        }
    });
    let (_pool, checkout) = remote_checkout(&url).await;
    join_server(server).await;
    drop(checkout);
}

/// The connection's message limit follows the decode budget, so an oversize
/// reply is refused by size before it is buffered or decoded.
#[tokio::test]
async fn a_message_above_the_decode_budget_is_refused() {
    let complete = pb::WorkerEvent {
        kind: Some(pb::worker_event::Kind::Complete(pb::Complete::from(
            MontyObject::string("x".repeat(200_000)),
        ))),
        ..Default::default()
    };
    let body = encode_to_capped_vec(&complete).expect("encode");
    let len = body.len();
    let mock = MockServer::start().await;
    let mut config = PoolConfig::remote(mock.url());
    config.max_decode_bytes = 100_000;
    config.request_timeout = Some(Duration::from_secs(10));
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1");
        let _ = socket.send(Bytes::from(body));
        while socket.read().is_some() {}
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

#[tokio::test]
async fn finishing_a_checkout_sends_a_normal_close() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        expect_feed(&mut socket, "1");
        send_complete(&mut socket);
        assert!(socket.read().is_none(), "expected the session to end");
        assert_eq!(socket.client_close_code(), 1000);
    });
    let (pool, mut checkout) = remote_checkout(&url).await;
    checkout
        .feed("1", vec![], vec![], false, &mut no_print)
        .await
        .expect("feed");
    checkout.finish().await.expect("finish");
    join_server(server).await;
    pool.close().await;
}

#[tokio::test]
async fn an_abandoned_checkout_sends_a_normal_close() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(socket.read().is_none(), "expected the session to end");
        assert_eq!(socket.client_close_code(), 1000);
    });
    let (_pool, checkout) = remote_checkout(&url).await;
    drop(checkout);
    join_server(server).await;
}

#[tokio::test]
async fn shutdown_resumes_over_a_new_connection() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        read_request(&mut socket);
        send_event(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}), Some(b"sess-1"));
        expect_feed(&mut socket, "1 + 1");
        send_kind(
            &mut socket,
            pb::worker_event::Kind::Shutdown(pb::ShutdownDump {
                dump: Some(b"parked-1".to_vec().into()),
            }),
        );
        let mut socket = mock.accept();
        expect_configure(&mut socket);
        match read_request(&mut socket) {
            pb::client_request::Kind::Load(load) => assert_eq!(load.state.as_ref(), b"parked-1"),
            other => panic!("expected Load, got {other:?}"),
        }
        send_event(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}), Some(b"sess-2"));
        expect_feed(&mut socket, "1 + 1");
        send_complete(&mut socket);
        while socket.read().is_some() {}
    });
    let (_pool, mut checkout) = remote_checkout(&url).await;
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .expect("the drain is invisible to the caller");
    assert!(matches!(event, TurnEvent::Complete(_)), "got {event:?}");
    assert_eq!(checkout.session_id(), Some(&b"sess-2"[..]));
    checkout.finish().await.expect("finish");
    join_server(server).await;
}

#[tokio::test]
async fn request_timeout_bounds_the_upgrade() {
    // a listener that accepts TCP but never answers the upgrade
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("ws://{}", listener.local_addr().expect("addr"));
    let hold = tokio::spawn(async move {
        let _conn = listener.accept().await;
        pending::<()>().await;
    });
    let mut config = PoolConfig::remote(&url);
    config.request_timeout = Some(Duration::from_millis(200));
    let pool = Pool::new(config).await.expect("pool");
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("a stalled upgrade must fail the checkout");
    };
    assert!(matches!(err, PoolError::Connect { .. }), "got {err:?}");
    insta::assert_snapshot!(err.to_string().replace(&url, "URL"), @"URL: the server did not accept the session within 200ms (request_timeout)");
    hold.abort();
}

/// `wss://` always negotiates TLS: against a plaintext server the handshake
/// fails rather than falling back to an unencrypted upgrade.
#[tokio::test]
async fn wss_never_falls_back_to_plaintext() {
    let mock = MockServer::start().await;
    let url = mock.url().replace("ws://", "wss://");
    let pool = remote_pool(&url).await;
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("a TLS dial to a plaintext server must fail");
    };
    let PoolError::Connect { details, .. } = &err else {
        panic!("got {err:?}");
    };
    assert!(
        mock.try_accept(Duration::from_millis(100)).is_none(),
        "no upgrade reached the server"
    );
    assert!(details.transport_error.is_some(), "got {details:?}");
}

// === helpers ===

/// Answers each request with the obvious turn-ender (`Ok` for control
/// requests, `Complete(42)` for a feed) until the pool ends the session.
fn answer_requests(socket: &mut MockSocket) {
    while let Some(data) = socket.read() {
        let request = decode_frame::<pb::ClientRequest>(data.as_ref()).expect("decode request");
        let kind = match request.kind.expect("request kind") {
            pb::client_request::Kind::Feed(_) => {
                pb::worker_event::Kind::Complete(pb::Complete::from(MontyObject::int(42)))
            }
            _ => pb::worker_event::Kind::Ok(pb::Ok {}),
        };
        send_kind(socket, kind);
    }
}

fn read_request(socket: &mut MockSocket) -> pb::client_request::Kind {
    let data = socket.read().expect("expected a request message");
    decode_frame::<pb::ClientRequest>(data.as_ref())
        .expect("decode request")
        .kind
        .expect("request kind")
}

fn expect_configure(socket: &mut MockSocket) {
    let request = read_request(socket);
    assert!(
        matches!(request, pb::client_request::Kind::Configure(_)),
        "expected Configure, got {request:?}"
    );
    send_kind(socket, pb::worker_event::Kind::Ok(pb::Ok {}));
}

fn expect_feed(socket: &mut MockSocket, code: &str) {
    match read_request(socket) {
        pb::client_request::Kind::Feed(feed) => assert_eq!(feed.code, code),
        other => panic!("expected Feed, got {other:?}"),
    }
}

fn send_kind(socket: &mut MockSocket, kind: pb::worker_event::Kind) {
    send_event(socket, kind, None);
}

/// Sends one event, naming `session_id` when given (a server that stores sessions).
fn send_event(socket: &mut MockSocket, kind: pb::worker_event::Kind, session_id: Option<&[u8]>) {
    let event = pb::WorkerEvent {
        kind: Some(kind),
        session_id: session_id.map(|id| id.to_vec().into()),
        ..Default::default()
    };
    let body = encode_to_capped_vec(&event).expect("encode event");
    let _ = socket.send(body.into());
}

fn send_complete(socket: &mut MockSocket) {
    send_kind(
        socket,
        pb::worker_event::Kind::Complete(pb::Complete::from(MontyObject::int(42))),
    );
}

/// Feeds once and returns the disconnect evidence the failure carries.
async fn expect_disconnect(checkout: &mut Checkout) -> Box<monty_pool::RemoteDisconnect> {
    let err = checkout
        .feed("1", vec![], vec![], false, &mut no_print)
        .await
        .expect_err("the session must have ended");
    let PoolError::Disconnected {
        remote: Some(details), ..
    } = err
    else {
        panic!("got {err:?}");
    };
    details
}

async fn remote_checkout(url: &str) -> (Pool, Checkout) {
    let pool = remote_pool(url).await;
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    (pool, checkout)
}

async fn remote_pool(url: &str) -> Pool {
    let mut config = PoolConfig::remote(url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    Pool::new(config).await.expect("pool")
}

fn no_print(_: PrintStream, _: &str) -> PrintFuture {
    Box::pin(ready(()))
}

/// Joins a mock-server thread without blocking the runtime thread.
async fn join_server(server: thread::JoinHandle<()>) {
    spawn_blocking(move || server.join().expect("mock server thread"))
        .await
        .expect("join");
}
