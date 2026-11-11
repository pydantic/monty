//! Deterministic seed sequences for the heap refcount/reachability model.
//!
//! These run under `cargo test` and under `cargo miri test` with
//! `ref-count-return` and `memory-model-checks`; the libFuzzer `heap_ops` target
//! explores the same operation surface.

#![cfg(all(feature = "ref-count-return", feature = "memory-model-checks"))]

use monty::test_util::heap::{Kind, ModelError, Op, run_ops};

/// One sequence per shape the oracle must catch: self and mutual cycles,
/// duplicate edges, a drop cascade, extra references, async payload edges,
/// dump/restore with pending candidates, allocation during a live read, list
/// mutation, and childless containers.
fn seeds() -> Vec<Vec<Op>> {
    vec![
        // Self cycle through one list edge.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Append { slot: 0, child: 0 },
            Op::Drop(0),
            Op::Collect,
        ],
        // Two-node cycle.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Alloc {
                slot: 1,
                kind: Kind::List,
                children: vec![0],
            },
            Op::Append { slot: 0, child: 1 },
            Op::Drop(0),
            Op::Drop(1),
            Op::Collect,
        ],
        // Cycle through a cell: the closure owns the cell and the cell is
        // rewritten to own the closure.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::Cell,
                children: vec![],
            },
            Op::Alloc {
                slot: 1,
                kind: Kind::Closure,
                children: vec![0],
            },
            Op::Set { slot: 0, child: 1 },
            Op::Drop(0),
            Op::Drop(1),
            Op::Collect,
        ],
        // Duplicate self-edges: each is an independent refcount.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Append { slot: 0, child: 0 },
            Op::Append { slot: 0, child: 0 },
            Op::Drop(0),
            Op::Collect,
        ],
        // Drop cascade through a cell and a closure with duplicate cells.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::Cell,
                children: vec![],
            },
            Op::Alloc {
                slot: 1,
                kind: Kind::Cell,
                children: vec![0],
            },
            Op::Alloc {
                slot: 2,
                kind: Kind::Closure,
                children: vec![1, 1],
            },
            Op::Drop(2),
            Op::Drop(1),
            Op::Drop(0),
            Op::Collect,
        ],
        // Extra references outside any slot keep an otherwise dropped object alive.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Inc(0),
            Op::Inc(0),
            Op::Dec(0),
            Op::Dec(0),
            Op::Drop(0),
            Op::Collect,
        ],
        // Async payload edges: gather items, then the external future's sleep result.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Alloc {
                slot: 1,
                kind: Kind::ExternalFuture,
                children: vec![0],
            },
            Op::Alloc {
                slot: 2,
                kind: Kind::GatherFuture,
                children: vec![1, 1],
            },
            Op::Drop(2),
            Op::Drop(1),
            Op::Drop(0),
            Op::Collect,
        ],
        // A snapshot between the two halves of a pending cycle.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Alloc {
                slot: 1,
                kind: Kind::List,
                children: vec![0],
            },
            Op::Append { slot: 0, child: 1 },
            Op::Drop(0),
            Op::DumpRestore,
            Op::Drop(1),
            Op::Collect,
        ],
        // Allocation while a reader pins an entry.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::AllocDuringRead(0),
        ],
        // Pop releases the element reference and the model drops the edge.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Alloc {
                slot: 1,
                kind: Kind::List,
                children: vec![0],
            },
            Op::Append { slot: 1, child: 0 },
            Op::Pop(1),
            Op::Drop(1),
            Op::Drop(0),
            Op::Collect,
        ],
        // Childless storage classes and a tuple edge.
        vec![
            Op::Alloc {
                slot: 0,
                kind: Kind::List,
                children: vec![],
            },
            Op::Alloc {
                slot: 1,
                kind: Kind::Tuple,
                children: vec![0],
            },
            Op::Alloc {
                slot: 2,
                kind: Kind::Dict,
                children: vec![],
            },
            Op::Alloc {
                slot: 3,
                kind: Kind::Set,
                children: vec![],
            },
            Op::Drop(1),
            Op::Drop(2),
            Op::Drop(3),
            Op::Drop(0),
            Op::Collect,
        ],
    ]
}

#[test]
fn heap_model_seeds() {
    for (index, ops) in seeds().iter().enumerate() {
        run_ops(ops).unwrap_or_else(|err| panic!("seed {index}: {err}"));
    }
}

#[test]
fn malformed_sequence_is_invalid_not_a_mismatch() {
    let err = run_ops(&[Op::Drop(9)]).expect_err("out-of-range slot must be rejected");
    assert!(matches!(err, ModelError::Invalid(_)), "got {err}");
}
