//! Integration tests that verify unsound `HeapReader` usage patterns are rejected at compile time.
//!
//! Each test invokes `cargo rustc` with specific `cfg` flags that enable known-bad code
//! inside `crates/monty/tests/heap_reader_compile_fail_cases/cases.rs`, then asserts that
//! compilation fails with the expected borrow-checker error stored in an `insta` snapshot.
//!
//! Using `cargo rustc -p monty` instead of `cargo check` with `RUSTFLAGS` ensures the cfg
//! flags are only passed to the monty crate itself, not to its dependencies. This avoids
//! unnecessary recompilation of the dependency tree between test runs.
//!
//! The invocations also use a dedicated `--target-dir` (see [`isolated_target_dir`]) so they
//! never share the `check`-profile cache with `cargo clippy`, which would otherwise force a
//! full rebuild here on every `make main`.
//!
//! This approach is necessary because the `HeapReader` types are `pub(crate)`, so standard
//! compile-fail test frameworks (like `trybuild`) cannot access them from integration tests.
//!
//! ## Updating expected output
//!
//! When the compiler output changes (e.g., after modifying the test cases or upgrading rustc),
//! run `INSTA_UPDATE=always cargo test -p monty --test heap_reader_compile_fail`
//! and review the updated external snapshots.

// Gated to the default feature config, and off Windows.
//
// Every case shells out to `cargo rustc -p monty` without forwarding the
// features of the crate under test, so `memory-model-checks` and
// `ref-count-return` produce byte-identical diagnostics — they only repeat the
// cost. That cost is not small: each case compiles `monty` under a different
// `--cfg`, so they evict each other from the shared target dir and re-run the
// whole set. CI runs all three configs, so without this gate it pays for the
// same compilations three times.
//
// Windows is excluded because rustc writes backslashes in diagnostic paths
// (`crates\monty\src\..`) while the snapshots use forward slashes. The
// borrow-checker guarantees under test are platform-independent.
#![cfg(not(any(target_os = "windows", feature = "memory-model-checks", feature = "ref-count-return")))]

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use insta::assert_snapshot;

/// Dedicated target directory for the `cargo rustc` invocations below.
///
/// These run under the `check` profile, which `cargo clippy` also uses — but
/// clippy builds via `clippy-driver`, a different cargo fingerprint. Sharing
/// the main target dir means clippy and these tests mutually invalidate the
/// check-profile cache, so every `make main` (clippy → tests) rebuilds the
/// whole dependency tree *and* monty here from scratch. Isolating them lets the
/// dependency tree build once and stay cached across runs, independent of
/// clippy. The dir lives under `target/`, so it is gitignored and removed by
/// `cargo clean`.
fn isolated_target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("heap_reader_compile_fail")
}

/// Extracts just the error diagnostics from rustc stderr, filtering out warnings,
/// progress lines, and other noise that varies between runs.
fn normalize_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| {
            !line.starts_with("warning:")
                && !line.starts_with("   Compiling")
                && !line.starts_with("    Checking")
                && !line.starts_with("    Finished")
                && !line.starts_with("    Blocking")
                && !line.starts_with("error: could not compile")
                && !line.starts_with("warning: build failed")
                && !line.starts_with("error: process didn't exit successfully:")
                && !line.is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs `cargo rustc -p monty` with the given cfg flag and asserts that:
/// 1. Compilation fails (non-zero exit code)
/// 2. The normalized error output matches the corresponding snapshot
///
/// Using `cargo rustc --profile=check` instead of `cargo check` with `RUSTFLAGS` passes
/// the cfg flags only to the monty crate, so dependencies are compiled once and cached
/// across test runs.
fn check_compile_fail(test_name: &str) {
    let test_cfg = format!("heap_reader_compile_fail_test_{test_name}");

    let target_dir = isolated_target_dir();
    let output = Command::new(env!("CARGO"))
        .args([
            "rustc",
            "--package=monty",
            "--profile=check",
            "--target-dir",
            &target_dir.to_string_lossy(),
            "--",
            "--cfg=heap_reader_compile_fail_tests",
            "--cfg",
            &test_cfg,
            "--diagnostic-width=140",
        ])
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("failed to run cargo rustc");

    assert!(
        !output.status.success(),
        "{test_name}: expected compilation to fail, but it succeeded",
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let actual = normalize_stderr(&stderr);

    assert_snapshot!(test_name, actual);
}

#[test]
fn heap_mutation_while_reading() {
    check_compile_fail("heap_mutation_while_reading");
}

#[test]
fn double_get_mut() {
    check_compile_fail("double_get_mut");
}

#[test]
fn dec_ref_while_reading() {
    check_compile_fail("dec_ref_while_reading");
}

#[test]
fn smuggle_heap_read() {
    check_compile_fail("smuggle_heap_read");
}

#[test]
fn mutation_in_map_closure() {
    check_compile_fail("mutation_in_map_closure");
}

#[test]
fn smuggle_vm() {
    check_compile_fail("smuggle_vm");
}

#[test]
fn smuggle_and_swap_reader() {
    check_compile_fail("smuggle_and_swap_reader");
}

/// Protected shared access cannot outlive its source, even after deref coercion.
#[test]
fn protected_get_escape() {
    check_compile_fail("protected_get_escape");
}

/// Shared access through a mutable wrapper preserves the source lifetime.
#[test]
fn protected_mut_get_escape() {
    check_compile_fail("protected_mut_get_escape");
}

/// Protected mutable access cannot outlive its source, even after deref coercion.
#[test]
fn protected_get_mut_escape() {
    check_compile_fail("protected_get_mut_escape");
}

/// Protected shared access keeps the source immutably borrowed.
#[test]
fn protected_get_alias() {
    check_compile_fail("protected_get_alias");
}

/// Shared access through a mutable wrapper keeps the source borrowed.
#[test]
fn protected_mut_get_alias() {
    check_compile_fail("protected_mut_get_alias");
}

/// Protected mutable access keeps the source exclusively borrowed.
#[test]
fn protected_get_mut_alias() {
    check_compile_fail("protected_get_mut_alias");
}

/// Field projections preserve the original source's lifetime.
#[test]
fn projected_get_escape() {
    check_compile_fail("projected_get_escape");
}

/// Slice projections preserve the vector's lifetime.
#[test]
fn slice_get_escape() {
    check_compile_fail("slice_get_escape");
}

/// Box projections preserve the allocation's lifetime.
#[test]
fn box_get_escape() {
    check_compile_fail("box_get_escape");
}

/// Mutable capabilities cannot be swapped independently of their owners.
#[test]
fn mutable_inner_swap() {
    check_compile_fail("mutable_inner_swap");
}

/// Read-only capabilities cannot be swapped independently of their owners.
#[test]
fn shared_inner_swap() {
    check_compile_fail("shared_inner_swap");
}

/// Counted and uncounted capabilities cannot be exchanged.
#[test]
fn counted_inner_swap() {
    check_compile_fail("counted_inner_swap");
}

/// Shared source borrows expose no mutable capability.
#[test]
fn readonly_mutation() {
    check_compile_fail("readonly_mutation");
}

/// Field projection cannot upgrade a read-only capability.
#[test]
fn readonly_mutable_projection() {
    check_compile_fail("readonly_mutable_projection");
}

/// Capability aliases cannot overlap their payload borrows.
#[test]
fn aliased_payload_borrows() {
    check_compile_fail("aliased_payload_borrows");
}

/// Mutable field projections preserve their source lifetime.
#[test]
fn projected_get_mut_escape() {
    check_compile_fail("projected_get_mut_escape");
}

/// A live projected reference excludes parent mutation.
#[test]
fn projection_live_borrow() {
    check_compile_fail("projection_live_borrow");
}
