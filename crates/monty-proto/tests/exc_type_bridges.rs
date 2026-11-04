//! Keeps host exception-name mirrors aligned with `ExcType`.

use std::{fs, path::PathBuf};

use monty_types::ExcType;
use strum::VariantNames;

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

fn read_source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Parse quoted names between exact markers, failing if either marker is missing.
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

// Binding lists group exceptions by hierarchy, so compare sorted names.
fn sorted_variants() -> Vec<String> {
    let mut variants: Vec<String> = ExcType::VARIANTS.iter().map(|&s| s.to_owned()).collect();
    variants.sort();
    variants
}
