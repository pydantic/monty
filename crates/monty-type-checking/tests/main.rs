use std::thread;

use insta::assert_snapshot;
use monty_type_checking::{SourceFile, type_check};
use ruff_db::diagnostic::DiagnosticFormat;

#[test]
fn type_checking_success() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, 2)
    ";

    let result = type_check(&SourceFile::new(code, "main.py"), None).unwrap();
    assert!(result.is_none());
}

#[test]
fn type_checking_error() {
    let code = "\
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    ";

    let result = type_check(&SourceFile::new(code, "main.py"), None).unwrap();
    let error_diagnostics = result.expect("expected type errors").to_string();
    assert_snapshot!(error_diagnostics, @r#"
    error[invalid-argument-type]: Argument to function `add` is incorrect
     --> main.py:4:17
      |
    2 |     return x + y
    3 |
    4 | result = add(1, '2')
      |                 ^^^ Expected `int`, found `Literal["2"]`
      |
    info: Function defined here
     --> main.py:1:5
      |
    1 | def add(x: int, y: int) -> int:
      |     ^^^         ------ Parameter declared here
    2 |     return x + y
      |
    info: rule `invalid-argument-type` is enabled by default
    "#);
}

#[test]
fn type_checking_error_stubs() {
    let stubs = "\
from dataclasses import dataclass

@dataclass
class User:
    name: str
    age: int
";
    let code = "\
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')";

    let result = type_check(
        &SourceFile::new(code, "main.py"),
        Some(&SourceFile::new(stubs, "type_stubs.pyi")),
    )
    .unwrap();

    let error_diagnostics = result.expect("expected type errors").to_string();
    assert_snapshot!(error_diagnostics, @r#"
    error[invalid-argument-type]: Argument to function `add` is incorrect
     --> main.py:4:17
      |
    2 |     return x + y
    3 |
    4 | result = add(1, '2')
      |                 ^^^ Expected `int`, found `Literal["2"]`
      |
    info: Function defined here
     --> main.py:1:5
      |
    1 | def add(x: int, y: int) -> int:
      |     ^^^         ------ Parameter declared here
    2 |     return x + y
      |
    info: rule `invalid-argument-type` is enabled by default
    "#);
}

#[test]
fn type_checking_error_concise() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    ";

    let result = type_check(&SourceFile::new(code, "main.py"), None).unwrap();
    let failure = result.expect("expected type errors").format(DiagnosticFormat::Concise);
    assert_snapshot!(
        failure.to_string(),
        @r#"main.py:5:17: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["2"]`"#
    );
    let color_failure = failure.color(true).to_string();
    assert!(color_failure.starts_with('\u{1b}'));
}

#[test]
fn stdlib_datetime_resolves() {
    let code = "import datetime\nprint(datetime.datetime.now())";

    let result = type_check(&SourceFile::new(code, "main.py"), None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:#?}");
}

/// Test that good_types.py type-checks without errors.
///
/// This file uses `assert_type` from typing to verify that inferred types match expected types.
#[test]
fn type_good_types() {
    let code = include_str!("good_types.py");
    let result = type_check(&SourceFile::new(code, "good_types.py"), None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:#?}");
}

/// Test that bad_types.py produces the expected type errors.
///
/// Snapshot stored in `tests/snapshots/main__type_bad_types.snap`.
/// Run `cargo insta review` (or `cargo insta test --accept`) to update.
#[test]
fn type_bad_types() {
    let code = include_str!("bad_types.py");
    let result = type_check(&SourceFile::new(code, "bad_types.py"), None).unwrap();

    let failure = result.expect("Expected type errors in bad_types.py");
    let actual = failure.format(DiagnosticFormat::Concise).to_string();

    assert_snapshot!(actual);
}

/// Security-critical: verify the reusable `MemoryDb` pool never exposes files,
/// modules, or derived query results from a previous `type_check` call to a
/// later one. If any of these assertions break, code from a prior run could
/// leak into a later run's semantic analysis.
#[test]
fn pooled_db_no_cross_run_module_leak() {
    // First run: define a module with a secret constant. Looks valid on its own.
    let first = "SECRET = 'hunter2'\n";
    let r1 = type_check(&SourceFile::new(first, "leaky.py"), None).unwrap();
    assert!(r1.is_none(), "first run should succeed: {r1:#?}");

    // Second run: try to import the first run's symbol via its file stem.
    // Must fail with unresolved-import — the file was deleted when the db was
    // returned to the pool, and Salsa's module-resolution memo must have been
    // invalidated.
    let second = "from leaky import SECRET\nprint(SECRET)\n";
    let r2 = type_check(&SourceFile::new(second, "main.py"), None)
        .unwrap()
        .expect("second run must raise unresolved-import for the stale module");
    assert_snapshot!(
        r2.format(DiagnosticFormat::Concise).to_string(),
        @"main.py:1:6: error[unresolved-import] Cannot resolve imported module `leaky`"
    );
}

/// Security-critical: same path, different content on consecutive runs. The
/// second run must be evaluated against its own source, not a cached version
/// of the first run's source.
#[test]
fn pooled_db_no_cross_run_same_path_leak() {
    // First run: defines `GOOD` and type-checks cleanly.
    let first = "GOOD: int = 1\nresult = GOOD + 1\n";
    let r1 = type_check(&SourceFile::new(first, "main.py"), None).unwrap();
    assert!(r1.is_none(), "first run should succeed: {r1:#?}");

    // Second run on the *same* path references a name that doesn't exist in
    // this run's source. If the pool leaked `GOOD` from the first run, this
    // would type-check clean — so we assert an error is produced.
    let second = "x: int = GOOD\n";
    let r2 = type_check(&SourceFile::new(second, "main.py"), None)
        .unwrap()
        .expect("second run must error — `GOOD` was only defined in the first run");
    assert_snapshot!(
        r2.format(DiagnosticFormat::Concise).to_string(),
        @"main.py:1:10: error[unresolved-reference] Name `GOOD` used when not defined"
    );
}

/// Security-critical: stubs from a previous run must not persist as importable
/// modules in a later run.
#[test]
fn pooled_db_no_cross_run_stubs_leak() {
    let stubs = "class Widget:\n    x: int\n";
    let first = "from type_stubs import Widget\nw: Widget\n";
    let r1 = type_check(
        &SourceFile::new(first, "main.py"),
        Some(&SourceFile::new(stubs, "type_stubs.pyi")),
    )
    .unwrap();
    assert!(r1.is_none(), "first run with stubs should succeed: {r1:#?}");

    // Second run: no stubs provided, but still references `Widget`. Must fail
    // to resolve the name — the stubs file was deleted on cleanup.
    let second = "from type_stubs import Widget\n";
    let r2 = type_check(&SourceFile::new(second, "main.py"), None)
        .unwrap()
        .expect("second run must error — `type_stubs` was only provided in the first run");
    assert_snapshot!(
        r2.format(DiagnosticFormat::Concise).to_string(),
        @"main.py:1:6: error[unresolved-import] Cannot resolve imported module `type_stubs`"
    );
}

/// Security-critical: exercise the pool under concurrent load mixing both
/// clean and failing type-checks. If the cleanup / release logic ever returned
/// a contaminated db to the pool, subsequent checks would see stale names
/// from other threads and fail intermittently.
#[test]
fn pooled_db_concurrent_runs_stay_isolated() {
    thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|thread_idx| {
                scope.spawn(move || {
                    for iter in 0..20 {
                        // Alternate between a clean run that defines a name and
                        // a run that would only succeed if the prior run's
                        // name leaked through the pool.
                        let code_ok = format!("T_{thread_idx}_{iter}: int = 1\n");
                        let r1 = type_check(&SourceFile::new(&code_ok, "main.py"), None).unwrap();
                        assert!(r1.is_none(), "ok run must succeed: {r1:#?}");

                        let leak_probe = format!("x: int = T_{thread_idx}_{iter}\n");
                        // Reusing the same path: if the pool leaked the prior
                        // run's defs, `T_*_*` would still resolve. It must not.
                        let r2 = type_check(&SourceFile::new(&leak_probe, "main.py"), None).unwrap();
                        let d = r2.expect("leak probe must error — prior run's name must not be visible");
                        let msg = d.format(DiagnosticFormat::Concise).to_string();
                        assert_eq!(
                            msg,
                            format!(
                                "main.py:1:10: error[unresolved-reference] Name `T_{thread_idx}_{iter}` used when not defined\n"
                            ),
                        );
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
}

/// Security-critical: nested paths must be accepted and fully scrubbed (including
/// the intermediate directories) so the next pooled run cannot see the previous
/// run's module structure.
#[test]
fn pooled_db_nested_paths_are_cleaned_up() {
    // First run defines a name in a nested module; type-checks clean.
    let r1 = type_check(&SourceFile::new("LEAKY: int = 1\n", "sub_dir/leaky.py"), None).unwrap();
    assert!(r1.is_none(), "first nested-path run should succeed: {r1:#?}");

    // Second run on the same path uses a different name; if the previous run's
    // file or its containing directory leaked into the pool, `LEAKY` would still
    // resolve. Must error.
    let r2 = type_check(&SourceFile::new("x: int = LEAKY\n", "sub_dir/leaky.py"), None)
        .unwrap()
        .expect("nested-path leak probe must error — `LEAKY` must not survive into the next run");
    assert_snapshot!(
        r2.format(DiagnosticFormat::Concise).to_string(),
        @"sub_dir/leaky.py:1:10: error[unresolved-reference] Name `LEAKY` used when not defined"
    );

    // Third run from a *different* nested path importing the previous module must
    // also fail — the directory should be empty / unresolvable.
    let r3 = type_check(&SourceFile::new("from sub_dir.leaky import LEAKY\n", "other.py"), None)
        .unwrap()
        .expect("third run must error — sub_dir/leaky.py was deleted on cleanup");
    assert_snapshot!(
        r3.format(DiagnosticFormat::Concise).to_string(),
        @"other.py:1:6: error[unresolved-import] Cannot resolve imported module `sub_dir.leaky`"
    );
}

/// Snapshot stored in `tests/snapshots/main__reveal_types.snap`.
/// Run `cargo insta review` (or `cargo insta test --accept`) to update.
#[test]
fn test_reveal_types() {
    let code = include_str!("reveal_types.py");
    let result = type_check(&SourceFile::new(code, "reveal_types.py"), None).unwrap();

    let failure = result.expect("Expected type errors in reveal_types.py");
    let actual = failure.format(DiagnosticFormat::Concise).to_string();

    assert_snapshot!(actual);
}
