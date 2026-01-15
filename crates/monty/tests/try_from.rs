use monty::MontyRun;

/// Tests for successful TryFrom conversions from Python values to Rust types.
///
/// These tests validate that the `TryFrom` implementations on `MontyObject` correctly
/// convert Python objects to their corresponding Rust types when the conversion
/// is valid (e.g., Python int to Rust i64, Python str to Rust String).

#[test]
fn try_from_ok_int_to_i64() {
    let ex = MontyRun::new("42".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: i64 = (&result).try_into().expect("conversion should succeed");
    assert_eq!(value, 42);
}

#[test]
fn try_from_ok_zero_to_i64() {
    let ex = MontyRun::new("0".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: i64 = (&result).try_into().expect("conversion should succeed");
    assert_eq!(value, 0);
}

#[test]
#[expect(clippy::float_cmp)]
fn try_from_ok_float_to_f64() {
    let ex = MontyRun::new("2.5".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: f64 = (&result).try_into().expect("conversion should succeed");
    assert_eq!(value, 2.5);
}

#[test]
#[expect(clippy::float_cmp)]
fn try_from_ok_int_to_f64() {
    let ex = MontyRun::new("42".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: f64 = (&result).try_into().expect("conversion should succeed");
    assert_eq!(value, 42.0);
}

#[test]
fn try_from_ok_string_to_string() {
    let ex = MontyRun::new("'hello'".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: String = (&result).try_into().expect("conversion should succeed");
    assert_eq!(value, "hello".to_string());
}

#[test]
fn try_from_ok_empty_string_to_string() {
    let ex = MontyRun::new("''".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: String = (&result).try_into().expect("conversion should succeed");
    assert_eq!(value, String::new());
}

#[test]
fn try_from_ok_multiline_string_to_string() {
    let ex = MontyRun::new("'hello\\nworld'".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: String = (&result).try_into().expect("conversion should succeed");
    assert_eq!(value, "hello\nworld".to_string());
}

#[test]
fn try_from_ok_bool_true_to_bool() {
    let ex = MontyRun::new("True".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: bool = (&result).try_into().expect("conversion should succeed");
    assert!(value);
}

#[test]
fn try_from_ok_bool_false_to_bool() {
    let ex = MontyRun::new("False".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let value: bool = (&result).try_into().expect("conversion should succeed");
    assert!(!value);
}

/// Tests for failed TryFrom conversions from Python values to Rust types.
///
/// These tests validate that the `TryFrom` implementations correctly reject
/// invalid conversions with appropriate error messages (e.g., trying to convert
/// a Python str to a Rust i64).

#[test]
fn try_from_err_string_to_i64() {
    let ex = MontyRun::new("'hello'".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<i64>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected int, got str");
}

#[test]
fn try_from_err_float_to_i64() {
    let ex = MontyRun::new("2.5".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<i64>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected int, got float");
}

#[test]
fn try_from_err_none_to_i64() {
    let ex = MontyRun::new("None".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<i64>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected int, got NoneType");
}

#[test]
fn try_from_err_list_to_i64() {
    let ex = MontyRun::new("[1, 2, 3]".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<i64>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected int, got list");
}

#[test]
fn try_from_err_int_to_string() {
    let ex = MontyRun::new("42".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<String>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected str, got int");
}

#[test]
fn try_from_err_none_to_string() {
    let ex = MontyRun::new("None".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<String>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected str, got NoneType");
}

#[test]
fn try_from_err_list_to_string() {
    let ex = MontyRun::new("[1, 2]".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<String>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected str, got list");
}

#[test]
fn try_from_err_int_to_bool() {
    let ex = MontyRun::new("1".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<bool>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected bool, got int");
}

#[test]
fn try_from_err_string_to_bool() {
    let ex = MontyRun::new("'true'".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<bool>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected bool, got str");
}

#[test]
fn try_from_err_none_to_bool() {
    let ex = MontyRun::new("None".to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let err = TryInto::<bool>::try_into(&result).expect_err("conversion should fail");
    assert_eq!(err.to_string(), "expected bool, got NoneType");
}
