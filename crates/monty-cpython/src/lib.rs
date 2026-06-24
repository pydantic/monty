//! `monty-cpython`: a Monty wire-protocol child worker that runs fed code in
//! embedded CPython.
//!
//! It speaks the same protocol as `monty --subprocess` (so the existing
//! `monty-pool` can drive it) but, instead of running Monty, executes each
//! snippet in a real CPython interpreter and routes every undefined name back to
//! the parent as a `FunctionCall` — the Rust port of the `proto_child.py`
//! reference client. See [`pyexec`] for the `dict.__missing__` mechanism.
//!
//! Transports are pluggable ([`Transport`]); `--subprocess` uses stdio framed
//! like the Monty child, and the WebSocket modes connect to a relay or listen
//! for a parent.
//!
//! SECURITY: full CPython is **not** a sandbox. This worker runs untrusted code
//! with no isolation of its own — that is the deployment's responsibility (a
//! locked-down container, or a relay-provisioned sandbox). See this crate's
//! `README.md`.

mod events;
pub mod pyexec;
pub mod session;
pub mod transport;

use std::{cell::RefCell, env, process::ExitCode, rc::Rc};

use pyo3::prelude::*;

use crate::{
    session::Session,
    transport::{SharedTransport, StdioTransport, Transport, connect, listen},
};

/// Exit code for a usage error (bad/missing CLI mode).
const EXIT_USAGE: u8 = 64;
/// Exit code for a failure to initialize the embedded interpreter.
const EXIT_INIT: u8 = 70;
/// Exit code for a transport that could not be established (connect/bind).
const EXIT_TRANSPORT: u8 = 69;

/// Parses the CLI and runs the worker over the selected transport.
///
/// Modes:
/// - `--subprocess` / `--stdio` — framed stdio, drop-in for `monty-pool`.
/// - `--connect <ws-url>` — dial a relay (or a parent-as-server) as a client.
/// - `--listen <addr>` — bind and accept one parent (server mode).
#[must_use]
pub fn run() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let mode = args.get(1).map(String::as_str);
    let arg = args.get(2).map(String::as_str);
    match (mode, arg) {
        (Some("--subprocess" | "--stdio"), _) => run_with_transport(Box::new(StdioTransport::new())),
        (Some("--connect"), Some(url)) => match connect(url) {
            Ok(transport) => run_with_transport(Box::new(transport)),
            Err(err) => {
                eprintln!("monty-cpython: failed to connect to {url}: {err}");
                ExitCode::from(EXIT_TRANSPORT)
            }
        },
        (Some("--listen"), Some(addr)) => match listen(addr) {
            Ok(transport) => run_with_transport(Box::new(transport)),
            Err(err) => {
                eprintln!("monty-cpython: failed to listen on {addr}: {err}");
                ExitCode::from(EXIT_TRANSPORT)
            }
        },
        (mode, _) => {
            match mode {
                Some(arg) => eprintln!("monty-cpython: unknown or incomplete mode {arg:?}"),
                None => eprintln!("monty-cpython: missing mode"),
            }
            eprintln!("usage: monty-cpython (--subprocess | --connect <ws-url> | --listen <addr>)");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// Runs the session loop over an arbitrary transport. Exposed for tests, which
/// drive the worker over an in-memory transport.
#[must_use]
pub fn run_with_transport(transport: Box<dyn Transport>) -> ExitCode {
    let shared: SharedTransport = Rc::new(RefCell::new(transport));
    Python::attach(|py| match Session::new(py, shared) {
        Ok(mut session) => session.run(py),
        Err(err) => {
            eprintln!("monty-cpython: failed to initialize: {err}");
            ExitCode::from(EXIT_INIT)
        }
    })
}
