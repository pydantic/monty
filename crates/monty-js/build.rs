use std::{env, fs, path::Path};

/// Build script that sets up napi bindings and syncs the package.json version
/// with the Cargo workspace version.
///
/// Cargo sets `CARGO_PKG_VERSION` in the environment when executing build scripts,
/// so we use that as the single source of truth. If package.json has a different
/// version, we update it in place.
fn main() {
    // Re-run when package.json changes so we can re-check the version.
    println!("cargo:rerun-if-changed=package.json");
    sync_package_json_version();
    napi_build::setup();
}

/// Read the Cargo package version and update package.json if the version differs.
///
/// Uses the runtime `CARGO_PKG_VERSION` env var (not `env!()`) so that the build
/// script picks up version changes without needing to be recompiled.
fn sync_package_json_version() {
    let cargo_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION not set");
    let package_json_path = Path::new("package.json");

    let contents = fs::read_to_string(package_json_path).expect("failed to read package.json");

    // Replace the top-level "version" field. We match lines starting with
    // `  "version":` which is the standard prettier-formatted location.
    let expected = format!("  \"version\": \"{cargo_version}\",");
    let mut result = String::with_capacity(contents.len());
    let mut changed = false;

    for line in contents.lines() {
        // Only match the top-level "version" field (exactly 2-space indent),
        // not nested ones like scripts.version (4-space indent).
        if !changed && line.starts_with("  \"version\"") && line != expected {
            result.push_str(&expected);
            changed = true;
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }

    if changed {
        fs::write(package_json_path, result).expect("failed to write package.json");
        eprintln!("Updated package.json version to {cargo_version}");
    }
}
