//! Fuzz target for the heap refcount/reachability model.
//!
//! Decodes an arbitrary byte stream into [`Op`]s and runs them against a fresh
//! heap through [`run_ops_ignoring_invalid`]. Operations that cannot apply to the
//! current state are skipped, so random input still reaches deep into the
//! operation surface. A panic or a [`ModelError::Mismatch`] is a heap bug.
//!
//! Builds with `--features heap-model` (which turns on `monty/ref-count-return`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use monty::test_util::heap::{Kind, MAX_SLOTS, Op, run_ops_ignoring_invalid};

/// Upper bound on decoded operations per input; the model is sized for short
/// sequences that still reach multi-edge cycles.
const MAX_OPS: usize = 64;

/// Decodes a byte stream into operations.
///
/// Layout per operation: one tag byte and one slot byte, then tag-specific
/// bytes. A truncated tail stops decoding rather than producing a partial op.
fn decode(data: &[u8]) -> Vec<Op> {
    let slots = MAX_SLOTS as u8;
    let mut ops = Vec::new();
    let mut rest = data;
    while ops.len() < MAX_OPS {
        let Some((&tag, tail)) = rest.split_first() else {
            break;
        };
        let Some((&slot_byte, tail)) = tail.split_first() else {
            break;
        };
        rest = tail;
        let slot = slot_byte % slots;
        let op = match tag % 10 {
            0 => {
                let Some((&kind_byte, tail)) = rest.split_first() else {
                    break;
                };
                let Some((&count, tail)) = tail.split_first() else {
                    break;
                };
                rest = tail;
                let mut children = Vec::new();
                for _ in 0..count % 5 {
                    let Some((&child, tail)) = rest.split_first() else {
                        break;
                    };
                    children.push(child % slots);
                    rest = tail;
                }
                Op::Alloc {
                    slot,
                    kind: decode_kind(kind_byte),
                    children,
                }
            }
            1 => Op::Drop(slot),
            2 => Op::Inc(slot),
            3 => Op::Dec(slot),
            4 => Op::Collect,
            5 => Op::DumpRestore,
            6 => Op::AllocDuringRead(slot),
            7 => {
                let Some((&child, tail)) = rest.split_first() else {
                    break;
                };
                rest = tail;
                Op::Append {
                    slot,
                    child: child % slots,
                }
            }
            8 => Op::Pop(slot),
            _ => {
                let Some((&child, tail)) = rest.split_first() else {
                    break;
                };
                rest = tail;
                Op::Set {
                    slot,
                    child: child % slots,
                }
            }
        };
        ops.push(op);
    }
    ops
}

/// Maps a byte onto the modelled storage classes.
fn decode_kind(byte: u8) -> Kind {
    match byte % 8 {
        0 => Kind::List,
        1 => Kind::Tuple,
        2 => Kind::Cell,
        3 => Kind::Closure,
        4 => Kind::GatherFuture,
        5 => Kind::ExternalFuture,
        6 => Kind::Dict,
        _ => Kind::Set,
    }
}

fuzz_target!(|data: &[u8]| {
    let ops = decode(data);
    if let Err(err) = run_ops_ignoring_invalid(&ops) {
        panic!("{err} for ops {ops:?}");
    }
});
