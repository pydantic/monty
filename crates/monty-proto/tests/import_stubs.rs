//! Borrowed validation of `import_stubs` answers: an oversized dict whose entries share
//! one source string must be rejected before any per-entry copy is made.

use monty_proto::{MAX_FRAME_LEN, import_stubs_from_answer, validate_import_stubs_answer};
use monty_types::{
    MontyObject,
    unstable::{MontyGraph, MontyNode, object_from_graph},
};

/// A dict of `mod.m{i}` → one shared `source_len`-byte string, built with the string
/// stored once so the answer's borrowed size far exceeds its heap footprint.
fn shared_source_answer(entries: usize, source_len: usize) -> MontyObject {
    let mut graph = MontyGraph::new();
    let source = graph.push(MontyNode::String("x".repeat(source_len)));
    let pairs = (0..entries)
        .map(|index| {
            let key = graph.push(MontyNode::String(format!("mod.m{index}")));
            (key, source)
        })
        .collect();
    let root = graph.push(MontyNode::Dict(pairs));
    object_from_graph(graph, root).expect("the root node is in range")
}

#[test]
fn oversized_shared_source_answer_is_rejected_before_copying() {
    // Enough 1 MiB entries to cross the frame limit while the graph holds the string once,
    // so only a borrowed check can reject it cheaply.
    let entries = MAX_FRAME_LEN as usize / (1 << 20) + 50;
    let answer = shared_source_answer(entries, 1 << 20);
    let asked = vec!["mod".to_owned()];
    let err = validate_import_stubs_answer(&answer, &asked).expect_err("over the frame limit");
    assert!(err.contains("frame limit"), "{err}");
    let err = import_stubs_from_answer(&answer, &asked).expect_err("over the frame limit");
    assert!(err.contains("frame limit"), "{err}");
}

#[test]
fn small_answer_still_builds_stubs() {
    let mut graph = MontyGraph::new();
    let source = graph.push(MontyNode::String("x: int\n".to_owned()));
    let key = graph.push(MontyNode::String("tools".to_owned()));
    let root = graph.push(MontyNode::Dict(vec![(key, source)]));
    let answer = object_from_graph(graph, root).expect("the root node is in range");

    validate_import_stubs_answer(&answer, &["tools".to_owned()]).expect("valid answer");
    let stubs = import_stubs_from_answer(&answer, &["tools".to_owned()]).expect("valid answer");
    assert_eq!(stubs.len(), 1);
    assert_eq!(stubs[0].module(), "tools");
    assert_eq!(stubs[0].source(), "x: int\n");

    let err = validate_import_stubs_answer(&answer, &["other".to_owned()]).expect_err("not asked");
    assert!(err.contains("not asked"), "{err}");
}
