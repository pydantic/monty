use std::{borrow::Cow, env, fs, path::Path};

// `cargo_version_to_pep440`, shared verbatim with the library so that the pin
// written below always matches the version maturin builds the wheels under.
// Included textually because a build script cannot depend on its own crate.
include!("src/version.rs");

/// Build script that configures pyo3 and keeps pyproject.toml's
/// `pydantic-monty-runtime` pin exactly matching the Cargo workspace version.
///
/// Cargo sets `CARGO_PKG_VERSION` in the environment when executing build
/// scripts, so we use that as the single source of truth — the same approach
/// `crates/monty-js/build.rs` takes for package.json.
fn main() {
    // Re-run when either input to the pin changes.
    println!("cargo:rerun-if-changed=pyproject.toml");
    println!("cargo:rerun-if-changed=src/version.rs");
    sync_runtime_pin();
    // see https://pyo3.rs/main/building-and-distribution/multiple-python-versions.html
    pyo3_build_config::use_pyo3_cfgs();
}

/// Rewrite the `pydantic-monty-runtime` dependency pin in pyproject.toml if it
/// has drifted from the Cargo package version.
///
/// The two distributions are built from the same workspace and released
/// together, and `pydantic_monty` spawns the `monty` binary that
/// `pydantic-monty-runtime` ships, so the pin must be exact. Unlike monty-js
/// there is no lockfile to refresh afterwards: uv.lock records no specifier for
/// the workspace-editable `pydantic-monty-runtime` source.
///
/// Uses the runtime `CARGO_PKG_VERSION` env var (not `env!()`) so that the build
/// script picks up version changes without needing to be recompiled.
fn sync_runtime_pin() {
    let cargo_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION not set");
    let pin_version = cargo_version_to_pep440(&cargo_version);
    let pyproject_path = Path::new("pyproject.toml");

    let contents = fs::read_to_string(pyproject_path).expect("failed to read pyproject.toml");

    let mut result = String::with_capacity(contents.len());
    let mut changed = false;

    for line in contents.lines() {
        let synced = sync_line(line, &pin_version);
        if synced != line {
            changed = true;
        }
        result.push_str(&synced);
        result.push('\n');
    }

    if changed {
        eprintln!("Updating pydantic-monty-runtime pin in pyproject.toml to {pin_version}");
        fs::write(pyproject_path, &result).expect("failed to write pyproject.toml");
    }
}

/// Rewrite `line` with `version` if it is the `pydantic-monty-runtime` pin in
/// `[project].dependencies`. All other lines pass through unchanged.
///
/// Matching is indentation-sensitive (exactly 4 spaces, the array style used in
/// this file) so that the bare `pydantic-monty-runtime = { workspace = true }`
/// entry under `[tool.uv.sources]`, which must stay unpinned, is never touched.
fn sync_line<'a>(line: &'a str, version: &str) -> Cow<'a, str> {
    if line.starts_with("    \"pydantic-monty-runtime") {
        // Preserve the presence/absence of the trailing comma (the last entry
        // in the dependencies array has none).
        let comma = if line.ends_with(',') { "," } else { "" };
        Cow::Owned(format!("    \"pydantic-monty-runtime=={version}\"{comma}"))
    } else {
        Cow::Borrowed(line)
    }
}
