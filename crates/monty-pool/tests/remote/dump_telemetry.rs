//! Dump spans follow the public operation, including setup, errors and cancellation.

use std::{
    future::ready,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime},
};

use logfire::config::AdvancedOptions;
use monty_pool::{CheckoutOptions, Pool, PoolError, PrintFuture, ReplConfig, telemetry::TelemetryContext};
use monty_proto::pb;
use opentelemetry::{
    Value,
    trace::{SpanContext, SpanId, Status, TraceFlags, TraceId, TraceState},
};
use opentelemetry_sdk::{
    logs::{InMemoryLogExporter, SimpleLogProcessor},
    trace::{InMemorySpanExporter, SimpleSpanProcessor, SpanData},
};
use tokio::{sync::oneshot, time::timeout};

use super::{event_kind, join_server, mock_pool_config, read_request, send_event};

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
    let logfire = logfire::configure()
        .local()
        .send_to_logfire(false)
        .with_console(None)
        .with_install_panic_handler(false)
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
    let options = CheckoutOptions::default()
        .with_telemetry(Some(TelemetryContext::for_logfire(logfire).with_parent(parent.clone())));
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
    assert_eq!(attribute(&recorded[1], "logfire.level_num"), Some(17_i64.into()));
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
    const PREFIX: &str = "monty worker crashed: ";
    let spans = InMemorySpanExporter::default();
    let logfire = logfire::configure()
        .local()
        .send_to_logfire(false)
        .with_console(None)
        .with_install_panic_handler(false)
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
                message: "é".repeat(1024 * 1024),
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
    assert_eq!(description.len(), LIMIT);
    assert_eq!(
        description.as_ref(),
        format!("{PREFIX}{}", "é".repeat((LIMIT - PREFIX.len()) / 2))
    );
    assert_eq!(attribute(&recorded[0], "length_limit_exceeded"), Some(true.into()));
    pool.close().await;
    join_server(server).await;
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
