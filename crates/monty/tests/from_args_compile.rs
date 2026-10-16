//! Compile generic `FromArgs` expansions against the real interpreter contracts.

#![cfg(not(any(feature = "memory-model-checks", feature = "ref-count-return")))]

use std::{env, path::PathBuf, process::Command};

/// Keeps the compile-pass cache separate from Clippy and the negative UI cases.
fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
            PathBuf::from,
        )
        .join("from_args_compile")
}

/// Preserving generic syntax is insufficient: the generated cleanup implementation must type-check too.
#[test]
fn generic_arguments_compile() {
    let output = Command::new(env!("CARGO"))
        .args(["rustc", "--package=monty", "--profile=check", "--target-dir"])
        .arg(target_dir())
        .args(["--", "--cfg=heap_reader_compile_fail_tests"])
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("run generic argument compile control");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}
