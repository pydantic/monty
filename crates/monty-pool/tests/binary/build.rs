use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

pub fn binary(args: &[&str], name: &str) -> PathBuf {
    // The test executable sits somewhere under `<target>/<profile>/`; how deep depends on
    // Cargo's build-dir layout, so take the first component below the target directory.
    let executable = env::current_exe().unwrap();
    let target_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).parent().unwrap();
    let profile_dir = executable
        .strip_prefix(target_dir)
        .unwrap()
        .components()
        .next()
        .unwrap();
    let profile = profile_dir.as_os_str().to_str().unwrap();
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
