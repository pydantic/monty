//! Per-turn trace context over the remote transport: the caller's span rides
//! on each turn-starting request, and the span a server names on a suspension
//! reaches the host with it.

#[expect(dead_code, reason = "the mock's TLS, refusal and timeout controls serve `remote.rs`")]
#[path = "support/grpc_mock.rs"]
mod grpc_mock;

#[cfg(feature = "telemetry")]
use std::sync::Arc;
use std::{
    future::ready,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use grpc_mock::{MockServer, MockStream};
use monty_pool::{
    Checkout, Pool, PoolConfig, PrintFuture, ReplConfig, ResumeValue, Traceparent, TurnEvent, TurnOptions,
};
#[cfg(feature = "telemetry")]
use monty_pool::{
    CheckoutOptions,
    telemetry::{TelemetryAdapter, configure_telemetry_adapter, remote_suspension},
};
use monty_proto::{WireFunctionCall, decode_frame, encode_to_capped_vec, pb};
use monty_types::{CallArgs, MontyObject, PrintStream, SourceRange};
#[cfg(feature = "telemetry")]
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceId};
#[cfg(feature = "telemetry")]
use opentelemetry_sdk::{logs::SdkLogRecord, trace::SpanData};
use tokio::task::spawn_blocking;

const FIRST_CALLER: &str = "00-000000000000000000000000000000a1-00000000000000b1-01";
const SECOND_CALLER: &str = "00-000000000000000000000000000000a2-00000000000000b2-01";
const SERVER_CALL: &str = "00-000000000000000000000000000000a1-00000000000000c1-01";
const SERVER_STREAMED: &str = "00-000000000000000000000000000000a1-00000000000000c2-00";

/// Each feed on one checkout names its own caller's span, the first inside the
/// `Configure` that carries it, and a feed without a caller span names none.
#[tokio::test]
async fn each_feed_carries_its_callers_span() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        let opening = socket.read_unsplit().expect("the opening request");
        let opening = decode_frame::<pb::ClientRequest>(opening.as_ref()).expect("decode request");
        assert!(
            matches!(
                &opening.kind,
                Some(pb::client_request::Kind::Configure(configure))
                    if matches!(configure.first, Some(pb::configure::First::Feed(_)))
            ),
            "got {opening:?}"
        );
        send_complete(&mut socket);
        let mut seen = vec![opening.traceparent];
        for _ in 0..2 {
            seen.push(read_request(&mut socket).traceparent);
            send_complete(&mut socket);
        }
        drain(&mut socket);
        seen
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    for caller in [Some(FIRST_CALLER), Some(SECOND_CALLER), None] {
        feed(&mut checkout, caller).await;
    }
    checkout.finish().await.expect("finish");
    assert_eq!(
        join(server).await,
        [Some(FIRST_CALLER.to_owned()), Some(SECOND_CALLER.to_owned()), None]
    );
}

/// A suspension's `traceparent` reaches the host on the event and as the
/// callback context; an event naming no span, or a malformed one, leaves both
/// as they are without it.
#[tokio::test]
async fn a_suspension_carries_the_servers_span() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        read_request(&mut socket);
        send_kind(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}));
        read_request(&mut socket);
        for (call_id, traceparent) in [(1, Some(SERVER_CALL)), (2, None), (3, Some("00-not-a-span-01"))] {
            let event = pb::WorkerEvent {
                kind: Some(pb::worker_event::Kind::FunctionCall(call(call_id))),
                traceparent: traceparent.map(str::to_owned),
                ..Default::default()
            };
            socket
                .send(encode_to_capped_vec(&event).expect("encode event").into())
                .expect("send event");
            read_request(&mut socket);
        }
        send_complete(&mut socket);
        drain(&mut socket);
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let mut event = feed(&mut checkout, Some(FIRST_CALLER)).await;
    for expected in [Traceparent::parse(SERVER_CALL), None, None] {
        let TurnEvent::FunctionCall { traceparent, .. } = &event else {
            panic!("expected a function call, got {event:?}");
        };
        assert_eq!(*traceparent, expected);
        #[cfg(feature = "telemetry")]
        assert_eq!(callback_span(&checkout.callback_context()), expected.map(remote_span));
        event = checkout
            .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
            .await
            .expect("resume");
    }
    assert!(matches!(event, TurnEvent::Complete(_)), "got {event:?}");
    // answered: the span is not the context for whatever the session does next
    #[cfg(feature = "telemetry")]
    assert_eq!(callback_span(&checkout.callback_context()), None);
    checkout.finish().await.expect("finish");
    join(server).await;
}

/// A checkout recording through a host SDK adapter keeps the server's span on
/// the event, and runs the callback under its own recorded span.
#[cfg(feature = "telemetry")]
#[tokio::test]
async fn an_adapter_backed_checkout_keeps_the_servers_span_on_the_event() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        read_request(&mut socket);
        send_kind(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}));
        read_request(&mut socket);
        let event = pb::WorkerEvent {
            kind: Some(pb::worker_event::Kind::FunctionCall(call(1))),
            traceparent: Some(SERVER_CALL.to_owned()),
            ..Default::default()
        };
        socket
            .send(encode_to_capped_vec(&event).expect("encode event").into())
            .expect("send event");
        read_request(&mut socket);
        send_complete(&mut socket);
        drain(&mut socket);
    });

    let handle = configure_telemetry_adapter(Arc::new(NamesItsSpans)).expect("configure");
    let mut config = PoolConfig::remote(&url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");
    let context = handle
        .context("0af7651916cd43dd8448eb211c80319c", "b7ad6b7169203331", 1, "")
        .expect("context");
    let options = CheckoutOptions::default().with_telemetry(Some(context));
    let mut checkout = pool
        .checkout_with(&ReplConfig::default(), options)
        .await
        .expect("checkout");

    let event = feed(&mut checkout, Some(FIRST_CALLER)).await;
    let TurnEvent::FunctionCall { traceparent, .. } = &event else {
        panic!("expected a function call, got {event:?}");
    };
    assert_eq!(*traceparent, Traceparent::parse(SERVER_CALL));
    let (trace_id, span_id, remote) = callback_span(&checkout.callback_context()).expect("the recorded call span");
    assert_eq!(trace_id, remote_span(Traceparent::parse(SERVER_CALL).unwrap()).0);
    assert_ne!(span_id, remote_span(Traceparent::parse(SERVER_CALL).unwrap()).1);
    assert!(!remote);
    // `remote_suspension` is what a host without a tracer resolves: still the server's span
    assert_eq!(
        remote_suspension(&checkout.callback_context()),
        Traceparent::parse(SERVER_CALL)
    );
    let event = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .expect("resume");
    assert!(matches!(event, TurnEvent::Complete(_)), "got {event:?}");
    checkout.finish().await.expect("finish");
    join(server).await;
}

/// Dumping a suspended session leaves the suspension, and the span the server
/// named for it, in place for the callback that answers it.
#[cfg(feature = "telemetry")]
#[tokio::test]
async fn a_dump_keeps_the_suspensions_span() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        read_request(&mut socket);
        send_kind(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}));
        read_request(&mut socket);
        let event = pb::WorkerEvent {
            kind: Some(pb::worker_event::Kind::FunctionCall(call(1))),
            traceparent: Some(SERVER_CALL.to_owned()),
            ..Default::default()
        };
        socket
            .send(encode_to_capped_vec(&event).expect("encode event").into())
            .expect("send event");
        let dump = read_request(&mut socket);
        assert!(
            matches!(dump.kind, Some(pb::client_request::Kind::Dump(_))),
            "got {dump:?}"
        );
        send_kind(
            &mut socket,
            pb::worker_event::Kind::DumpResult(pb::DumpResult {
                state: b"state".to_vec().into(),
            }),
        );
        read_request(&mut socket);
        send_complete(&mut socket);
        drain(&mut socket);
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    feed(&mut checkout, None).await;
    assert_eq!(checkout.dump().await.expect("dump"), b"state");
    assert_eq!(
        callback_span(&checkout.callback_context()),
        Traceparent::parse(SERVER_CALL).map(remote_span)
    );
    checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .expect("resume");
    checkout.finish().await.expect("finish");
    join(server).await;
}

/// An answer the server rejects without ending the feed leaves the suspension,
/// and the span the server named for it, in place for the retried answer.
#[cfg(feature = "telemetry")]
#[tokio::test]
async fn a_rejected_answer_keeps_the_suspensions_span() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        read_request(&mut socket);
        send_kind(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}));
        read_request(&mut socket);
        let event = pb::WorkerEvent {
            kind: Some(pb::worker_event::Kind::FunctionCall(call(1))),
            traceparent: Some(SERVER_CALL.to_owned()),
            ..Default::default()
        };
        socket
            .send(encode_to_capped_vec(&event).expect("encode event").into())
            .expect("send event");
        read_request(&mut socket);
        let rejected = pb::WorkerEvent {
            kind: Some(pb::worker_event::Kind::Error(pb::Error {
                exception: Some(pb::RaisedException {
                    exc_type: "TypeError".to_owned(),
                    message: Some("bad answer".to_owned()),
                    traceback: vec![].into(),
                    data: None,
                }),
            })),
            feed_suspended: Some(true),
            ..Default::default()
        };
        socket
            .send(encode_to_capped_vec(&rejected).expect("encode event").into())
            .expect("send event");
        read_request(&mut socket);
        send_complete(&mut socket);
        drain(&mut socket);
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    feed(&mut checkout, None).await;
    let rejected = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await;
    assert!(rejected.is_err(), "got {rejected:?}");
    assert_eq!(
        callback_span(&checkout.callback_context()),
        Traceparent::parse(SERVER_CALL).map(remote_span)
    );
    let event = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .expect("resume");
    assert!(matches!(event, TurnEvent::Complete(_)), "got {event:?}");
    checkout.finish().await.expect("finish");
    join(server).await;
}

/// Each streamed call carries the span the server named for it, as the value
/// and as the context its handler runs under.
#[tokio::test]
async fn streamed_calls_carry_their_own_spans() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        read_request(&mut socket);
        send_kind(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}));
        read_request(&mut socket);
        let calls = vec![
            WireFunctionCall {
                traceparent: Traceparent::parse(SERVER_CALL),
                ..call(1)
            },
            call(2),
            WireFunctionCall {
                traceparent: Traceparent::parse(SERVER_STREAMED),
                ..call(3)
            },
        ];
        send_kind(
            &mut socket,
            pb::worker_event::Kind::StreamedCalls(pb::StreamedCalls { calls: calls.into() }),
        );
        send_complete(&mut socket);
        drain(&mut socket);
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    let (started_tx, started_rx) = mpsc::channel();
    checkout.set_async_functions(Some(vec!["fetch".to_owned()]));
    checkout.set_streamed_call_handler(Some(Box::new(move |call| {
        #[cfg(feature = "telemetry")]
        assert_eq!(callback_span(&call.callback_context), call.traceparent.map(remote_span));
        started_tx.send((call.call_id, call.traceparent)).expect("started");
        Box::pin(ready(()))
    })));
    feed(&mut checkout, None).await;
    assert_eq!(
        started_rx.try_iter().collect::<Vec<_>>(),
        [
            (1, Traceparent::parse(SERVER_CALL)),
            (2, None),
            (3, Traceparent::parse(SERVER_STREAMED))
        ]
    );
    checkout.finish().await.expect("finish");
    join(server).await;
}

/// A feed re-sent after its relay shut down names the same caller span.
#[tokio::test]
async fn an_auto_resumed_feed_resends_its_callers_span() {
    let mock = MockServer::start().await;
    let url = mock.url();
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        read_request(&mut socket);
        send_with_session_id(&mut socket, b"sess-1");
        read_request(&mut socket);
        send_kind(
            &mut socket,
            pb::worker_event::Kind::Shutdown(pb::ShutdownDump {
                dump: Some(b"parked-1".to_vec().into()),
            }),
        );
        let mut socket = mock.accept();
        // the reload's `Configure` and `Load`, which no caller started
        let reload = [read_request(&mut socket), read_request(&mut socket)];
        send_kind(&mut socket, pb::worker_event::Kind::Ok(pb::Ok {}));
        send_with_session_id(&mut socket, b"sess-2");
        let resent = read_request(&mut socket);
        assert!(
            matches!(resent.kind, Some(pb::client_request::Kind::Feed(_))),
            "got {resent:?}"
        );
        send_complete(&mut socket);
        drain(&mut socket);
        (reload.map(|request| request.traceparent), resent.traceparent)
    });

    let (_pool, mut checkout) = remote_checkout(&url).await;
    feed(&mut checkout, Some(FIRST_CALLER)).await;
    checkout.finish().await.expect("finish");
    let (reload, resent) = join(server).await;
    assert_eq!(reload, [None, None]);
    assert_eq!(resent.as_deref(), Some(FIRST_CALLER));
}

/// Feeds `fetch()` under `caller`'s span.
async fn feed(checkout: &mut Checkout, caller: Option<&str>) -> TurnEvent {
    let options = TurnOptions::default().with_traceparent(caller.and_then(Traceparent::parse));
    checkout
        .feed_with("fetch()", vec![], vec![], None, false, options, &mut no_print)
        .await
        .expect("feed")
}

/// The remote span a callback context carries, if any.
#[cfg(feature = "telemetry")]
fn callback_span(context: &opentelemetry::Context) -> Option<(TraceId, SpanId, bool)> {
    let span = context.span();
    let span = span.span_context();
    span.is_valid()
        .then(|| (span.trace_id(), span.span_id(), span.is_remote()))
}

/// An adapter whose SDK exports each span under the ID the pool recorded it with.
#[cfg(feature = "telemetry")]
struct NamesItsSpans;

#[cfg(feature = "telemetry")]
impl TelemetryAdapter for NamesItsSpans {
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
    fn exported_span_context(&self, span: &SpanContext) -> Option<SpanContext> {
        Some(span.clone())
    }
}

#[cfg(feature = "telemetry")]
fn remote_span(traceparent: Traceparent) -> (TraceId, SpanId, bool) {
    (traceparent.trace_id().into(), traceparent.span_id().into(), true)
}

fn call(call_id: u32) -> WireFunctionCall {
    let position = SourceRange {
        filename: "main.py".to_owned(),
        start: 0,
        end: 7,
    };
    WireFunctionCall::new("fetch".to_owned(), CallArgs::new(), call_id, None, false, position)
}

fn read_request(socket: &mut MockStream) -> pb::ClientRequest {
    let data = socket.read().expect("a request");
    decode_frame::<pb::ClientRequest>(data.as_ref()).expect("decode request")
}

fn send_kind(socket: &mut MockStream, kind: pb::worker_event::Kind) {
    let event = pb::WorkerEvent {
        kind: Some(kind),
        ..Default::default()
    };
    socket
        .send(encode_to_capped_vec(&event).expect("encode event").into())
        .expect("send event");
}

/// Acknowledges a request as a storing relay does, naming the session.
fn send_with_session_id(socket: &mut MockStream, session_id: &[u8]) {
    let event = pb::WorkerEvent {
        kind: Some(pb::worker_event::Kind::Ok(pb::Ok {})),
        session_id: Some(session_id.to_vec().into()),
        ..Default::default()
    };
    socket
        .send(encode_to_capped_vec(&event).expect("encode event").into())
        .expect("send event");
}

fn send_complete(socket: &mut MockStream) {
    send_kind(
        socket,
        pb::worker_event::Kind::Complete(pb::Complete::from(MontyObject::none())),
    );
}

/// Acknowledges whatever the checkout sends while finishing.
fn drain(socket: &mut MockStream) {
    while socket.read().is_some() {
        send_kind(socket, pb::worker_event::Kind::Ok(pb::Ok {}));
    }
}

fn no_print(_: PrintStream, _: &str) -> PrintFuture {
    Box::pin(ready(()))
}

async fn remote_checkout(url: &str) -> (Pool, Checkout) {
    let mut config = PoolConfig::remote(url);
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(10));
    let pool = Pool::new(config).await.expect("pool");
    let checkout = pool.checkout(&ReplConfig::default()).await.expect("checkout");
    (pool, checkout)
}

async fn join<T: Send + 'static>(server: JoinHandle<T>) -> T {
    spawn_blocking(move || server.join().expect("the mock server"))
        .await
        .expect("join")
}
