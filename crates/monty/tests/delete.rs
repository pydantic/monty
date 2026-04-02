use monty::{ExcType, MontyException, MontyRun};

/// Runs code and asserts that execution fails with the expected exception type and message.
fn assert_runtime_error(code: &str, expected_type: ExcType, expected_message: &str) {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).expect("code should compile");
    let err = run.run_no_limits(vec![]).expect_err("code should raise");
    assert_exception(&err, expected_type, expected_message);
}

/// Asserts the exception type and message without caring whether it came from parse or runtime.
fn assert_exception(err: &MontyException, expected_type: ExcType, expected_message: &str) {
    assert_eq!(err.exc_type(), expected_type);
    assert_eq!(err.message(), Some(expected_message));
}

#[test]
fn deleting_missing_global_raises_name_error() {
    assert_runtime_error("del missing", ExcType::NameError, "name 'missing' is not defined");
}

#[test]
fn deleting_unbound_local_raises_unbound_local_error() {
    assert_runtime_error(
        "def f():\n    del x\n    x = 1\nf()",
        ExcType::UnboundLocalError,
        "cannot access local variable 'x' where it is not associated with a value",
    );
}

#[test]
fn deleting_captured_local_keeps_outer_name_unbound() {
    assert_runtime_error(
        "def outer():\n    x = 1\n    def inner():\n        return x\n    del x\n    return x\nouter()",
        ExcType::UnboundLocalError,
        "cannot access local variable 'x' where it is not associated with a value",
    );
}

#[test]
fn deleting_captured_local_keeps_inner_closure_unbound() {
    assert_runtime_error(
        "def outer():\n    x = 1\n    def inner():\n        return x\n    del x\n    return inner()\nouter()",
        ExcType::NameError,
        "cannot access free variable 'x' where it is not associated with a value in enclosing scope",
    );
}

#[test]
fn deleting_nonlocal_keeps_outer_cell_unbound() {
    assert_runtime_error(
        "def outer():\n    x = 1\n    def delete_x():\n        nonlocal x\n        del x\n    delete_x()\n    return x\nouter()",
        ExcType::UnboundLocalError,
        "cannot access local variable 'x' where it is not associated with a value",
    );
}
