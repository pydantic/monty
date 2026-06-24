//! Entry point for the `monty-cpython` child worker. All logic lives in the
//! library crate so it can be driven over an in-memory transport in tests.

use std::process::ExitCode;

fn main() -> ExitCode {
    monty_cpython::run()
}
