use std::process::Command;

/// The trainer must not rebuild the interpreter or CPython to drive a subprocess.
#[test]
fn training_dependencies_are_host_only() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "--offline",
            "--package",
            "monty-pgo",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let tree = String::from_utf8(output.stdout).unwrap();
    let unexpected: Vec<_> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| {
            matches!(
                *name,
                "monty" | "monty-datatest" | "monty-fixture" | "monty-type-checking" | "pyo3"
            )
        })
        .collect();
    assert_eq!(unexpected, Vec::<&str>::new());
}
