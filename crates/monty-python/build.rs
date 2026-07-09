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
    let mut pins = 0usize;

    for line in contents.lines() {
        let synced = if is_runtime_pin(line) {
            pins += 1;
            // Preserve the presence/absence of the trailing comma (the last
            // entry in the dependencies array has none).
            let comma = if line.ends_with(',') { "," } else { "" };
            Cow::Owned(format!("    \"{RUNTIME_DIST}=={pin_version}\"{comma}"))
        } else {
            Cow::Borrowed(line)
        };
        if synced != line {
            changed = true;
        }
        result.push_str(&synced);
        result.push('\n');
    }

    // Anything that stops the pin from being recognised — renaming it, adding
    // extras or an environment marker, dropping it — must fail the build rather
    // than silently ship a stale or absent pin.
    assert_eq!(
        pins, 1,
        "expected exactly one `{RUNTIME_DIST}` pin in [project].dependencies of pyproject.toml, found {pins}"
    );

    if changed {
        eprintln!("Updating {RUNTIME_DIST} pin in pyproject.toml to {pin_version}");
        fs::write(pyproject_path, &result).expect("failed to write pyproject.toml");
    }
}

/// The PyPI distribution this package pins exactly; see [`sync_runtime_pin`].
const RUNTIME_DIST: &str = "pydantic-monty-runtime";

/// Whether `line` is the [`RUNTIME_DIST`] entry in `[project].dependencies`.
///
/// The rewrite replaces the whole line, so this must recognise *only* what the
/// rewrite can reproduce: the bare name plus an optional version specifier. A
/// requirement carrying anything else — extras, an environment marker, a direct
/// URL — is deliberately not matched, so the `pins == 1` assertion fires rather
/// than the extra syntax being silently dropped.
fn is_runtime_pin(line: &str) -> bool {
    dependency_requirement(line)
        .and_then(|requirement| requirement.strip_prefix(RUNTIME_DIST))
        .is_some_and(is_version_specifier)
}

/// The PEP 508 requirement of a `[project].dependencies` array entry.
///
/// Matching is indentation-sensitive (exactly 4 spaces, the array style used in
/// this file) so that the bare `pydantic-monty-runtime = { workspace = true }`
/// entry under `[tool.uv.sources]`, which must stay unpinned, is never touched.
/// Only a trailing comma may follow the closing quote — the last entry has none.
fn dependency_requirement(line: &str) -> Option<&str> {
    let entry = line.strip_prefix("    \"")?;
    let (requirement, tail) = entry.split_once('"')?;
    matches!(tail, "" | ",").then_some(requirement)
}

/// Whether `specifier` is what may legally follow [`RUNTIME_DIST`] in a pin we
/// are willing to rewrite: nothing at all, or a PEP 440 version specifier.
///
/// The leading-character check also enforces the name boundary. `-` is a legal
/// PEP 508 name character, so accepting any suffix would claim (and clobber) a
/// future sibling dependency such as `pydantic-monty-runtime-stubs`. `;` starts
/// an environment marker, which the rewrite cannot preserve.
fn is_version_specifier(specifier: &str) -> bool {
    specifier.is_empty() || (specifier.starts_with(['=', '<', '>', '!', '~']) && !specifier.contains(';'))
}
