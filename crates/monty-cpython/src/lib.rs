//! `monty-cpython`: a Monty wire-protocol child worker that runs fed code in
//! embedded CPython.
//!
//! It speaks the same protocol as `monty subprocess` (so the existing
//! `monty-pool` can drive it) but, instead of running Monty, executes each
//! snippet in a real CPython interpreter and routes every undefined name back to
//! the parent as a `FunctionCall` — the Rust port of the `proto_child.py`
//! reference client. See [`pyexec`] for the `dict.__missing__` mechanism.
//!
//! Transports are pluggable ([`Transport`]) and chosen by subcommand: the
//! `subprocess` subcommand uses stdio framed like the Monty child, while
//! `connect`/`server` use WebSocket (dial a relay/parent, or accept one parent).
//!
//! SECURITY: full CPython is **not** a sandbox. This worker runs untrusted code
//! with no isolation of its own — that is the deployment's responsibility (a
//! locked-down container, or a relay-provisioned sandbox). See this crate's
//! `README.md`.

mod events;
pub mod pyexec;
pub mod session;
pub mod transport;

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use clap::{Parser, Subcommand};
use pyo3::prelude::*;

use crate::{
    session::Session,
    transport::{SharedTransport, StdioTransport, Transport, connect, listen},
};

/// Exit code for a failure to initialize the embedded interpreter.
const EXIT_INIT: u8 = 70;
/// Exit code for a transport that could not be established (connect/bind).
const EXIT_TRANSPORT: u8 = 69;

/// CLI for the embedded-CPython worker. The transport is chosen by subcommand;
/// `subprocess` mirrors `monty subprocess` so the same `monty-pool` spawn path
/// drives this binary as a drop-in worker.
#[derive(Parser)]
#[command(
    version,
    about = "Monty wire-protocol child worker running fed code in embedded CPython"
)]
struct Cli {
    #[command(subcommand)]
    transport: TransportArg,
}

/// The transport this worker speaks to its parent over.
#[derive(Subcommand)]
enum TransportArg {
    /// Run as a framed-stdio child, a drop-in worker for `monty-pool`.
    Subprocess,
    /// Dial a relay (or a parent-as-server) as a WebSocket client.
    Connect {
        /// The `ws://`/`wss://` URL to dial.
        url: String,
    },
    /// Bind an address and accept one parent connection (server mode).
    Server {
        /// The `host:port` to bind and accept on.
        addr: String,
    },
}

/// Parses the CLI and runs the worker over the selected transport.
#[must_use]
pub fn run() -> ExitCode {
    match Cli::parse().transport {
        TransportArg::Subprocess => run_with_transport(Box::new(StdioTransport::new())),
        TransportArg::Connect { url } => match connect(&url) {
            Ok(transport) => run_with_transport(Box::new(transport)),
            Err(err) => {
                eprintln!("monty-cpython: failed to connect to {url}: {err}");
                ExitCode::from(EXIT_TRANSPORT)
            }
        },
        TransportArg::Server { addr } => match listen(&addr) {
            Ok(transport) => run_with_transport(Box::new(transport)),
            Err(err) => {
                eprintln!("monty-cpython: failed to listen on {addr}: {err}");
                ExitCode::from(EXIT_TRANSPORT)
            }
        },
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
