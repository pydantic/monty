use std::{env, path::PathBuf, process::Command};

pub fn binary(args: &[&str], name: &str) -> PathBuf {
    let executable = env::current_exe().unwrap();
    let profile_dir = executable.parent().unwrap().parent().unwrap();
    let profile = profile_dir.file_name().unwrap().to_str().unwrap();
    let profile = if profile == "debug" { "dev" } else { profile };
    let output = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["build", "--profile", profile, "--message-format=json"])
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
