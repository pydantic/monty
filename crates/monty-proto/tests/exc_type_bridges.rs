//! Pins the host bridges' exception name lists to `ExcType`.
//!
//! Three lists have to agree for an exception to survive the crossing: Rust's
//! `ExcType`, `PYTHON_EXC_NAMES` in the TypeScript drive loop, and the `ExcType`
//! literal in `pydantic_monty`. The latter two are hand-written, and both fail
//! quietly when they drift — a missing name in the TS set downgrades that
//! exception to `RuntimeError` on the way in, and a missing name in the Python
//! literal still type-checks while being rejected at runtime.
//!
//! Reading the two sources here keeps the check out of the shipped bindings:
//! neither package needs to expose the name list just so a test can read it.

use std::{fs, path::PathBuf};

use monty_types::ExcType;
use strum::VariantNames;

/// The TS list the drive loop matches a thrown `Error.name` against.
#[test]
fn python_exc_names_matches_exc_type() {
    let names = names_between(
        &read_source("monty-js/ts/errors.ts"),
        "PYTHON_EXC_NAMES: ReadonlySet<string> = new Set([",
        "])",
    );
    assert_eq!(
        names,
        sorted_variants(),
        "PYTHON_EXC_NAMES in crates/monty-js/ts/errors.ts is out of sync with ExcType"
    );
}

/// The literal `ExternalExceptionData` names an exception by.
#[test]
fn python_exc_type_literal_matches_exc_type() {
    let names = names_between(
        &read_source("monty-python/python/pydantic_monty/__init__.py"),
        "ExcType = Literal[",
        "]",
    );
    assert_eq!(
        names,
        sorted_variants(),
        "the ExcType literal in crates/monty-python/python/pydantic_monty/__init__.py is out of sync with ExcType"
    );
}

/// Reads a workspace source file, relative to `crates/`.
fn read_source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Collects the quoted strings of the list literal delimited by `open`/`close`.
///
/// Panics rather than returning nothing when a delimiter is missing, so a
/// refactor that moves either list fails loudly instead of passing vacuously.
fn names_between(source: &str, open: &str, close: &str) -> Vec<String> {
    let after_open = source
        .split_once(open)
        .unwrap_or_else(|| panic!("no `{open}` in source"))
        .1;
    let body = after_open
        .split_once(close)
        .unwrap_or_else(|| panic!("no `{close}` closing `{open}`"))
        .0;
    let mut names: Vec<String> = body.split('\'').skip(1).step_by(2).map(ToOwned::to_owned).collect();
    assert!(!names.is_empty(), "no quoted names inside `{open}`");
    names.sort();
    names
}

/// `ExcType`'s names sorted, since both lists group by hierarchy for reading
/// rather than following the enum's declaration order.
fn sorted_variants() -> Vec<String> {
    let mut variants: Vec<String> = ExcType::VARIANTS.iter().map(|&s| s.to_owned()).collect();
    variants.sort();
    variants
}
