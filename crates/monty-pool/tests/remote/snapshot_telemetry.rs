//! Snapshot API spans follow the public call through setup, errors and cancellation.

use std::{
    future::{poll_fn, ready},
    pin::Pin,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime},
};

use logfire::{
    Logfire,
    config::{AdvancedOptions, LogfireConfigBuilder},
    set_local_logfire,
};
use monty_pool::{
    CheckoutOptions, Pool, PoolError, PrintFuture, ReplConfig, TurnEvent, TurnOptions, telemetry::TelemetryContext,
};
use monty_proto::{Traceparent, decode_frame, pb};
use monty_types::{MontyObject, NameLookupResult};
use opentelemetry::{
    Value,
    trace::{SpanContext, SpanId, Status, TraceContextExt, TraceFlags, TraceId, TraceState},
};
use opentelemetry_sdk::{
    logs::{InMemoryLogExporter, SimpleLogProcessor},
    trace::{InMemorySpanExporter, SimpleSpanProcessor, SpanData},
};
use tokio::{sync::oneshot, time::timeout};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use super::{
    configure_with_session_id, event_kind, expect_configure, expect_load, join_server, mock_pool_config, no_print,
    ok_event, position, read_request, send_event, send_kind, send_with_session_id, shutdown,
};

/// Dropping the API future preserves its trace and leaves the host span open.
#[tokio::test]
async fn cancelling_a_dump_preserves_the_ambient_parent() {
    let spans = InMemorySpanExporter::default();
    let logfire = local_logfire()
        .with_additional_span_processor(SimpleSpanProcessor::new(spans.clone()))
        .finish()
        .unwrap();
    let host = {
        let _provider = set_local_logfire(logfire.clone());
        tracing::info_span!("host request")
    };
    let host_context = host.context().span().span_context().clone();
    let (configured, mut configuration) = oneshot::channel();
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::client_request::Kind::Configure(_)
        ));
        configured.send(()).unwrap();
        assert!(socket.read().is_none());
    });
    let pool = Pool::new(config).await.unwrap();
    let options = CheckoutOptions::default().with_telemetry(Some(TelemetryContext::for_logfire(logfire.clone())));
    let mut checkout = pool.checkout_with(&ReplConfig::default(), options).await.unwrap();
    let mut dump = Box::pin(checkout.dump());
    timeout(
        Duration::from_secs(10),
        poll_fn(|cx| {
            let _provider = set_local_logfire(logfire.clone());
            host.in_scope(|| {
                assert!(dump.as_mut().poll(cx).is_pending());
                Pin::new(&mut configuration).poll(cx)
            })
        }),
    )
    .await
    .unwrap()
    .unwrap();

    let ambient = local_logfire().finish().unwrap();
    {
        let _ambient = set_local_logfire(ambient);
        drop(dump);
        drop(checkout);
    }
    let recorded = spans.get_finished_spans().unwrap();
    assert!(recorded.iter().all(|span| span.name != "host request"));
    let dump = recorded.iter().find(|span| span.name == "dump").unwrap();
    assert_eq!(attribute(dump, "monty.outcome"), Some("cancelled".into()));
    assert_eq!(dump.span_context.trace_id(), host_context.trace_id());
    assert_eq!(dump.parent_span_id, host_context.span_id());
    drop(host);
    assert_eq!(
        spans
            .get_finished_spans()
            .unwrap()
            .iter()
            .filter(|span| span.span_context.span_id() == host_context.span_id())
            .count(),
        1
    );
    pool.close().await;
    join_server(server).await;
}

/// Base configuration for in-memory telemetry tests.
fn local_logfire() -> LogfireConfigBuilder {
    logfire::configure()
        .local()
        .send_to_logfire(false)
        .with_console(None)
        .with_install_panic_handler(false)
}

/// A replacement worker's refusal belongs to the original dump call.
#[tokio::test]
async fn dump_retry_error_keeps_the_original_operation_span() {
    let spans = InMemorySpanExporter::default();
    let logs = InMemoryLogExporter::default();
    let logfire = local_logfire()
        .with_additional_span_processor(SimpleSpanProcessor::new(spans.clone()))
        .with_advanced_options(AdvancedOptions::default().with_log_processor(SimpleLogProcessor::new(logs.clone())))
        .finish()
        .unwrap();
    let (mock, mut config) = mock_pool_config().await;
    config.auto_resume = true;
    let server = thread::spawn(move || {
        let mut original = mock.accept();
        configure_with_session_id(&mut original, b"sess-1");
        assert!(matches!(read_request(&mut original), pb::client_request::Kind::Dump(_)));
        send_kind(&mut original, shutdown(Some(b"parked")));
        drop(original);

        let mut replacement = mock.accept();
        expect_configure(&mut replacement);
        expect_load(&mut replacement, b"parked");
        send_with_session_id(&mut replacement, ok_event(), b"sess-2");
        assert!(matches!(
            read_request(&mut replacement),
            pb::client_request::Kind::Dump(_)
        ));
        send_kind(
            &mut replacement,
            pb::worker_event::Kind::Error(pb::Error {
                exception: Some(pb::RaisedException {
                    exc_type: "RuntimeError".to_owned(),
                    message: Some("replacement refused the dump".to_owned()),
                    ..Default::default()
                }),
            }),
        );
        assert!(replacement.read().is_none());
    });
    let pool = Pool::new(config).await.unwrap();
    let options = CheckoutOptions::default().with_telemetry(Some(TelemetryContext::for_logfire(logfire.clone())));
    let mut checkout = pool.checkout_with(&ReplConfig::default(), options).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(10), checkout.dump()).await.unwrap(),
        Err(PoolError::Runtime(_))
    ));
    assert_eq!(checkout.session_id(), Some(&b"sess-2"[..]));
    checkout.finish().await.unwrap();
    pool.close().await;
    join_server(server).await;

    let recorded = dump_spans(&spans);
    let [dump] = recorded.as_slice() else {
        panic!("expected one dump operation");
    };
    assert_eq!(dump.status, Status::error("RuntimeError: replacement refused the dump"));
    let emitted = logs.get_emitted_logs().unwrap();
    let error = emitted
        .iter()
        .find(|entry| {
            entry
                .record
                .attributes_iter()
                .any(|(key, _)| key.as_str() == "exc_type")
        })
        .unwrap();
    assert_eq!(
        error.record.trace_context().unwrap().span_id,
        dump.span_context.span_id()
    );
}

/// Setup and persistence acknowledgements bound the API span; raw forwarding emits no dump span.
#[tokio::test]
async fn dump_spans_cover_the_api_and_close_on_success_error_and_cancellation() {
    timeout(Duration::from_secs(10), Box::pin(dump_span_scenario()))
        .await
        .expect("dump telemetry scenario completed");
}

/// Exercise one traced checkout through acknowledged, raw, refused and cancelled dumps.
async fn dump_span_scenario() {
    let spans = InMemorySpanExporter::default();
    let logs = InMemoryLogExporter::default();
    let logfire = local_logfire()
        .with_additional_span_processor(SimpleSpanProcessor::new(spans.clone()))
        .with_advanced_options(AdvancedOptions::default().with_log_processor(SimpleLogProcessor::new(logs.clone())))
        .finish()
        .expect("logfire");
    let parent = SpanContext::new(
        TraceId::from(17_u128),
        SpanId::from(18_u64),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let options = CheckoutOptions::default().with_telemetry(Some(
        TelemetryContext::for_logfire(logfire.clone()).with_parent(parent.clone()),
    ));
    let (configured, configuration) = oneshot::channel();
    let (configure_ack, configure_release) = mpsc::channel();
    let (dumping, dump_request) = oneshot::channel();
    let (persisted, persistence_ack) = mpsc::channel();
    let (cancelling, cancel_request) = oneshot::channel();
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::client_request::Kind::Configure(_)
        ));
        configured.send(SystemTime::now()).unwrap();
        configure_release.recv().unwrap();
        send_event(&mut socket, &event_kind(pb::worker_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Dump(_)));
        dumping.send(()).unwrap();
        persistence_ack.recv().unwrap();
        let dump = event_kind(pb::worker_event::Kind::DumpResult(pb::DumpResult {
            state: b"durable snapshot handle".to_vec().into(),
        }));
        send_event(&mut socket, &dump);
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Dump(_)));
        send_event(&mut socket, &dump);
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Dump(_)));
        send_event(
            &mut socket,
            &event_kind(pb::worker_event::Kind::Error(pb::Error {
                exception: Some(pb::RaisedException {
                    exc_type: "RuntimeError".to_owned(),
                    message: Some("snapshot store refused the dump".to_owned()),
                    traceback: vec![].into(),
                    data: None,
                }),
            })),
        );
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Dump(_)));
        cancelling.send(()).unwrap();
        assert!(socket.read().is_none());
    });
    let pool = Pool::new(config).await.unwrap();
    let mut checkout = pool.checkout_with(&ReplConfig::default(), options).await.unwrap();
    let configured_at = {
        let dump = checkout.dump();
        tokio::pin!(dump);
        let configured_at = tokio::select! {
            result = &mut dump => panic!("dump completed before configuration: {result:?}"),
            configured = configuration => configured.unwrap(),
        };
        assert_eq!(dump_spans(&spans).len(), 0);
        configure_ack.send(()).unwrap();
        tokio::select! {
            result = &mut dump => panic!("dump completed before persistence: {result:?}"),
            requested = dump_request => requested.unwrap(),
        }
        assert_eq!(dump_spans(&spans).len(), 0);
        let acknowledged_at = SystemTime::now();
        persisted.send(()).unwrap();
        assert_eq!(dump.await.unwrap(), b"durable snapshot handle");
        let recorded = dump_spans(&spans);
        assert_eq!(recorded.len(), 1, "{:?}", spans.get_finished_spans().unwrap());
        let span = &recorded[0];
        assert!(span.start_time <= configured_at);
        assert!(span.end_time >= acknowledged_at);
        assert_eq!(span.span_context.trace_id(), parent.trace_id());
        assert_eq!(span.parent_span_id, parent.span_id());
        assert_eq!(attribute(span, "monty.outcome"), Some("ok".into()));
        configured_at
    };
    let request = pb::ClientRequest {
        kind: Some(pb::client_request::Kind::Dump(pb::Dump {})),
        ..pb::ClientRequest::default()
    };
    let mut discard = |_: &pb::WorkerEvent| Box::pin(ready(())) as PrintFuture;
    let reply = checkout.turn_raw(&request, &mut discard).await.unwrap();
    assert!(matches!(reply.kind, Some(pb::worker_event::Kind::DumpResult(_))));
    assert_eq!(dump_spans(&spans).len(), 1);
    assert!(matches!(checkout.dump().await, Err(PoolError::Runtime(_))));
    let recorded = dump_spans(&spans);
    assert_eq!(recorded.len(), 2);
    assert_eq!(attribute(&recorded[1], "monty.outcome"), Some("error".into()));
    assert_eq!(
        attribute(&recorded[1], "logfire.level_num"),
        provider_error_level(&logfire, &spans)
    );
    assert_eq!(attribute(&recorded[1], "logfire.msg_template"), None);
    let emitted = logs.get_emitted_logs().unwrap();
    let errors: Vec<_> = emitted
        .iter()
        .filter(|entry| {
            entry
                .record
                .attributes_iter()
                .any(|(key, _)| key.as_str() == "exc_type")
        })
        .collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].record.trace_context().unwrap().span_id,
        recorded[1].span_context.span_id()
    );
    assert_eq!(
        recorded[1].status,
        Status::error("RuntimeError: snapshot store refused the dump")
    );
    {
        let dump = checkout.dump();
        tokio::pin!(dump);
        tokio::select! {
            result = &mut dump => panic!("dump completed before cancellation: {result:?}"),
            requested = cancel_request => requested.unwrap(),
        }
        assert_eq!(dump_spans(&spans).len(), 2);
    }
    let recorded = dump_spans(&spans);
    assert_eq!(recorded.len(), 3);
    assert_eq!(attribute(&recorded[2], "monty.outcome"), Some("cancelled".into()));
    assert!(recorded[2].start_time >= configured_at);
    checkout.terminate().await;
    pool.close().await;
    join_server(server).await;
}

/// Worker-controlled fatal messages stay bounded while formatting the status, including UTF-8 boundaries.
#[tokio::test]
async fn dump_status_caps_fatal_worker_messages() {
    const LIMIT: usize = 64 * 1024;
    const PREFIX: &str = "monty worker crashed: x";
    let spans = InMemorySpanExporter::default();
    let logfire = local_logfire()
        .with_additional_span_processor(SimpleSpanProcessor::new(spans.clone()))
        .finish()
        .unwrap();
    let options = CheckoutOptions::default().with_telemetry(Some(TelemetryContext::for_logfire(logfire.clone())));
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        assert!(matches!(
            read_request(&mut socket),
            pb::client_request::Kind::Configure(_)
        ));
        send_event(&mut socket, &event_kind(pb::worker_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Dump(_)));
        send_event(
            &mut socket,
            &event_kind(pb::worker_event::Kind::FatalError(pb::FatalError {
                message: format!("x{}", "é".repeat(1024 * 1024)),
            })),
        );
        assert!(socket.read().is_none());
    });
    let pool = Pool::new(config).await.unwrap();
    let mut checkout = pool.checkout_with(&ReplConfig::default(), options).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(10), checkout.dump()).await.unwrap(),
        Err(PoolError::Crashed { .. })
    ));
    let recorded = dump_spans(&spans);
    assert_eq!(recorded.len(), 1);
    let Status::Error { description } = &recorded[0].status else {
        panic!("fatal dump status");
    };
    assert_eq!(description.len(), LIMIT - 1);
    assert_eq!(
        description.as_ref(),
        format!("{PREFIX}{}", "é".repeat((LIMIT - PREFIX.len()) / 2))
    );
    assert_eq!(attribute(&recorded[0], "monty.error.truncated"), Some(true.into()));
    pool.close().await;
    join_server(server).await;
}

/// Restore latency ends at the API return. The restored execution keeps its callback span.
#[tokio::test]
async fn restore_span_ends_at_the_first_suspension() {
    timeout(Duration::from_secs(10), Box::pin(restore_span_scenario()))
        .await
        .expect("restore telemetry scenario completed");
}

/// Hold the opening reply and check automatic span closure on each load outcome.
async fn restore_span_scenario() {
    let exporter = InMemorySpanExporter::default();
    let logfire = local_logfire()
        .with_additional_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .finish()
        .expect("logfire");
    let parent = SpanContext::new(
        TraceId::from(27_u128),
        SpanId::from(28_u64),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let options = CheckoutOptions::default()
        .with_telemetry(Some(TelemetryContext::for_logfire(logfire).with_parent(parent.clone())));
    let caller = Traceparent::new(31, 32, TraceFlags::SAMPLED.to_u8()).unwrap();
    let (loading, load_request) = oneshot::channel();
    let (loaded, load_reply) = mpsc::channel();
    let (cancelling, cancel_request) = oneshot::channel();
    let (mock, config) = mock_pool_config().await;
    let server = thread::spawn(move || {
        let mut socket = mock.accept();
        let request = socket.read_unsplit().expect("the first request");
        let request = decode_frame::<pb::ClientRequest>(request.as_ref()).expect("decode request");
        let wire_parent = request.parsed_traceparent().expect("opening load carries its span");
        let Some(pb::client_request::Kind::Configure(configure)) = request.kind else {
            panic!("expected Configure carrying Load");
        };
        assert!(matches!(configure.first, Some(pb::configure::First::Load(_))));
        loading.send((SystemTime::now(), wire_parent)).unwrap();
        load_reply.recv().unwrap();
        send_event(
            &mut socket,
            &pb::WorkerEvent {
                restored_script_name: Some("restored.py".to_owned()),
                execution_clock: Some(pb::ExecutionClock::Wall as i32),
                ..event_kind(pb::worker_event::Kind::NameLookup(pb::NameLookup {
                    name: "value".to_owned(),
                    object_id: None,
                    position: Some((&position()).into()),
                }))
            },
        );
        assert!(matches!(
            read_request(&mut socket),
            pb::client_request::Kind::ResumeNameLookup(_)
        ));
        send_event(
            &mut socket,
            &event_kind(pb::worker_event::Kind::Complete(pb::Complete::from(MontyObject::int(
                42,
            )))),
        );
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Load(_)));
        send_event(&mut socket, &event_kind(pb::worker_event::Kind::Ok(pb::Ok {})));
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Load(_)));
        send_event(
            &mut socket,
            &event_kind(pb::worker_event::Kind::Error(pb::Error {
                exception: Some(pb::RaisedException {
                    exc_type: "RuntimeError".to_owned(),
                    message: Some("snapshot unavailable".to_owned()),
                    ..Default::default()
                }),
            })),
        );
        assert!(matches!(read_request(&mut socket), pb::client_request::Kind::Load(_)));
        cancelling.send(()).unwrap();
        assert!(socket.read().is_none());
    });
    let pool = Pool::new(config).await.unwrap();
    let mut checkout = pool.checkout_with(&ReplConfig::default(), options).await.unwrap();
    let mut ignore_print = no_print;
    let load_wire_parent = {
        let restore = checkout.restore_with(
            vec![1, 2, 3],
            vec![],
            TurnOptions::default().with_traceparent(Some(caller)),
            &mut ignore_print,
        );
        tokio::pin!(restore);
        let (sent_at, wire_parent) = tokio::select! {
            result = &mut restore => panic!("restore completed before the load reply: {result:?}"),
            sent = load_request => sent.unwrap(),
        };
        assert!(
            exporter
                .get_finished_spans()
                .unwrap()
                .iter()
                .all(|span| span.name != "restore")
        );
        let replied_at = SystemTime::now();
        loaded.send(()).unwrap();
        let (suspension, script_name) = restore.await.unwrap();
        assert!(matches!(suspension, Some(TurnEvent::NameLookup { .. })));
        assert_eq!(script_name.as_deref(), Some("restored.py"));
        let spans = exporter.get_finished_spans().unwrap();
        let span = spans.iter().find(|span| span.name == "restore").unwrap();
        assert!(span.start_time <= sent_at);
        assert!(span.end_time >= replied_at);
        assert_eq!(span.span_context.trace_id(), TraceId::from(caller.trace_id()));
        assert_eq!(span.parent_span_id, SpanId::from(caller.span_id()));
        assert_eq!(attribute(span, "monty.outcome"), Some("ok".into()));
        assert!(spans.iter().all(|span| span.name != "load"));
        wire_parent
    };
    let callback = checkout.callback_context().span().span_context().clone();
    assert_eq!(callback.trace_id(), TraceId::from(caller.trace_id()));
    let result = checkout
        .resume_name_lookup(NameLookupResult::from(MontyObject::int(42)), &mut no_print)
        .await
        .unwrap();
    assert!(matches!(result, TurnEvent::Complete(value) if value == MontyObject::int(42)));
    let spans = exporter.get_finished_spans().unwrap();
    let restore = spans.iter().find(|span| span.name == "restore").unwrap();
    let load = spans.iter().find(|span| span.name == "load").unwrap();
    let call = spans.iter().find(|span| span.name == "name lookup {name}").unwrap();
    assert_eq!(load.parent_span_id, restore.span_context.span_id());
    assert_eq!(TraceId::from(load_wire_parent.trace_id()), load.span_context.trace_id());
    assert_eq!(SpanId::from(load_wire_parent.span_id()), load.span_context.span_id());
    assert_eq!(call.parent_span_id, load.span_context.span_id());
    assert_eq!(callback.span_id(), load.span_context.span_id());
    assert_eq!(attribute(load, "state_bytes"), Some("3".into()));

    let raw_load = pb::ClientRequest {
        kind: Some(pb::client_request::Kind::Load(pb::Load { state: vec![4].into() })),
        ..pb::ClientRequest::default()
    };
    let mut discard = |_: &pb::WorkerEvent| Box::pin(ready(())) as PrintFuture;
    let raw_reply = checkout.turn_raw(&raw_load, &mut discard).await.unwrap();
    assert!(matches!(raw_reply.kind, Some(pb::worker_event::Kind::Ok(_))));
    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.iter().filter(|span| span.name == "restore").count(), 1);
    let raw_span = spans
        .iter()
        .find(|span| attribute(span, "state_bytes") == Some("1".into()))
        .unwrap();
    assert_ne!(raw_span.parent_span_id, restore.span_context.span_id());

    assert!(matches!(
        checkout.restore(vec![5], vec![], &mut ignore_print).await,
        Err(PoolError::Runtime(_))
    ));
    let spans = exporter.get_finished_spans().unwrap();
    let refused = spans.iter().rfind(|span| span.name == "restore").unwrap();
    assert_eq!(attribute(refused, "monty.outcome"), Some("error".into()));
    assert_eq!(refused.status, Status::error("restore failed"));
    {
        let restore = checkout.restore(vec![6], vec![], &mut ignore_print);
        tokio::pin!(restore);
        tokio::select! {
            result = &mut restore => panic!("restore completed before cancellation: {result:?}"),
            requested = cancel_request => requested.unwrap(),
        }
    }
    let spans = exporter.get_finished_spans().unwrap();
    let mut restores = spans.iter().filter(|span| span.name == "restore");
    assert_eq!(
        restores.next().unwrap().span_context.span_id(),
        restore.span_context.span_id()
    );
    assert_eq!(
        restores.next().unwrap().span_context.span_id(),
        refused.span_context.span_id()
    );
    assert_eq!(
        attribute(restores.next().unwrap(), "monty.outcome"),
        Some("cancelled".into())
    );
    assert!(restores.next().is_none());
    let ambient = local_logfire().finish().expect("ambient logfire");
    {
        let _ambient = set_local_logfire(ambient);
        drop(checkout);
    }
    pool.close().await;
    join_server(server).await;
}

/// Derive provider-owned error metadata from a plain span that Monty does not customize.
fn provider_error_level(logfire: &Logfire, exporter: &InMemorySpanExporter) -> Option<Value> {
    let _provider = set_local_logfire(logfire.clone());
    let span = tracing::info_span!("provider error reference");
    let _ = span.context();
    span.set_status(Status::error("provider error reference"));
    drop(span);
    let recorded = exporter.get_finished_spans().unwrap();
    let reference = recorded
        .iter()
        .find(|span| span.name == "provider error reference")
        .unwrap();
    attribute(reference, "logfire.level_num")
}

/// Read only completed dump operations from the synchronous exporter.
fn dump_spans(exporter: &InMemorySpanExporter) -> Vec<SpanData> {
    exporter
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .filter(|span| span.name == "dump")
        .collect()
}

/// Read the last recorded value when Logfire updates an attribute such as severity.
fn attribute(span: &SpanData, name: &str) -> Option<Value> {
    span.attributes
        .iter()
        .rev()
        .find(|kv| kv.key.as_str() == name)
        .map(|kv| kv.value.clone())
}
