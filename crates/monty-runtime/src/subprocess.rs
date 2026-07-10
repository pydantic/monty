//! `monty subprocess`: protocol child mode.
//!
//! A thin stdio shell around [`monty_proto::worker::Child`] (the transport-agnostic
//! state machine). Reads framed [`pb::ParentRequest`]s from stdin and writes
//! the [`pb::ChildEvent`]s the child emits to stdout (see `monty-proto` for the
//! schema and protocol rules). The child is strictly turn-based: one request
//! in, zero or more streamed `Print` events out, then exactly one turn-ending
//! event.
//!
//! Crash isolation is the entire point of this mode: the parent must treat a
//! child that exits (or EOFs) *without* a `FatalError` event as crashed —
//! stack overflows and allocator aborts produce no final frame.
//!
//! In this mode stdout carries only protocol frames; diagnostics go to stderr.

use std::{io, panic, process::ExitCode};

use monty_proto::{
    FrameError, FrameReader, pb,
    worker::{Child, EventSink, HandleOutcome, fatal_error_event, protocol_violation},
    write_frame,
};

/// Runs the subprocess child loop until EOF, `Shutdown`, or a fatal error.
pub(crate) fn run() -> ExitCode {
    install_panic_hook();
    let mut reader = FrameReader::new(io::stdin().lock());
    let mut child = Child::new();
    let mut sink = StdoutSink;

    loop {
        match reader.read::<pb::ParentRequest>() {
            Ok(Some(request)) => match child.handle(request, &mut sink) {
                Ok(HandleOutcome::Continue) => {}
                Ok(HandleOutcome::Shutdown) => return ExitCode::SUCCESS,
                // the child emitted a FatalError (e.g. version skew) and cannot
                // keep serving — exit non-zero so the parent sees a clean cause
                Ok(HandleOutcome::Fatal) => return ExitCode::from(4),
                // an oversize event was rejected before any bytes hit the
                // wire, so the stream is still in sync and the parent can
                // receive a parseable last gasp
                Err(FrameError::FrameTooLarge { len, max }) => {
                    fatal(
                        &child,
                        &mut sink,
                        &format!("response frame of {len} bytes exceeds maximum of {max} bytes"),
                    );
                    return ExitCode::from(2);
                }
                // writing to stdout failed: the parent is gone, nothing left to do
                Err(_) => return ExitCode::from(3),
            },
            // clean EOF at a frame boundary: the parent closed stdin
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
                fatal(&child, &mut sink, &format!("malformed request frame: {err}"));
                return ExitCode::from(2);
            }
        }
    }
}

/// Writes framed child events to stdout.
///
/// Framing is stateless and `Stdout` handles share one global buffer, so a
/// fresh handle per write is safe.
struct StdoutSink;

impl EventSink for StdoutSink {
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), FrameError> {
        write_frame(&mut io::stdout(), event)
    }
}

/// Emits a best-effort `FatalError` event, duplicated to stderr. Used only for
/// unrecoverable conditions detected by the shell — the child exits right
/// after.
fn fatal(child: &Child, sink: &mut impl EventSink, message: &str) {
    eprintln!("monty subprocess fatal error: {message}");
    let _ = sink.send(&child.fatal_event(message));
}

/// Installs a panic hook that emits a best-effort `FatalError` frame before
/// the default unwind, giving the parent a parseable last gasp for ordinary
/// panics. Hard crashes (stack overflow, allocator abort) bypass this — the
/// parent's contract is "exit without FatalError == crash".
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // stdout's lock is reentrant on the same thread, and if the panic
        // interrupted a write its buffer may hold a partial frame we cannot
        // complete — a corrupt tail is fine, the parent already treats it as
        // a crash. The hook has no `Child` in scope, so the event is unstamped.
        let _ = write_frame(
            &mut io::stdout(),
            &fatal_error_event(&format!("child panicked: {info}")),
        );
        default_hook(info);
    }));
}
