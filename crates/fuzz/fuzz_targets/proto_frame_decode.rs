//! Fuzz target for wire-frame decoding.
//!
//! Feeds arbitrary bytes to `monty_proto`'s frame decoder under a small
//! allocation budget. A parent treats a child's frames as untrusted, so decoding
//! must never panic and never leave the decode budget escaped; a message that
//! decodes must also re-encode and decode again identically.
#![no_main]

use libfuzzer_sys::fuzz_target;
use monty_proto::{decode_budget_remaining, decode_frame_with_budget, pb};
use prost::Message;

/// Small enough that a hostile length prefix or repeated field must trip the
/// budget instead of allocating freely; well above any legitimate frame the
/// tests send.
const DECODE_BUDGET: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    decode::<pb::ParentRequest>(data);
    decode::<pb::ChildEvent>(data);
});

/// Decodes one message type, asserting budget and round-trip invariants.
fn decode<M: Message + Default + PartialEq>(bytes: &[u8]) {
    let Ok(message) = decode_frame_with_budget::<M>(bytes, DECODE_BUDGET) else {
        assert!(decode_budget_remaining().is_none(), "decode budget leaked on error");
        return;
    };
    assert!(decode_budget_remaining().is_none(), "decode budget leaked on success");

    let encoded = message.encode_to_vec();
    let decoded = decode_frame_with_budget::<M>(&encoded, DECODE_BUDGET).expect("re-encoding a decoded message fails");
    assert!(
        decoded == message,
        "message does not round-trip through its own encoding"
    );
}
