use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

/// Builds a binary with the same cargo profile as the running test and returns its path.
pub fn binary(args: &[&str], name: &str) -> PathBuf {
    let profile = test_profile();
    let output = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["build", "--profile", &profile, "--message-format=json"])
        .args(args)
        .output()
        .expect("build the test binary");
    assert!(
        output.status.success(),
        "Cargo build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    let path = output
        .lines()
        .find_map(|line| {
            let event: serde_json::Value = serde_json::from_str(line).expect("Cargo JSON output");
            if event["reason"] == "compiler-artifact" && event["target"]["name"] == name {
                event["executable"].as_str().map(PathBuf::from)
            } else {
                None
            }
        })
        .expect("Cargo did not report the test binary");
    assert!(path.is_file(), "test binary missing at {}", path.display());
    path
}

/// The cargo profile the running test was built with, read from its executable's location.
///
/// Test executables live in `<profile-dir>/deps/` before Cargo 1.100 and in
/// `<profile-dir>/build/<pkg>/<hash>/out/` from it, and a fresh 1.100 target has no `deps`
/// directory, so the profile directory is the parent of the outermost `deps` or `build` ancestor.
fn test_profile() -> String {
    let executable = env::current_exe().unwrap();
    let target_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).parent().unwrap();
    let profile_dir = executable
        .ancestors()
        .filter(|dir| {
            dir.starts_with(target_dir) && matches!(dir.file_name().and_then(|n| n.to_str()), Some("deps" | "build"))
        })
        .last()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("no profile directory above {}", executable.display()));
    match profile_dir.file_name().unwrap().to_str().unwrap() {
        "debug" => "dev".to_owned(),
        profile => profile.to_owned(),
    }
}
