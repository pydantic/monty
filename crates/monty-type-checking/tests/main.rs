use monty_type_checking::type_check;
use ruff_db::diagnostic::DiagnosticFormat;

#[test]
fn type_checking_success() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, 2)
    ";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none());
}

#[test]
fn type_checking_error() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    ";

    let result = type_check(code, None).unwrap();
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

    let result = type_check(code, None).unwrap();
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

    let result = type_check(code, None).unwrap();
    assert!(result.is_some());

    let failure = result.unwrap().format(DiagnosticFormat::Concise);
    let error_diagnostics = failure.to_string();
    assert_eq!(
        error_diagnostics,
        "main.py:1:8: error[unresolved-import] Cannot resolve imported module `datetime`\n"
    );
    let dbg = format!("{failure:?}");
    assert_eq!(dbg, "TypeCheckingFailure { format: Concise, color: false, diagnostics: \"main.py:1:8: error[unresolved-import] Cannot resolve imported module `datetime`\\n\" }");
}
