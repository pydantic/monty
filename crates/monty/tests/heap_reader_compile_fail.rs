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

    let output = Command::new(env!("CARGO"))
        .args([
            "rustc",
            "--package=monty",
            "--profile=check",
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

// These tests are disabled on Windows because rustc uses backslashes in diagnostic
// paths (`crates\monty\src\..`) while the `.stderr` expectation files use forward
// slashes. The borrow-checker guarantees being tested are platform-independent.

#[test]
#[cfg(not(target_os = "windows"))]
fn heap_mutation_while_reading() {
    check_compile_fail("heap_mutation_while_reading");
}

#[test]
#[cfg(not(target_os = "windows"))]
fn double_get_mut() {
    check_compile_fail("double_get_mut");
}

#[test]
#[cfg(not(target_os = "windows"))]
fn dec_ref_while_reading() {
    check_compile_fail("dec_ref_while_reading");
}

#[test]
#[cfg(not(target_os = "windows"))]
fn smuggle_heap_read() {
    check_compile_fail("smuggle_heap_read");
}

#[test]
#[cfg(not(target_os = "windows"))]
fn mutation_in_map_closure() {
    check_compile_fail("mutation_in_map_closure");
}
