use std::{borrow::Cow, env, fs, path::Path, process::Command};

/// Sets up napi bindings and syncs package.json's version with the Cargo workspace.
/// Platform dependency pins are added only when assembling release tarballs.
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

/// Rewrites only the top-level version, identified by its two-space JSON indentation.
fn sync_line<'a>(line: &'a str, version: &str) -> Cow<'a, str> {
    if line.starts_with("  \"version\"") {
        Cow::Owned(format!("  \"version\": \"{version}\","))
    } else {
        Cow::Borrowed(line)
    }
}
