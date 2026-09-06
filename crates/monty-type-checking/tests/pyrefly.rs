#![cfg(feature = "pyrefly")]

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
