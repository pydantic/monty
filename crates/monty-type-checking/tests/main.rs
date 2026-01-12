use monty_type_checking::type_check;

#[test]
fn test_type_checking_success() {
    let code = r#"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, 2)
    "#;

    let result = type_check(&code, None).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_type_checking_error() {
    let code = r#"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    "#;

    let result = type_check(&code, None).unwrap();
    assert!(result.is_some());

    let error_diagnostics = result.unwrap();
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
