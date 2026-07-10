//! The lean wasip1 Monty worker.
//!
//! This is the browser/wasm analog of `monty subprocess`: a [`Child`] state
//! machine ([`monty_proto::worker`]) driven one protocol turn per call. Where the
//! native subprocess loops forever reading framed requests from a pipe, the
//! wasm worker is event-driven — the host (a Web Worker drive loop) hands it one
//! request frame per `postMessage` and reads back that turn's reply frames — so
//! the module exports a single `monty_dispatch_turn` that runs exactly one turn
//! and returns.
//!
//! ## Transport
//!
//! The module is a WASI *reactor*: it has no `main`, the host instantiates it
//! once and the instance (with its session [`Child`]) lives across turns. Bytes
//! cross over WASI stdio, supplied and captured by the host's WASI shim:
//!
//! - **stdin** carries the single framed `ParentRequest` for the turn;
//! - **stdout** receives the turn's framed `ChildEvent`s (zero or more `Print`s
//!   then one turn-ending event).
//!
//! Using stdio keeps the FFI surface to one zero-argument export and routes all
//! marshalling through safe `std::io` — there is no hand-written pointer
//! arithmetic. The host resets the stdin/stdout buffers around each call.
//!
//! ## Crash isolation
//!
//! Exactly as in the subprocess model, a turn that ends *without* a turn-ending
//! event means the instance trapped (stack overflow, allocator abort) and the
//! host must discard it. A graceful turn always emits one terminating event.

use std::{
    cell::RefCell,
    io::{self, Read, Write},
};

use monty_proto::{
    decode_frame, pb,
    worker::{Child, HandleOutcome, dispatch_frame},
};
use pb::child_event::Kind;
use prost::Message;
use serde::Serialize;

thread_local! {
    /// The session worker, created on first use and reused across turns. wasip1
    /// is single-threaded, so a thread-local `RefCell` is the whole story — no
    /// locking, no `static mut`.
    static CHILD: RefCell<Child> = RefCell::new(Child::new());
}

/// Return code of [`monty_dispatch_turn`], read by the host drive loop.
mod turn_status {
    /// The turn completed; keep the instance and serve the next request.
    pub const CONTINUE: i32 = 0;
    /// The child asked to shut down (or hit an unrecoverable protocol error);
    /// the host should drop the instance.
    pub const SHUTDOWN: i32 = 1;
    /// stdio itself failed — the host's buffers are misconfigured. Treated as
    /// terminal.
    pub const IO_ERROR: i32 = 2;
}

/// Runs one protocol turn: reads the framed request from stdin, drives the
/// session, and writes the framed reply events to stdout.
///
/// Returns one of the [`turn_status`] codes. The reply (including any
/// turn-ending error or `FatalError`) is always written to stdout before this
/// returns, so the host reads stdout regardless of the status code.
#[unsafe(no_mangle)]
pub extern "C" fn monty_dispatch_turn() -> i32 {
    let mut request = Vec::new();
    if io::stdin().read_to_end(&mut request).is_err() {
        return turn_status::IO_ERROR;
    }

    let (reply, outcome) = CHILD.with_borrow_mut(|child| dispatch_frame(child, &request));

    let mut stdout = io::stdout();
    if stdout.write_all(&reply).and_then(|()| stdout.flush()).is_err() {
        return turn_status::IO_ERROR;
    }

    match outcome {
        HandleOutcome::Continue => turn_status::CONTINUE,
        // a fatal child error (e.g. version skew) terminates the worker just
        // like a clean shutdown; the emitted FatalError frame carries the cause
        HandleOutcome::Shutdown | HandleOutcome::Fatal => turn_status::SHUTDOWN,
    }
}

/// Decodes framed `ChildEvent` messages from stdin using the Rust protobuf
/// implementation and writes a compact JSON event list to stdout.
#[unsafe(export_name = "monty_decode_child_events")]
#[must_use]
pub extern "C" fn monty_decode_child_events() -> i32 {
    let mut input = Vec::new();
    if io::stdin().read_to_end(&mut input).is_err() {
        return turn_status::IO_ERROR;
    }

    let mut events = Vec::new();
    let mut offset = 0;
    while offset < input.len() {
        let Some(frame) = next_frame(&input, &mut offset) else {
            return turn_status::IO_ERROR;
        };
        let Ok(event) = decode_frame::<pb::ChildEvent>(frame) else {
            return turn_status::IO_ERROR;
        };
        let Some(decoded) = DecodedChildEvent::from_event(event) else {
            return turn_status::IO_ERROR;
        };
        events.push(decoded);
    }

    let Ok(output) = serde_json::to_vec(&events) else {
        return turn_status::IO_ERROR;
    };
    let mut stdout = io::stdout();
    if stdout.write_all(&output).and_then(|()| stdout.flush()).is_err() {
        return turn_status::IO_ERROR;
    }
    turn_status::CONTINUE
}

#[derive(Serialize)]
struct DecodedChildEvent {
    kind: u8,
    bytes: Vec<u8>,
}

fn next_frame<'a>(input: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    let header = input.get(*offset..*offset + 4)?;
    let len = u32::from_le_bytes(header.try_into().ok()?) as usize;
    *offset += 4;
    let frame = input.get(*offset..*offset + len)?;
    *offset += len;
    Some(frame)
}

impl DecodedChildEvent {
    fn from_event(event: pb::ChildEvent) -> Option<Self> {
        let (kind, bytes) = match event.kind? {
            Kind::Print(value) => (1, value.encode_to_vec()),
            Kind::FunctionCall(value) => (2, value.encode_to_vec()),
            Kind::OsCall(value) => (3, value.encode_to_vec()),
            Kind::NameLookup(value) => (4, value.encode_to_vec()),
            Kind::ResolveFutures(value) => (5, value.encode_to_vec()),
            Kind::Complete(value) => (6, value.encode_to_vec()),
            Kind::Error(value) => (7, value.encode_to_vec()),
            Kind::TypingError(value) => (8, value.encode_to_vec()),
            Kind::DumpResult(value) => (9, value.encode_to_vec()),
            Kind::Ok(value) => (10, value.encode_to_vec()),
            Kind::FatalError(value) => (11, value.encode_to_vec()),
        };
        Some(Self { kind, bytes })
    }
}
