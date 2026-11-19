//! Checkout routing and the cost of recording a session on the process dispatcher.

use std::time::Instant;

use logfire::set_local_logfire;
use monty_proto::pb;
use monty_types::MontyObject;
use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SimpleSpanProcessor};

use super::{
    Recorder,
    tests::{event, request, test_logfire},
};
use crate::telemetry::TelemetryContext;

/// A reused recorder must stop overriding the dispatcher when the next checkout opts into ambient recording.
#[test]
fn ambient_checkout_uses_current_pipeline_and_propagated_parent() {
    let (ambient, ambient_spans, ambient_logs) = test_logfire();
    let (explicit, explicit_spans, explicit_logs) = test_logfire();
    let _guard = set_local_logfire(ambient);
    let parent = SpanContext::new(
        TraceId::from(7_u128),
        SpanId::from(42_u64),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let mut recorder = Recorder::new(None);
    for context in [TelemetryContext::for_logfire(explicit), TelemetryContext::ambient()] {
        recorder.set_adapter_context(context.with_parent(parent.clone()));
        recorder.begin_session(&pb::Configure::default());
        recorder.event(&event(pb::worker_event::Kind::Shutdown(pb::ShutdownDump::default())));
        recorder.begin_turn(&request(pb::client_request::Kind::Reset(pb::Reset {})));
    }
    for (exporter, logs) in [(ambient_spans, ambient_logs), (explicit_spans, explicit_logs)] {
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].span_context.trace_id(), parent.trace_id());
        assert_eq!(spans[0].parent_span_id, parent.span_id());
        let records = logs.get_emitted_logs().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.event_name(), Some("shutdown"));
        assert_eq!(
            records[0].record.trace_context().unwrap().span_id,
            spans[0].span_context.span_id()
        );
    }
}

/// Run this alone in a release test binary to benchmark the process-global dispatcher.
#[test]
#[ignore = "manual process-global recorder benchmark"]
fn benchmark_global_checkout_recording() {
    const SESSIONS: usize = 2000;
    let spans = InMemorySpanExporter::default();
    let logfire = logfire::configure()
        .send_to_logfire(false)
        .with_console(None)
        .with_metrics(None)
        .with_additional_span_processor(SimpleSpanProcessor::new(spans.clone()))
        .finish()
        .unwrap();
    let parent = SpanContext::new(
        TraceId::from(7_u128),
        SpanId::from(42_u64),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let configure = pb::Configure::default();
    let feed = request(pb::client_request::Kind::Feed(pb::Feed {
        code: "1".to_owned(),
        ..Default::default()
    }));
    let complete = event(pb::worker_event::Kind::Complete(pb::Complete::from(MontyObject::int(
        1,
    ))));
    let reset = request(pb::client_request::Kind::Reset(pb::Reset {}));
    let scoped = TelemetryContext::for_logfire(logfire).with_parent(parent.clone());
    let ambient = TelemetryContext::ambient().with_parent(parent.clone());
    for round in 0..=8 {
        let mut modes = [("guarded", &scoped), ("ambient", &ambient)];
        if round % 2 == 1 {
            modes.reverse();
        }
        for (mode, context) in modes {
            let start = Instant::now();
            for _ in 0..SESSIONS {
                let mut recorder = Recorder::new(Some(1));
                recorder.set_adapter_context(context.clone());
                recorder.begin_session(&configure);
                recorder.begin_turn(&feed);
                recorder.event(&complete);
                recorder.begin_turn(&reset);
            }
            let elapsed = start.elapsed();
            let exported = spans.get_finished_spans().unwrap();
            assert_eq!(exported.len(), SESSIONS * 2);
            for [feed, session] in exported.as_chunks::<2>().0 {
                assert_eq!(feed.name, "run code");
                assert_eq!(session.name, "session {script_name}");
                assert_eq!(feed.parent_span_id, session.span_context.span_id());
                assert_eq!(session.parent_span_id, parent.span_id());
                assert_eq!(feed.span_context.trace_id(), parent.trace_id());
                assert_eq!(session.span_context.trace_id(), parent.trace_id());
            }
            spans.reset();
            if round != 0 {
                println!("{mode} {} ns/session", elapsed.as_nanos() / SESSIONS as u128);
            }
        }
    }
}
