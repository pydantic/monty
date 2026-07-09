#![doc = include_str!("../README.md")]

//! # Crate layout
//!
//! Transports are pluggable ([`Transport`]) and chosen by subcommand; see
//! [`pyexec`] for the `dict.__missing__` mechanism that routes undefined
//! names back to the parent.

mod events;
mod install;
mod pyexec;
mod session;
mod traceback;
// `pep_723` and `transport` are `pub` only because the integration tests in
// `tests/` are separate crates and reach them directly (`pep_723::dependencies`,
// the `transport::Transport` trait an in-memory test parent implements). The
// rest of the worker's modules are crate-internal.
pub mod pep_723;
pub mod transport;

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use clap::{Parser, Subcommand};
use pyo3::prelude::*;

use crate::{
    session::Session,
    transport::{SharedTransport, StdioTransport, Transport, connect},
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
    Websocket {
        /// The `ws://`/`wss://` URL to dial.
        url: String,
    },
}

/// Parses the CLI and runs the worker over the selected transport.
#[must_use]
pub fn run() -> ExitCode {
    match Cli::parse().transport {
        TransportArg::Subprocess => run_with_transport(Box::new(StdioTransport::new())),
        TransportArg::Websocket { url } => match connect(&url) {
            Ok(transport) => run_with_transport(Box::new(transport)),
            Err(err) => {
                eprintln!("monty-cpython: failed to connect to {url}: {err}");
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
