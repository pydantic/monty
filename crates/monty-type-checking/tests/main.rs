use std::fs;

use monty_type_checking::type_check;
use ruff_db::diagnostic::DiagnosticFormat;

#[test]
fn type_checking_success() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, 2)
    ";

    let result = type_check(code, "main.py").unwrap();
    assert!(result.is_none());
}

#[test]
fn type_checking_error() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    ";

    let result = type_check(code, "main.py").unwrap();
    assert!(result.is_some());

    let error_diagnostics = result.unwrap().to_string();
    assert_eq!(
        error_diagnostics,
        r#"error[invalid-argument-type]: Argument to function `add` is incorrect
 --> main.py:5:17
  |
3 |     return x + y
4 |
5 | result = add(1, '2')
  |                 ^^^ Expected `int`, found `Literal["2"]`
  |
info: Function defined here
 --> main.py:2:5
  |
2 | def add(x: int, y: int) -> int:
  |     ^^^         ------ Parameter declared here
3 |     return x + y
  |
info: rule `invalid-argument-type` is enabled by default

"#
    );
}

#[test]
fn type_checking_error_concise() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    ";

    let result = type_check(code, "main.py").unwrap();
    assert!(result.is_some());

    let failure = result.unwrap().format(DiagnosticFormat::Concise);
    let error_diagnostics = failure.to_string();
    assert_eq!(
        error_diagnostics,
        "main.py:5:17: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal[\"2\"]`\n"
    );
    let color_failure = failure.color(true).to_string();
    assert!(color_failure.starts_with('\u{1b}'));
}

#[test]
fn missing_stdlib_datetime() {
    let code = "import datetime\nprint(datetime.datetime.now())";

    let result = type_check(code, "main.py").unwrap();
    assert!(result.is_some());

    let failure = result.unwrap().format(DiagnosticFormat::Concise);
    let error_diagnostics = failure.to_string();
    assert_eq!(
        error_diagnostics,
        "main.py:1:8: error[unresolved-import] Cannot resolve imported module `datetime`\n"
    );
    let dbg = format!("{failure:?}");
    assert_eq!(
        dbg,
        "TypeCheckingFailure { format: Concise, color: false, diagnostics: \"main.py:1:8: error[unresolved-import] Cannot resolve imported module `datetime`\\n\" }"
    );
}

/// Test that good_types.py type-checks without errors.
///
/// This file uses `assert_type` from typing to verify that inferred types match expected types.
#[test]
fn type_good_types() {
    let code = include_str!("good_types.py");
    let result = type_check(code, "good_types.py").unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

fn check_file_content(file_name: &str, mut actual: &str) {
    let expected_path = format!("{}/tests/{}", env!("CARGO_MANIFEST_DIR"), file_name);
    let expected = if fs::exists(&expected_path).unwrap() {
        fs::read_to_string(&expected_path).unwrap()
    } else {
        std::fs::write(&expected_path, actual).unwrap();
        panic!("{file_name} did not exist, file created.")
    };

    let expected = expected.as_str().trim();
    actual = actual.trim();

    if actual == expected {
        println!("File content matches expected.");
        return;
    }

    let status = if std::env::var("UPDATE_EXPECT").is_ok() {
        std::fs::write(&expected_path, actual).unwrap();
        "FILE UPDATE"
    } else {
        "FILE NOT UPDATED"
    };

    panic!("Type errors don't match expected.\n\nEXPECTED:\n{expected}\n\nACTUAL:\n{actual}\n\n{status}.");
}

/// Test that bad_types.py produces the expected type errors.
///
/// Set `UPDATE_EXPECT=1` to update the expected errors file.
#[test]
fn type_bad_types() {
    let code = include_str!("bad_types.py");
    let result = type_check(code, "bad_types.py").unwrap();

    let failure = result.expect("Expected type errors in bad_types.py");
    let actual = failure
        .format(ruff_db::diagnostic::DiagnosticFormat::Concise)
        .to_string();

    check_file_content("bad_types_output.txt", &actual);
}

#[test]
fn test_reveal_types() {
    let code = include_str!("reveal_types.py");
    let result = type_check(code, "reveal_types.py").unwrap();

    let failure = result.expect("Expected type errors in reveal_types.py");
    let actual = failure
        .format(ruff_db::diagnostic::DiagnosticFormat::Concise)
        .to_string();

    check_file_content("reveal_types_output.txt", &actual);
}
