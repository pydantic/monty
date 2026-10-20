//! Compile-fail checks of reader borrows and generative owner isolation.
//! These compile inside the library to exercise its private interpreter APIs.

#![cfg(not(any(target_os = "windows", feature = "memory-model-checks", feature = "ref-count-return")))]

use std::{env, path::PathBuf, process::Command};

use insta::assert_snapshot;
use serde_json::Value;

/// Avoids invalidating Clippy's cache or writing build products into a mounted source tree.
fn isolated_target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
            PathBuf::from,
        )
        .join("heap_reader_compile_fail")
}

/// Snapshots only compiler errors; warnings and Cargo progress are not part of the contract.
fn check_compile_fail(name: &str, cfg: &str) {
    let output = Command::new(env!("CARGO"))
        .args([
            "rustc",
            "--package=monty",
            "--profile=check",
            "--message-format=json",
            "--target-dir",
            &isolated_target_dir().to_string_lossy(),
            "--",
            "--cfg=heap_reader_compile_fail_tests",
            "--cfg",
            cfg,
            "--diagnostic-width=140",
        ])
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("failed to run cargo rustc");
    assert!(!output.status.success(), "{name}: expected compilation to fail");
    let diagnostics = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-message" && message["message"]["level"] == "error")
        .map(|message| message["message"]["rendered"].as_str().unwrap().trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n\n");
    assert!(
        !diagnostics.is_empty(),
        "{name}: no compiler errors: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_snapshot!(name, diagnostics);
}

/// Reader scopes remain invariant independently of arena brands.
#[test]
fn reader_boundaries() {
    for name in [
        "heap_mutation_while_reading",
        "double_get_mut",
        "dec_ref_while_reading",
        "smuggle_heap_read",
        "mutation_in_map_closure",
        "smuggle_vm",
        "smuggle_and_swap_reader",
    ] {
        check_compile_fail(name, &format!("heap_reader_compile_fail_test_{name}"));
    }
}

/// Protected and projected capabilities retain their source lifetime and access permissions.
#[test]
fn borrowed_boundaries() {
    for name in [
        "protected_get_escape",
        "protected_mut_get_escape",
        "protected_get_mut_escape",
        "protected_get_alias",
        "protected_mut_get_alias",
        "protected_get_mut_alias",
        "projected_get_escape",
        "slice_get_escape",
        "box_get_escape",
        "mutable_inner_swap",
        "shared_inner_swap",
        "counted_inner_swap",
        "readonly_mutation",
        "readonly_mutable_projection",
        "aliased_payload_borrows",
        "projected_get_mut_escape",
        "projection_live_borrow",
    ] {
        check_compile_fail(name, &format!("heap_reader_compile_fail_test_{name}"));
    }
}

/// Covers escaping roots, independent arenas, persistence, constants and thread capabilities.
#[test]
fn owner_boundaries() {
    for name in [
        "escape_pointer",
        "retain_root",
        "foreign_read",
        "foreign_root",
        "swap_heaps",
        "swap_states",
        "manufacture_heap",
        "deserialize_pointer",
        "deserialize_heap",
        "indirect_constant",
        "send_pointer",
        "send_view",
        "sync_owner",
    ] {
        check_compile_fail(
            &format!("owner_{name}"),
            &format!("heap_owner_compile_fail = \"{name}\""),
        );
    }
}
