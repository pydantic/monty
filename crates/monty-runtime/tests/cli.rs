// every test here drives the standalone CLI, which a worker-only build refuses
#![cfg(feature = "standalone")]

use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

/// Runs the CLI binary with `stdin` piped in as its standard input.
fn run_monty_with_stdin(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_monty"))
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("monty CLI should run");
    child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(stdin.as_bytes())
        .expect("stdin should be written");
    child.wait_with_output().expect("monty CLI should finish")
}

/// `monty -` runs the program read from standard input, like `python -`.
#[test]
fn dash_runs_the_program_from_stdin() {
    let output = run_monty_with_stdin(&["-"], "x = 'hello'\nprint(x, 'world')\n");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "CLI should succeed, stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello world\n");
}

/// A traceback from a program read from stdin names `<stdin>` as its file.
#[test]
fn dash_reports_errors_against_stdin() {
    let output = run_monty_with_stdin(&["-"], "1 / 0\n");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "CLI should fail, stderr: {stderr}");
    assert!(
        stderr.contains("File \"<stdin>\", line 1"),
        "unexpected stderr: {stderr}"
    );
    assert!(stderr.contains("ZeroDivisionError"), "unexpected stderr: {stderr}");
}

/// Options before `-` still apply to the program read from stdin.
#[test]
fn dash_keeps_cli_options() {
    let output = run_monty_with_stdin(
        &["--max-recursion-depth", "10", "-"],
        "def f(n):\n    return f(n + 1)\nf(0)\n",
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "CLI should fail, stderr: {stderr}");
    assert!(stderr.contains("RecursionError"), "unexpected stderr: {stderr}");
}

/// The type checker names `<stdin>` in its diagnostics for a program read from stdin.
#[test]
fn dash_type_checks_against_stdin() {
    let output = run_monty_with_stdin(&["-t", "-"], "x: int = 'a'\n");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("<stdin>:1:"), "unexpected stderr: {stderr}");
}
