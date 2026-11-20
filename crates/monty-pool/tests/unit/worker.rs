use monty_proto::{Traceparent, WireFunctionCall, pb};

use super::drop_worker_spans;

/// A subprocess worker's event keeps its payload but none of the spans it names.
#[test]
fn spans_named_by_a_subprocess_worker_are_dropped() {
    let traceparent = Traceparent::parse("00-000000000000000000000000000000a1-00000000000000c1-01");
    let call = WireFunctionCall {
        function_name: "fetch".to_owned(),
        call_id: 7,
        traceparent,
        ..WireFunctionCall::default()
    };
    let mut streamed = pb::WorkerEvent {
        kind: Some(pb::worker_event::Kind::StreamedCalls(pb::StreamedCalls {
            calls: vec![call.clone()].into(),
        })),
        traceparent: traceparent.map(|traceparent| traceparent.to_string()),
        ..pb::WorkerEvent::default()
    };
    drop_worker_spans(&mut streamed);
    assert_eq!(streamed.parsed_traceparent(), None);
    let Some(pb::worker_event::Kind::StreamedCalls(started)) = streamed.kind else {
        panic!("expected streamed calls");
    };
    assert_eq!(
        started.calls.into_inner(),
        [WireFunctionCall {
            traceparent: None,
            ..call
        }]
    );
}
