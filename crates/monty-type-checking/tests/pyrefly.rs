#![cfg(feature = "pyrefly")]

use insta::assert_snapshot;
use monty_type_checking::{SourceFile, TypeChecker};
use monty_types::TypeCheckingConfig;

fn check(checker: &mut TypeChecker, code: &str, stubs: Option<&str>) -> Option<String> {
    let stubs = stubs.map(|s| SourceFile::new(s, "type_stubs.pyi"));
    checker
        .run(
            &SourceFile::new(code, "main.py"),
            stubs.as_ref(),
            TypeCheckingConfig::default(),
        )
        .unwrap()
        .map(|d| d.to_string())
}

#[test]
fn single_snippet() {
    let mut checker = TypeChecker::default();
    assert!(check(&mut checker, "x: int = 1\n", None).is_none());
    assert!(check(&mut checker, "x: int = 'nope'\n", None).is_some());
}

#[test]
fn repl_sequence() {
    let mut checker = TypeChecker::default();
    let stubs = "x = 1\n";
    assert!(check(&mut checker, "y = x + 2\n", Some(stubs)).is_none());
    assert!(check(&mut checker, "y = undefined\n", Some(stubs)).is_some());
}

#[test]
fn error_output() {
    let mut checker = TypeChecker::default();
    let code = "def add(x: int, y: int) -> int:\n    return x + y\n\nr = add(1, '2')\n";
    assert_snapshot!(check(&mut checker, code, None).unwrap(), @"main.py:4:12: error[bad-argument-type] Argument `Literal['2']` is not assignable to parameter `y` with type `int` in function `add`");
}

/// The injected stub import must not shift the reported line: the error is on
/// line 2 of the snippet below.
#[test]
fn error_output_with_stubs() {
    let mut checker = TypeChecker::default();
    let code = "w = Widget()\nw.x = 'not an int'\n";
    let stubs = "class Widget:\n    x: int\n";
    assert_snapshot!(check(&mut checker, code, Some(stubs)).unwrap(), @"main.py:2:7: error[bad-assignment] `Literal['not an int']` is not assignable to attribute `x` with type `int`");
}

/// Security-critical: `reset` must scrub the stubs a session supplied.
#[test]
fn reset_removes_stubs() {
    let mut checker = TypeChecker::default();
    let code = "from __monty_stubs__ import Widget\n";
    assert!(check(&mut checker, code, Some("class Widget:\n    x: int\n")).is_none());

    checker.reset().unwrap();
    assert!(check(&mut checker, code, None).is_some());
}
