use std::{env, path::PathBuf, process::Command, sync::Once};

/// Locates (building once if needed) the `monty` CLI binary for tests.
pub fn monty_binary() -> PathBuf {
    static BUILD: Once = Once::new();
    if let Ok(path) = env::var("MONTY_TEST_BIN") {
        return PathBuf::from(path);
    }
    let executable = env::current_exe().expect("test executable path");
    let profile_dir = executable.parent().unwrap().parent().unwrap();
    let profile = profile_dir.file_name().unwrap().to_str().unwrap();
    let profile = if profile == "debug" { "dev" } else { profile };
    let path = profile_dir.join(format!("monty{}", env::consts::EXE_SUFFIX));
    BUILD.call_once(|| {
        if !path.exists() {
            let status = Command::new(env!("CARGO"))
                .args(["build", "-p", "monty-runtime", "--profile", profile, "--target-dir"])
                .arg(profile_dir.parent().unwrap())
                .status()
                .expect("failed to run cargo build -p monty-runtime");
            assert!(status.success(), "building the monty binary failed");
        }
    });
    assert!(path.exists(), "monty binary missing at {}", path.display());
    path
}
