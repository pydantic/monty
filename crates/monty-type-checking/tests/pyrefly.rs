//! Tests for the experimental Pyrefly backend.

#![cfg(feature = "pyrefly")]

use insta::assert_snapshot;
use monty_type_checking::{SourceFile, TypeChecker};
use monty_types::{TypeCheckingConfig, TypeCheckingFormat};

fn pyrefly() -> TypeChecker {
    TypeChecker::default()
}

fn concise() -> TypeCheckingConfig {
    TypeCheckingConfig {
        format: TypeCheckingFormat::Concise,
        color: false,
    }
}

fn render(checker: &mut TypeChecker, code: &str, path: &str) -> Option<String> {
    checker
        .run(&SourceFile::new(code, path), None, concise())
        .expect("type check should not fail internally")
        .map(|d| d.to_string())
}

#[test]
fn clean_snippet_has_no_diagnostics() {
    let code = "\
def add(x: int, y: int) -> int:
    return x + y

result = add(1, 2)
";
    let result = render(&mut pyrefly(), code, "main.py");
    assert!(result.is_none(), "expected no type errors, got: {result:#?}");
}

/// The same snippet `main.rs` checks under ty, for comparing wording.
#[test]
fn argument_type_error_is_reported() {
    let code = "\
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
";
    assert_snapshot!(
        render(&mut pyrefly(), code, "main.py").expect("expected type errors"),
        @"main.py:4:17: error[bad-argument-type] Argument `Literal['2']` is not assignable to parameter `y` with type `int` in function `add`"
    );
}

/// The injected stub import must not shift the reported line.
#[test]
fn stub_import_line_offset_is_removed() {
    let stubs = "\
class Widget:
    x: int
";
    let code = "\
w = Widget()
w.x = 'not an int'
";
    let mut checker = pyrefly();
    let diagnostics = checker
        .run(
            &SourceFile::new(code, "main.py"),
            Some(&SourceFile::new(stubs, "type_stubs.pyi")),
            concise(),
        )
        .unwrap()
        .expect("expected type errors")
        .to_string();
    assert_snapshot!(
        diagnostics,
        @"main.py:2:7: error[bad-assignment] `Literal['not an int']` is not assignable to attribute `x` with type `int`"
    );
}

#[test]
fn machine_readable_formats_are_rejected() {
    let mut checker = pyrefly();
    let result = checker.run(
        &SourceFile::new("x = 1\n", "main.py"),
        None,
        TypeCheckingConfig {
            format: TypeCheckingFormat::Json,
            color: false,
        },
    );
    match result {
        Err(err) => assert_snapshot!(
            err,
            @"the pyrefly type-check backend renders only 'full' and 'concise', not 'json'"
        ),
        Ok(_) => panic!("json must be rejected by the pyrefly backend"),
    }
}

/// Security-critical: a reused checker must not resolve a name from a prior run.
#[test]
fn rerun_does_not_see_previous_source() {
    let mut checker = pyrefly();
    let first = render(&mut checker, "GOOD: int = 1\nresult = GOOD + 1\n", "main.py");
    assert!(first.is_none(), "first run should succeed: {first:#?}");

    let second = render(&mut checker, "x: int = GOOD\n", "main.py")
        .expect("second run must error — `GOOD` was only defined in the first run");
    assert_snapshot!(second, @"main.py:1:10: error[unknown-name] Could not find name `GOOD`");
}

/// Security-critical: `reset` must scrub the stubs a session supplied.
#[test]
fn reset_removes_stubs() {
    let mut checker = pyrefly();
    let first = checker
        .run(
            &SourceFile::new("from __monty_stubs__ import Widget\nw: Widget\n", "main.py"),
            Some(&SourceFile::new("class Widget:\n    x: int\n", "type_stubs.pyi")),
            concise(),
        )
        .unwrap()
        .map(|d| d.to_string());
    assert!(first.is_none(), "first run with stubs should succeed: {first:#?}");
    checker.reset().expect("reset");

    let second = render(&mut checker, "from __monty_stubs__ import Widget\n", "main.py")
        .expect("second run must error — the stubs were scrubbed by reset");
    assert_snapshot!(second, @"main.py:1:29: error[missing-module-attribute] Could not import `Widget` from `__monty_stubs__`");
}
