//! Tests for passing input values to the executor.
//!
//! These tests verify that `MontyObject` inputs are correctly converted to `Object`
//! and can be used in Python code execution.

use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject};

// === Immediate Value Tests ===

#[test]
fn input_int() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Int(42)]).unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn input_int_arithmetic() {
    let ex = MontyRun::new(
        "x + 1".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Int(41)]).unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn input_bool_true() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Bool(true)]).unwrap();
    assert_eq!(result, MontyObject::Bool(true));
}

#[test]
fn input_bool_false() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Bool(false)]).unwrap();
    assert_eq!(result, MontyObject::Bool(false));
}

#[test]
fn input_float() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Float(2.5)]).unwrap();
    assert_eq!(result, MontyObject::Float(2.5));
}

#[test]
fn input_none() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::None]).unwrap();
    assert_eq!(result, MontyObject::None);
}

#[test]
fn input_ellipsis() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Ellipsis]).unwrap();
    assert_eq!(result, MontyObject::Ellipsis);
}

// === Heap-Allocated Value Tests ===

#[test]
fn input_string() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::String("hello".to_string())])
        .unwrap();
    assert_eq!(result, MontyObject::String("hello".to_string()));
}

#[test]
fn input_string_concat() {
    let ex = MontyRun::new(
        "x + ' world'".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::String("hello".to_string())])
        .unwrap();
    assert_eq!(result, MontyObject::String("hello world".to_string()));
}

#[test]
fn input_bytes() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Bytes(vec![1, 2, 3])]).unwrap();
    assert_eq!(result, MontyObject::Bytes(vec![1, 2, 3]));
}

#[test]
fn input_list() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn input_list_append() {
    let ex = MontyRun::new(
        "x.append(3)\nx".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2), MontyObject::Int(3)])
    );
}

#[test]
fn input_tuple() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Tuple(vec![
            MontyObject::Int(1),
            MontyObject::String("two".to_string()),
        ])])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Int(1), MontyObject::String("two".to_string())])
    );
}

#[test]
fn input_dict() {
    let map = vec![(MontyObject::String("a".to_string()), MontyObject::Int(1))];

    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::dict(map)]).unwrap();

    // Build expected map for comparison
    assert_eq!(
        result,
        MontyObject::dict(vec![(MontyObject::String("a".to_string()), MontyObject::Int(1))])
    );
}

#[test]
fn input_dict_get() {
    let map = vec![(MontyObject::String("key".to_string()), MontyObject::Int(42))];

    let ex = MontyRun::new(
        "x['key']".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::dict(map)]).unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

// === Multiple Inputs ===

#[test]
fn multiple_inputs_two() {
    let ex = MontyRun::new(
        "x + y".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Int(10), MontyObject::Int(32)])
        .unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn multiple_inputs_three() {
    let ex = MontyRun::new(
        "x + y + z".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned(), "z".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Int(10), MontyObject::Int(20), MontyObject::Int(12)])
        .unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn multiple_inputs_mixed_types() {
    // Create a list from two inputs
    let ex = MontyRun::new(
        "[x, y]".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Int(1), MontyObject::String("two".to_string())])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::String("two".to_string())])
    );
}

// === Edge Cases ===

#[test]
fn no_inputs() {
    let ex = MontyRun::new("42".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn nested_list() {
    let ex = MontyRun::new(
        "x[0][1]".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::List(vec![MontyObject::List(vec![
            MontyObject::Int(1),
            MontyObject::Int(2),
        ])])])
        .unwrap();
    assert_eq!(result, MontyObject::Int(2));
}

#[test]
fn empty_list_input() {
    let ex = MontyRun::new(
        "len(x)".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::List(vec![])]).unwrap();
    assert_eq!(result, MontyObject::Int(0));
}

#[test]
fn empty_string_input() {
    let ex = MontyRun::new(
        "len(x)".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::String(String::new())]).unwrap();
    assert_eq!(result, MontyObject::Int(0));
}

// === Exception Input Tests ===

#[test]
fn input_exception() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Exception {
            exc_type: ExcType::ValueError,
            arg: Some("test message".to_string()),
        }])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::Exception {
            exc_type: ExcType::ValueError,
            arg: Some("test message".to_string()),
        }
    );
}

#[test]
fn input_exception_no_arg() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Exception {
            exc_type: ExcType::TypeError,
            arg: None,
        }])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::Exception {
            exc_type: ExcType::TypeError,
            arg: None,
        }
    );
}

#[test]
fn input_exception_in_list() {
    let ex = MontyRun::new(
        "x[0]".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::List(vec![MontyObject::Exception {
            exc_type: ExcType::KeyError,
            arg: Some("key".to_string()),
        }])])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::Exception {
            exc_type: ExcType::KeyError,
            arg: Some("key".to_string()),
        }
    );
}

#[test]
fn input_exception_raise() {
    // Test that an exception passed as input can be raised
    let ex = MontyRun::new(
        "raise x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Exception {
        exc_type: ExcType::ValueError,
        arg: Some("input error".to_string()),
    }]);
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("input error"));
}

// === Invalid Input Tests ===

#[test]
fn invalid_input_repr() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::Repr("some repr".to_string())]);
    assert!(result.is_err(), "Repr should not be a valid input");
}

#[test]
fn invalid_input_repr_nested_in_list() {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // Repr nested inside a list should still be invalid
    let result = ex.run_no_limits(vec![MontyObject::List(vec![MontyObject::Repr(
        "nested repr".to_string(),
    )])]);
    assert!(result.is_err(), "Repr nested in list should be invalid");
}

// === Error-Path Cleanup Tests ===
// An invalid element placed *after* elements that allocate heap values: the
// partially-built container must release the already-converted values (the
// `memory-model-checks` feature panics on any missed drop) and report the error.

/// Runs `x` bound to `input`, returning the conversion/execution result.
fn run_input(input: MontyObject) -> Result<MontyObject, monty_types::MontyException> {
    let ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    ex.run_no_limits(vec![input])
}

/// A list element guaranteed to allocate on the heap during conversion.
fn heap_element() -> MontyObject {
    MontyObject::List(vec![MontyObject::Int(1)])
}

#[test]
fn invalid_input_repr_in_list_after_heap_values() {
    let err = run_input(MontyObject::List(vec![
        heap_element(),
        MontyObject::Repr("bad".to_owned()),
    ]))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_tuple_after_heap_values() {
    let err = run_input(MontyObject::Tuple(vec![
        heap_element(),
        MontyObject::Repr("bad".to_owned()),
    ]))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_dict_value_after_pairs() {
    // The first pair converts fully (heap key and value); the second pair's
    // key converts before its value fails, exercising the key-guard path too.
    let err = run_input(MontyObject::Dict(
        vec![
            (MontyObject::String("a".to_owned()), heap_element()),
            (MontyObject::String("b".to_owned()), MontyObject::Repr("bad".to_owned())),
        ]
        .into(),
    ))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_set_after_heap_values() {
    let err = run_input(MontyObject::Set(vec![
        MontyObject::String("heap string".to_owned()),
        MontyObject::Repr("bad".to_owned()),
    ]))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_frozenset_after_heap_values() {
    let err = run_input(MontyObject::FrozenSet(vec![
        MontyObject::String("heap string".to_owned()),
        MontyObject::Repr("bad".to_owned()),
    ]))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_namedtuple_after_heap_values() {
    let err = run_input(MontyObject::NamedTuple {
        type_name: "nt".to_owned(),
        field_names: vec!["a".to_owned(), "b".to_owned()],
        values: vec![heap_element(), MontyObject::Repr("bad".to_owned())],
    })
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_namedtuple_length_mismatch() {
    // `NamedTuple::new` asserts equal lengths — malformed host input must
    // surface as an error, not a panic.
    let err = run_input(MontyObject::NamedTuple {
        type_name: "nt".to_owned(),
        field_names: vec!["a".to_owned()],
        values: vec![MontyObject::Int(1), MontyObject::Int(2)],
    })
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: NamedTuple field_names and values must have the same length")
    );
}

#[test]
fn invalid_input_repr_in_dataclass_attrs() {
    let err = run_input(MontyObject::Dataclass {
        name: "Point".to_owned(),
        type_id: 1,
        field_names: vec!["a".to_owned(), "b".to_owned()],
        attrs: vec![
            (MontyObject::String("a".to_owned()), heap_element()),
            (MontyObject::String("b".to_owned()), MontyObject::Repr("bad".to_owned())),
        ]
        .into(),
        frozen: false,
    })
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

// === Function Parameter Shadowing Tests ===
// These tests verify that function parameters properly shadow script inputs with the same name.

#[test]
fn function_param_shadows_input() {
    // Function parameter `x` should shadow the script input `x`
    let code = "
def foo(x):
    return x + 1

foo(x * 2)
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5 (input), foo(x * 2) = foo(10), inside foo x=10 (param), returns 11
    let result = ex.run_no_limits(vec![MontyObject::Int(5)]).unwrap();
    assert_eq!(result, MontyObject::Int(11));
}

#[test]
fn function_param_shadows_input_multiple_params() {
    // Multiple function parameters should all shadow their corresponding inputs
    let code = "
def add(x, y):
    return x + y

add(x * 10, y * 100)
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=2, y=3 (inputs), add(20, 300), inside add x=20, y=300, returns 320
    let result = ex
        .run_no_limits(vec![MontyObject::Int(2), MontyObject::Int(3)])
        .unwrap();
    assert_eq!(result, MontyObject::Int(320));
}

#[test]
fn function_param_shadows_input_but_global_accessible() {
    // Function parameter shadows input, but other inputs are still accessible as globals
    let code = "
def foo(x):
    return x + y

foo(100)
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5, y=3 (inputs), foo(100), inside foo x=100 (param), y=3 (global), returns 103
    let result = ex
        .run_no_limits(vec![MontyObject::Int(5), MontyObject::Int(3)])
        .unwrap();
    assert_eq!(result, MontyObject::Int(103));
}

#[test]
fn function_param_shadows_input_accessible_outside() {
    // Script input should still be accessible outside the function that shadows it
    let code = "
def double(x):
    return x * 2

double(10) + x
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5 (input), double(10) = 20, then 20 + x (global) = 20 + 5 = 25
    let result = ex.run_no_limits(vec![MontyObject::Int(5)]).unwrap();
    assert_eq!(result, MontyObject::Int(25));
}

#[test]
fn function_param_with_default_shadows_input() {
    // Function parameter with default should shadow input when called with argument
    let code = "
def foo(x=100):
    return x + 1

foo(x * 2)
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5 (input), foo(10), inside foo x=10 (param), returns 11
    let result = ex.run_no_limits(vec![MontyObject::Int(5)]).unwrap();
    assert_eq!(result, MontyObject::Int(11));
}

#[test]
fn function_uses_input_as_argument() {
    // Input can be passed as argument, and param shadows inside function
    let code = "
def double(x):
    return x * 2

double(x)
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=7 (input), double(7), inside double x=7 (param from arg), returns 14
    let result = ex.run_no_limits(vec![MontyObject::Int(7)]).unwrap();
    assert_eq!(result, MontyObject::Int(14));
}

#[test]
fn function_doesnt_uses_input_as_argument() {
    let code = "
def double(x):
    return x * 2

double(2)
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=7 (input), double(7), inside double x=7 (param from arg), returns 14
    let result = ex.run_no_limits(vec![MontyObject::Int(7)]).unwrap();
    assert_eq!(result, MontyObject::Int(4));
}

#[test]
fn invalid_identifier() {
    let err = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["foo.bar".to_owned()],
        CompileOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message(), Some("Input name 'foo.bar' not a valid identifier"));
}

#[test]
fn invalid_is_keyword() {
    let err = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["async".to_owned()],
        CompileOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message(), Some("Input name 'async' not a valid identifier"));
}
