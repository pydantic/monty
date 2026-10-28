//! `convert_event` validates every payload as an untrusted remote's, which the
//! codec component relies on: a frame that decodes but names a missing node or
//! breaks the arena order must be rejected before JavaScript indexes it.

use monty_proto::{BudgetVec, WireArena, WireFunctionCall, pb};
use monty_types::unstable::{MontyNode, NodeId};
use monty_wasm_types::{EventError, convert_event, push_print, types::Event};

/// An arena of `nodes`, as decoding collects them: unchecked.
fn arena(nodes: Vec<MontyNode>) -> WireArena {
    WireArena(BudgetVec::from(nodes))
}

fn complete(value: u32, nodes: Vec<MontyNode>) -> pb::child_event::Kind {
    pb::child_event::Kind::Complete(pb::Complete {
        value,
        values: Some(arena(nodes)),
    })
}

/// The message of an [`EventError::Invalid`], failing the test on anything else.
fn invalid(result: Result<Event, EventError>) -> String {
    match result {
        Err(EventError::Invalid(message)) => message,
        other => panic!("expected an invalid event, got {other:?}"),
    }
}

#[test]
fn a_valid_result_converts_with_its_root() {
    let nodes = vec![MontyNode::Int(1), MontyNode::List(vec![NodeId(0)])];
    let Ok(Event::Complete(event)) = convert_event(Some(complete(1, nodes))) else {
        panic!("expected a complete event");
    };
    assert_eq!(event.value, 1);
    assert_eq!(event.values.nodes.len(), 2);
}

#[test]
fn a_result_root_outside_its_arena_is_invalid() {
    assert_eq!(
        invalid(convert_event(Some(complete(5, vec![MontyNode::None])))),
        "invalid Complete payload: invalid value for Arena: value root 5 is out of range for an arena of 1 nodes"
    );
}

#[test]
fn a_child_that_does_not_precede_its_holder_is_invalid() {
    let nodes = vec![MontyNode::List(vec![NodeId(1)]), MontyNode::None];
    assert_eq!(
        invalid(convert_event(Some(complete(0, nodes)))),
        "invalid Complete payload: invalid value for Arena: value node 0 references node 1, which is not below it"
    );
}

#[test]
fn a_call_argument_outside_its_arena_is_invalid() {
    let call = WireFunctionCall {
        function_name: "f".to_owned(),
        values: arena(vec![MontyNode::None]),
        args: BudgetVec::from(vec![NodeId(3)]),
        kwargs: BudgetVec::new(),
        call_id: 1,
        object_id: None,
        allow_eager_await: false,
        position: None,
    };
    assert_eq!(
        invalid(convert_event(Some(pb::child_event::Kind::FunctionCall(call)))),
        "invalid FunctionCall payload: invalid value for Arena: value root 3 is out of range for an arena of 1 nodes"
    );
}

#[test]
fn a_lookup_receiver_that_is_not_a_uuid_is_invalid() {
    let lookup = pb::NameLookup {
        name: "x".to_owned(),
        object_id: Some(pb::Uuid {
            data: vec![1, 2, 3].into(),
        }),
        position: None,
    };
    assert_eq!(
        invalid(convert_event(Some(pb::child_event::Kind::NameLookup(lookup)))),
        "NameLookup.object_id is not a 16-byte uuid"
    );
}

#[test]
fn an_event_without_a_kind_is_invalid() {
    assert_eq!(invalid(convert_event(None)), "ChildEvent carried no kind");
}

#[test]
fn an_error_without_an_exception_is_invalid() {
    let error = pb::Error { exception: None };
    assert_eq!(
        invalid(convert_event(Some(pb::child_event::Kind::Error(error)))),
        "Error event carried no exception"
    );
}

#[test]
fn a_print_expands_into_one_event_per_stream_run() {
    let print = pb::Print {
        segments: vec![
            pb::PrintSegment {
                stream: pb::PrintStream::Stdout.into(),
                text: "out".to_owned(),
            },
            pb::PrintSegment {
                stream: pb::PrintStream::Stderr.into(),
                text: "err".to_owned(),
            },
        ]
        .into(),
    };
    let mut events = Vec::new();
    push_print(&print, &mut events);
    let runs: Vec<(bool, &str)> = events
        .iter()
        .map(|event| match event {
            Event::Print(print) => (print.stderr, print.text.as_str()),
            other => panic!("expected a print, got {other:?}"),
        })
        .collect();
    assert_eq!(runs, [(false, "out"), (true, "err")]);
    assert!(
        matches!(
            convert_event(Some(pb::child_event::Kind::Print(print))),
            Err(EventError::Invalid(_))
        ),
        "a print must go through push_print"
    );
}
