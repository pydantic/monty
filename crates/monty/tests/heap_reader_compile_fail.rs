//! Integration tests that verify unsound `HeapReader` usage patterns are rejected at compile time.
//!
//! Each test invokes `cargo rustc` with specific `cfg` flags that enable known-bad code
//! inside `crates/monty/tests/heap_reader_compile_fail_cases/cases.rs`, then asserts that
//! compilation fails with the expected borrow-checker error stored in a `.stderr` file.
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
//! run with `UPDATE_EXPECT=1` to overwrite the `.stderr` files with the actual output:
//!
//! ```sh
//! UPDATE_EXPECT=1 cargo test -p monty --test heap_reader_compile_fail
//! ```

// Gated to the default feature config, and off Windows.
//
// Every case shells out to `cargo rustc -p monty` without forwarding the
// features of the crate under test, so `memory-model-checks` and
// `ref-count-return` produce byte-identical diagnostics — they only repeat the
// cost. That cost is not small: each case compiles `monty` under a different
// `--cfg`, so the seven of them evict each other from the shared target dir and
// re-run the whole set (~50s, ~65s cold). CI runs all three configs, so without
// this gate it pays for the same seven compilations three times.
//
// Windows is excluded because rustc writes backslashes in diagnostic paths
// (`crates\monty\src\..`) while the `.stderr` files use forward slashes. The
// borrow-checker guarantees under test are platform-independent.
#![cfg(not(any(target_os = "windows", feature = "memory-model-checks", feature = "ref-count-return")))]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Directory containing the compile-fail test cases and `.stderr` expectation files.
fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("heap_reader_compile_fail_cases")
}

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
/// 2. The normalized error output matches the corresponding `.stderr` file
///
/// Using `cargo rustc --profile=check` instead of `cargo check` with `RUSTFLAGS` passes
/// the cfg flags only to the monty crate, so dependencies are compiled once and cached
/// across test runs.
///
/// When `UPDATE_EXPECT=1` is set, overwrites the `.stderr` file instead of asserting.
fn check_compile_fail(test_name: &str) {
    let test_cfg = format!("heap_reader_compile_fail_test_{test_name}");
    let stderr_path = cases_dir().join(format!("{test_name}.stderr"));

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

    if env::var("UPDATE_EXPECT").is_ok() {
        fs::write(&stderr_path, format!("{actual}\n"))
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", stderr_path.display()));
        eprintln!("updated {}", stderr_path.display());
        return;
    }

    let expected = fs::read_to_string(&stderr_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", stderr_path.display()))
        .trim()
        .to_owned();

    assert!(
        actual == expected,
        "{test_name}: stderr mismatch (run with UPDATE_EXPECT=1 to update)\n\n--- expected ({}) ---\n{expected}\n\n--- actual ---\n{actual}\n",
        stderr_path.display(),
    );
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
