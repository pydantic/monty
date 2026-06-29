//! Entry point for the `monty-cpython` child worker. All logic lives in the
//! library crate so it can be driven over an in-memory transport in tests.

use std::process::ExitCode;

use rustls::crypto::aws_lc_rs::default_provider;

fn main() -> ExitCode {
    // rustls 0.23 panics on first TLS use if it can't pick a `CryptoProvider`
    // automatically — happens whenever the dep tree compiles in both
    // `aws-lc-rs` and `ring`, or neither. Install one explicitly *before*
    // any tungstenite `wss://` dial in the `websocket` subcommand.
    default_provider()
        .install_default()
        .expect("install rustls aws_lc_rs CryptoProvider");

    monty_cpython::run()
}
