use std::{env, path::PathBuf, process::Command};

/// Builds binary target `name` with the running test's Cargo profile and returns its path.
///
/// `args` selects the package (e.g. `["-p", "monty-runtime"]`).
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

/// Returns the Cargo profile the running test binary was built with.
///
/// The profile directory is the parent of `deps` (`<profile>/deps/<exe>`, Cargo before 1.100)
/// or of `build` (`<profile>/build/<pkg>/<hash>/out/<exe>`, Cargo 1.100's build-dir layout).
fn test_profile() -> String {
    let executable = env::current_exe().unwrap();
    let profile = executable
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "deps" || name == "build"))
        .and_then(|dir| dir.parent()?.file_name()?.to_str())
        .unwrap_or_else(|| panic!("no profile directory above {}", executable.display()));
    if profile == "debug" {
        "dev".to_owned()
    } else {
        profile.to_owned()
    }
}
