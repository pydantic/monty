use monty::{CompileOptions, MontyObject, MontyRun};

/// Test we can reuse exec without borrow checker issues.
#[test]
fn repeat_exec() {
    let ex = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);
}

#[test]
fn test_get_interned_string() {
    let ex = MontyRun::new("'foobar'".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

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

    let ex = MontyRun::new(
        "point.sum()".to_owned(),
        "test.py",
        vec!["point".to_string()],
        CompileOptions::default(),
    )
    .unwrap();

    let err = ex.run_no_limits(vec![point]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Method call 'sum' not implemented with standard execution"),
        "Expected NotImplementedError for method call, got: {msg}"
    );
}

/// Test that subscript augmented matrix multiplication reports the dedicated
/// unsupported-operation compile error.
///
/// CPython supports `@=` syntax, so the comparative Python test-case suite
/// cannot cover Monty's current compile-time rejection of this operator. Keep
/// this as a Rust-side regression test until matrix multiplication support
/// exists.
#[test]
fn subscript_augassign_matmul_reports_not_supported() {
    let err = MontyRun::new(
        "d = {'x': 1}\nd['x'] @= 2".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 2\n    d['x'] @= 2\n    ~~~~~~\nSyntaxError: matrix multiplication augmented assignment (@=) is not yet supported"
    );
}

/// A class whose `__init__` is bound to an external function cannot suspend:
/// non-plain-function `__init__` runs synchronously via `evaluate_function`,
/// which cannot yield to the host, so the call raises `NotImplementedError`
/// (documented in `limitations/classes.md`). Kept as a Rust-side test because
/// on CPython the external is a real function and construction would succeed,
/// so the comparative test-case suite cannot cover it.
#[test]
fn external_function_as_init_raises_not_implemented() {
    let code = "class Foo:\n    __init__ = ext_fn\n\nFoo()";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::Function {
            name: "ext_fn".to_owned(),
            docstring: None,
        }])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 4, in <module>\n    Foo()\n    ~~~~~\nNotImplementedError: __init__: external functions are not yet supported in this context"
    );
}

/// The 3-arg `type()` form rejects non-empty bases because Monty classes
/// cannot inherit (documented in `limitations/classes.md`). Kept as a
/// Rust-side test because CPython accepts bases, so the comparative
/// test-case suite cannot cover the divergence.
#[test]
fn dynamic_type_with_bases_raises_type_error() {
    let code = "type('A', (int,), {})";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let err = ex.run_no_limits(vec![]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 1, in <module>\n    type('A', (int,), {})\n    ~~~~~~~~~~~~~~~~~~~~~\nTypeError: type() bases are not supported"
    );
}

/// The 3-arg `type()` form rejects non-string namespace keys with a
/// `TypeError` — CPython only emits a `RuntimeWarning`, and Monty has no
/// warnings machinery, so silently accepting them would hide the mistake
/// (documented in `limitations/classes.md`). Kept as a Rust-side test
/// because CPython succeeds here, so the comparative test-case suite
/// cannot cover the divergence.
#[test]
fn dynamic_type_with_non_string_key_raises_type_error() {
    let code = "type('A', (), {1: 'one'})";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let err = ex.run_no_limits(vec![]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 1, in <module>\n    type('A', (), {1: 'one'})\n    ~~~~~~~~~~~~~~~~~~~~~~~~~\nTypeError: non-string key (int) in the namespace of class 'A'"
    );
}
