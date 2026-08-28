use std::{
    borrow::Cow,
    env, fs,
    path::{Path, PathBuf},
};

// `cargo_version_to_pep440`, shared verbatim with the library so that the
// version and pins written below always match the version maturin builds the
// wheels under. Included textually because a build script cannot depend on its
// own crate.
include!("src/version.rs");

/// Build script that configures pyo3 and, when building from the monty
/// workspace, keeps the `pydantic-monty` metapackage's version and dependency
/// pins matching the Cargo workspace version.
///
/// Cargo sets `CARGO_PKG_VERSION` in the environment when executing build
/// scripts, so we use that as the single source of truth — the same approach
/// `crates/monty-js/build.rs` takes for package.json.
fn main() {
    if let Some(path) = meta_pyproject_path() {
        // Re-run when either input to the metapackage rewrite changes.
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rerun-if-changed=src/version.rs");
        sync_meta_pyproject(&path);
    }
    // see https://pyo3.rs/main/building-and-distribution/multiple-python-versions.html
    pyo3_build_config::use_pyo3_cfgs();
}

/// Rewrite the metapackage's `version` and its pins on this distribution and
/// `pydantic-monty-runtime` if any has drifted from the Cargo package version.
///
/// All three distributions are built from the same workspace and released
/// together — `pydantic-monty` exists only to install the other two, and the
/// worker rejects a client whose version differs — so the pins must be exact.
/// Unlike monty-js there is no lockfile to refresh afterwards: uv.lock records
/// no specifier for workspace-editable sources.
///
/// Uses the runtime `CARGO_PKG_VERSION` env var (not `env!()`) so that the build
/// script picks up version changes without needing to be recompiled.
fn sync_meta_pyproject(path: &Path) {
    let cargo_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION not set");
    let pin_version = cargo_version_to_pep440(&cargo_version);

    let contents = fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    let mut result = String::with_capacity(contents.len());
    let mut changed = false;
    // one count per entry of `PINNED_DISTS`, plus the `version = ` line
    let mut pins = [0usize; PINNED_DISTS.len()];
    let mut versions = 0usize;

    for line in contents.lines() {
        let synced = if let Some(index) = pinned_dist(line) {
            pins[index] += 1;
            // Preserve the presence/absence of the trailing comma (the last
            // entry in the dependencies array has none).
            let comma = if line.ends_with(',') { "," } else { "" };
            Cow::Owned(format!("    \"{}=={pin_version}\"{comma}", PINNED_DISTS[index]))
        } else if line.starts_with("version = \"") {
            versions += 1;
            Cow::Owned(format!("version = \"{pin_version}\""))
        } else {
            Cow::Borrowed(line)
        };
        if synced != line {
            changed = true;
        }
        result.push_str(&synced);
        result.push('\n');
    }

    // Anything that stops a pin from being recognised — renaming it, adding
    // extras or an environment marker, dropping it — must fail the build rather
    // than silently ship a stale or absent pin. The same goes for the version,
    // which would otherwise leave the metapackage frozen at an old release.
    for (dist, found) in PINNED_DISTS.iter().zip(pins) {
        assert_eq!(
            found,
            1,
            "expected exactly one `{dist}` pin in [project].dependencies of {}, found {found}",
            path.display()
        );
    }
    assert_eq!(
        versions,
        1,
        "expected exactly one `version = \"...\"` line in {}, found {versions}",
        path.display()
    );

    if changed {
        eprintln!("Updating {} to {pin_version}", path.display());
        fs::write(path, &result).unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
    }
}

/// The PyPI distributions the metapackage pins exactly; see [`sync_meta_pyproject`].
const PINNED_DISTS: [&str; 2] = ["pydantic-monty-client", "pydantic-monty-runtime"];

/// The metapackage's pyproject.toml, which owns the `pydantic-monty` name on
/// PyPI, or `None` when there is nothing to sync.
///
/// The metapackage has no Cargo package of its own to carry the version, so
/// this build script — the one belonging to the distribution it pins —
/// maintains it. It lives outside this crate, and maturin's sdist ships only
/// the crate and its Rust path dependencies, so a source install legitimately
/// has no metapackage and must not panic over that.
fn meta_pyproject_path() -> Option<PathBuf> {
    let path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"))
        .join("../../packages/pydantic-monty/pyproject.toml");
    if path.is_file() {
        Some(path)
    } else {
        println!("cargo:warning=no file at {}, skipping version sync", path.display());
        None
    }
}

/// The index in [`PINNED_DISTS`] of the distribution `line` pins, if any.
///
/// The rewrite replaces the whole line, so this must recognise *only* what the
/// rewrite can reproduce: the bare name plus an optional version specifier. A
/// requirement carrying anything else — extras, an environment marker, a direct
/// URL — is deliberately not matched, so the `pins == 1` assertion fires rather
/// than the extra syntax being silently dropped.
fn pinned_dist(line: &str) -> Option<usize> {
    let requirement = dependency_requirement(line)?;
    PINNED_DISTS
        .iter()
        .position(|dist| requirement.strip_prefix(dist).is_some_and(is_version_specifier))
}

/// The PEP 508 requirement of a `[project].dependencies` array entry.
///
/// Matching is indentation-sensitive (exactly 4 spaces, the array style used in
/// this file) so that the bare `pydantic-monty-client = { workspace = true }`
/// entries under `[tool.uv.sources]`, which must stay unpinned, are never
/// touched. Only a trailing comma may follow the closing quote — the last entry
/// has none.
fn dependency_requirement(line: &str) -> Option<&str> {
    let entry = line.strip_prefix("    \"")?;
    let (requirement, tail) = entry.split_once('"')?;
    matches!(tail, "" | ",").then_some(requirement)
}

/// Whether `specifier` is what may legally follow a [`PINNED_DISTS`] name in a
/// pin we are willing to rewrite: nothing at all, or a PEP 440 version specifier.
///
/// The leading-character check also enforces the name boundary. `-` is a legal
/// PEP 508 name character, so accepting any suffix would claim (and clobber) a
/// future sibling dependency such as `pydantic-monty-runtime-stubs`. `;` starts
/// an environment marker, which the rewrite cannot preserve.
fn is_version_specifier(specifier: &str) -> bool {
    specifier.is_empty() || (specifier.starts_with(['=', '<', '>', '!', '~']) && !specifier.contains(';'))
}
