use monty::{MontyObject, MontyRun};

/// Test we can reuse exec without borrow checker issues.
#[test]
fn repeat_exec() {
    let ex = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], vec![]).unwrap();

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);
}

#[test]
fn test_get_interned_string() {
    let ex = MontyRun::new("'foobar'".to_owned(), "test.py", vec![], vec![]).unwrap();

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: String = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, "foobar");

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: String = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, "foobar");
}

/// Test that calling a method on a dataclass in standard execution mode
/// (without iter/external function support) returns a NotImplementedError.
/// This exercises the `FrameExit::MethodCall` path in `frame_exit_to_object`.
#[test]
fn dataclass_method_call_in_standard_mode_errors() {
    let point = MontyObject::Dataclass {
        name: "Point".to_string(),
        type_id: 0,
        field_names: vec!["x".to_string(), "y".to_string()],
        attrs: vec![
            (MontyObject::String("x".to_string()), MontyObject::Int(1)),
            (MontyObject::String("y".to_string()), MontyObject::Int(2)),
        ]
        .into(),
        frozen: true,
    };

    let ex = MontyRun::new("point.sum()".to_owned(), "test.py", vec!["point".to_string()], vec![]).unwrap();

    let err = ex.run_no_limits(vec![point]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Method call 'sum' not implemented with standard execution"),
        "Expected NotImplementedError for method call, got: {msg}"
    );
}
