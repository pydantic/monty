//! `monty subprocess`: protocol worker mode.
//!
//! A thin stdio shell around [`monty_proto::worker::Worker`] (the transport-agnostic
//! state machine). Reads framed [`pb::ClientRequest`]s from stdin and writes
//! the [`pb::WorkerEvent`]s the worker emits to stdout (see `monty-proto` for the
//! schema and protocol rules). The worker is strictly turn-based: one request
//! in, zero or more streamed `Print` and `StreamedCalls` events out, then
//! exactly one turn-ending event.
//!
//! Crash isolation is the entire point of this mode: the client must treat a
//! worker that exits (or EOFs) *without* a `FatalError` event as crashed —
//! stack overflows and allocator aborts produce no final frame.
//!
//! In this mode stdout carries only protocol frames; diagnostics go to stderr.

use std::{io, panic, process::ExitCode, thread};

use monty_proto::{
    FrameError, FrameReader, pb,
    worker::{EventSink, HandleOutcome, SessionBudget, Worker, fatal_error_event, protocol_violation},
    write_frame,
};
use monty_types::memory_limit_with_headroom;

/// BSD `sysexits.h` "remote error in protocol" — the frame stream desynchronized, or
/// an event too large to frame left the response unsendable.
const EX_PROTOCOL: u8 = 76;

/// Native stack for the worker thread. The interpreter's container `repr` and
/// `==` and the value exporter recurse once per nesting level, up to the
/// 1000-frame recursion limit: that fits the 8 MiB main thread of Linux and
/// macOS but not Windows' 1 MiB. A fixed size gives every OS the same budget;
/// the memory is reserved, not committed.
const WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Runs [`serve`] on a thread with [`WORKER_STACK_SIZE`] of stack and returns
/// its exit code. A panic there has already written its `FatalError` frame in
/// the panic hook, so it exits with 101 like an unwound main thread.
///
/// `prewarm_type_checker` selects [`Worker::with_prewarmed_type_checker`].
pub(crate) fn run(prewarm_type_checker: bool) -> ExitCode {
    thread::Builder::new()
        .name("monty-worker".to_owned())
        .stack_size(WORKER_STACK_SIZE)
        .spawn(move || serve(prewarm_type_checker))
        .expect("failed to spawn the worker thread")
        .join()
        .unwrap_or(ExitCode::from(101))
}

/// The worker loop: one request in, its events out, until EOF, `Shutdown` or a fatal error.
fn serve(prewarm_type_checker: bool) -> ExitCode {
    install_panic_hook();
    let mut reader = FrameReader::new(io::stdin().lock());
    let mut worker = if prewarm_type_checker {
        Worker::with_prewarmed_type_checker()
    } else {
        Worker::default()
    };
    let mut sink = StdoutSink;
    // set the memory baseline on the pristine worker: a session's first request
    // is decoded before its ceiling is armed, and must not count as overhead
    apply_memory_limit(&worker);
    worker.warm_up();

    loop {
        match reader.read::<pb::ClientRequest>() {
            Ok(Some(request)) => {
                let outcome = worker.handle(request, &mut sink);
                apply_memory_limit(&worker);
                match outcome {
                    // after the re-arm, so a `Reset` that dropped the checker has
                    // lowered the baseline before the new one is added to it
                    Ok(HandleOutcome::Continue) => worker.warm_up(),
                    Ok(HandleOutcome::Shutdown) => return ExitCode::SUCCESS,
                    // the worker emitted a FatalError (e.g. version skew) and cannot
                    // keep serving — exit non-zero so the client sees a clean cause
                    Ok(HandleOutcome::Fatal) => return ExitCode::from(4),
                    // an oversize event was rejected before any bytes hit the
                    // wire, so the stream is still in sync and the client can
                    // receive a parseable last gasp
                    Err(FrameError::FrameTooLarge { len, max }) => {
                        fatal(
                            &worker,
                            &mut sink,
                            &format!("response frame of {len} bytes exceeds maximum of {max} bytes"),
                        );
                        return ExitCode::from(EX_PROTOCOL);
                    }
                    // writing to stdout failed: the client is gone, nothing left to do
                    Err(_) => return ExitCode::from(3),
                }
            }
            // clean EOF at a frame boundary: the client closed stdin
            Ok(None) => return ExitCode::SUCCESS,
            // the frame arrived intact but its payload didn't decode — this
            // includes values failing semantic validation (bad dates, unknown
            // enum names), which happens during decode. The stream is still
            // in sync, so answer with a turn-ending error and keep serving.
            Err(FrameError::Decode(err)) => {
                if sink
                    .send(&protocol_violation(&format!("malformed request: {err}")))
                    .is_err()
                {
                    return ExitCode::from(3);
                }
            }
            Err(err) => {
                // the stream is desynchronized — unrecoverable by design
                fatal(&worker, &mut sink, &format!("malformed request frame: {err}"));
                return ExitCode::from(EX_PROTOCOL);
            }
        }
    }
}

/// Applies the hard memory limit of whatever session the worker now holds, at
/// start-up and after every request.
///
/// Reading the worker's state rather than the request is what keeps the limit
/// honest: a rejected `Configure` changes nothing, a `Load` brings the dump's
/// limits capped by the destination policy, and `Reset` ends the session. It lives in the shell rather than
/// in the shared state machine because each entry point declares its own
/// allocator: the wasm worker does the same thing in its own turn loop.
fn apply_memory_limit(worker: &Worker) {
    apply_budget(worker.session_budget());
}

/// Sets the allocator's hard ceiling for `budget`.
fn apply_budget(budget: SessionBudget) {
    let hard_memory_limit = memory_limit_with_headroom(budget.max_memory, budget.type_check);
    monty_alloc::set_hard_limit(hard_memory_limit).expect("monty-runtime must install LimitedAllocator globally");
}

/// Writes framed worker events to stdout.
///
/// Framing is stateless and `Stdout` handles share one global buffer, so a
/// fresh handle per write is safe.
struct StdoutSink;

impl EventSink for StdoutSink {
    fn send(&mut self, event: &pb::WorkerEvent) -> Result<(), FrameError> {
        write_frame(&mut io::stdout(), event)
    }

    /// A session opened by its first request runs under its ceiling, not the unconfigured one.
    fn session_opened(&mut self, budget: SessionBudget) {
        apply_budget(budget);
    }
}

/// Emits a best-effort `FatalError` event, duplicated to stderr. Used only for
/// unrecoverable conditions detected by the shell — the worker exits right
/// after.
fn fatal(worker: &Worker, sink: &mut impl EventSink, message: &str) {
    eprintln!("monty subprocess fatal error: {message}");
    let _ = sink.send(&worker.fatal_event(message));
}

/// Installs a panic hook that emits a best-effort `FatalError` frame before
/// the default unwind, giving the client a parseable last gasp for ordinary
/// panics. Hard crashes (stack overflow, allocator abort) bypass this — the
/// client's contract is "exit without FatalError == crash".
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // stdout's lock is reentrant on the same thread, and if the panic
        // interrupted a write its buffer may hold a partial frame we cannot
        // complete — a corrupt tail is fine, the client already treats it as
        // a crash. The hook has no `Worker` in scope, so the event is unstamped.
        let _ = write_frame(
            &mut io::stdout(),
            &fatal_error_event(&format!("worker panicked: {info}")),
        );
        default_hook(info);
    }));
}
