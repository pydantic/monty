use std::{borrow::Cow, env, fs, path::Path, process::Command};

/// Build script that sets up napi bindings and syncs package.json's version
/// fields with the Cargo workspace version.
///
/// Cargo sets `CARGO_PKG_VERSION` in the environment when executing build scripts,
/// so we use that as the single source of truth. If package.json's top-level
/// `version` or any `@pydantic/monty-*` platform pin in `optionalDependencies`
/// differs, we update it in place (CI's create-platform-packages fails if the
/// pins drift from the package version).
fn main() {
    // Re-run when package.json changes so we can re-check the versions.
    println!("cargo:rerun-if-changed=package.json");
    sync_package_json_version();
    napi_build::setup();
}

/// Read the Cargo package version and update package.json if any version-bearing
/// line differs, then refresh package-lock.json to match.
///
/// Uses the runtime `CARGO_PKG_VERSION` env var (not `env!()`) so that the build
/// script picks up version changes without needing to be recompiled.
fn sync_package_json_version() {
    let cargo_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION not set");
    let package_json_path = Path::new("package.json");

    let contents = fs::read_to_string(package_json_path).expect("failed to read package.json");

    let mut result = String::with_capacity(contents.len());
    let mut changed = false;

    for line in contents.lines() {
        let synced = sync_line(line, &cargo_version);
        if synced != line {
            changed = true;
        }
        result.push_str(&synced);
        result.push('\n');
    }

    if !changed {
        return;
    }

    eprintln!("Updating package.json versions to {cargo_version}");
    fs::write(package_json_path, &result).expect("failed to write package.json");

    // Sync package-lock.json to match the updated versions.
    let status = Command::new("npm")
        .args(["install", "--package-lock-only"])
        .status()
        .expect("failed to run npm");
    assert!(status.success(), "npm install --package-lock-only failed");
}

/// Rewrite `line` with `version` if it is a version-bearing line: the top-level
/// `"version"` field or a `@pydantic/monty-*` platform pin in
/// `optionalDependencies`. All other lines pass through unchanged.
///
/// Matching is indentation-sensitive (prettier-formatted, 2-space indent per
/// level): exactly 2 spaces for the top-level field — so nested `version` keys
/// don't match — and exactly 4 for the platform pins.
fn sync_line<'a>(line: &'a str, version: &str) -> Cow<'a, str> {
    if line.starts_with("  \"version\"") {
        Cow::Owned(format!("  \"version\": \"{version}\","))
    } else if let Some(name) = line
        .strip_prefix("    \"@pydantic/monty-")
        .and_then(|rest| rest.split('"').next())
    {
        // Preserve the presence/absence of the trailing comma (the last entry
        // in optionalDependencies has none).
        let comma = if line.ends_with(',') { "," } else { "" };
        Cow::Owned(format!("    \"@pydantic/monty-{name}\": \"{version}\"{comma}"))
    } else {
        Cow::Borrowed(line)
    }
}
