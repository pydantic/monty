use std::{fs, process::Command};

use tempfile::TempDir;

/// Runs the CLI binary and returns trimmed, ANSI-free stderr for exact assertions.
fn run_monty(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_monty"))
        .args(args)
        .output()
        .expect("monty CLI should run");

    (
        output.status.success(),
        strip_ansi(&String::from_utf8(output.stderr).expect("stderr should be valid UTF-8"))
            .trim()
            .to_owned(),
    )
}

/// Removes ANSI escape sequences so tests can assert on the actual error text.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }

        if chars.next_if_eq(&'[').is_none() {
            continue;
        }

        for next in chars.by_ref() {
            if next.is_ascii_alphabetic() {
                break;
            }
        }
    }

    out
}

/// Writes `code` to a temporary Python file and returns the containing temp dir.
fn script_dir(code: &str) -> TempDir {
    let dir = TempDir::new().expect("tempdir should be created");
    fs::write(dir.path().join("script.py"), code).expect("script should be written");
    dir
}

#[test]
fn unmounted_paths_report_permission_error() {
    let host_dir = TempDir::new().expect("tempdir should be created");
    let script_dir = script_dir("from pathlib import Path\nPath('/outside.txt').read_text()\n");
    let mount = format!("{}::/mnt", host_dir.path().display());
    let script = script_dir.path().join("script.py");

    let (success, stderr) = run_monty(&["-m", &mount, script.to_str().expect("utf-8 path")]);

    assert!(!success, "CLI should fail for unmounted filesystem access");
    assert!(
        stderr.contains("PermissionError: Permission denied: '/outside.txt'"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn mount_write_limit_is_enforced_from_cli_spec() {
    let host_dir = TempDir::new().expect("tempdir should be created");
    let script_dir = script_dir("from pathlib import Path\nPath('/mnt/out.txt').write_text('hello')\n");
    let mount = format!("{}::/mnt::rw::4", host_dir.path().display());
    let script = script_dir.path().join("script.py");

    let (success, stderr) = run_monty(&["-m", &mount, script.to_str().expect("utf-8 path")]);

    assert!(!success, "CLI should fail when the configured write limit is exceeded");
    assert!(
        stderr.contains("OSError: disk write limit of 4 bytes exceeded"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn invalid_write_limit_is_rejected_during_mount_parsing() {
    let script_dir = script_dir("1\n");
    let mount = format!("{}::/mnt::rw::abc", script_dir.path().display());
    let script = script_dir.path().join("script.py");

    let (success, stderr) = run_monty(&["-m", &mount, script.to_str().expect("utf-8 path")]);

    assert!(!success, "CLI should reject invalid mount write limits");
    assert_eq!(
        stderr,
        "error: invalid write limit 'abc' in '".to_owned() + &mount + "': expected a non-negative integer"
    );
}
