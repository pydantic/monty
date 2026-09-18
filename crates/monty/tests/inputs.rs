//! Tests for passing input values to the executor.
//!
//! These tests verify that `MontyObject` inputs are correctly converted to `Object`
//! and can be used in Python code execution.

use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject, MontyUuid};

// === Immediate Value Tests ===

#[test]
fn input_int() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::int(42)]).unwrap();
    assert_eq!(result, MontyObject::int(42));
}

#[test]
fn input_int_arithmetic() {
    let mut ex = MontyRun::new(
        "x + 1".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::int(41)]).unwrap();
    assert_eq!(result, MontyObject::int(42));
}

#[test]
fn input_bool_true() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::bool(true)]).unwrap();
    assert_eq!(result, MontyObject::bool(true));
}

#[test]
fn input_bool_false() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::bool(false)]).unwrap();
    assert_eq!(result, MontyObject::bool(false));
}

#[test]
fn input_float() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::float(2.5)]).unwrap();
    assert_eq!(result, MontyObject::float(2.5));
}

#[test]
fn input_none() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::none()]).unwrap();
    assert_eq!(result, MontyObject::none());
}

#[test]
fn input_ellipsis() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::ellipsis()]).unwrap();
    assert_eq!(result, MontyObject::ellipsis());
}

// === Heap-Allocated Value Tests ===

#[test]
fn input_string() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::string("hello".to_string())])
        .unwrap();
    assert_eq!(result, MontyObject::string("hello".to_string()));
}

#[test]
fn input_string_concat() {
    let mut ex = MontyRun::new(
        "x + ' world'".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::string("hello".to_string())])
        .unwrap();
    assert_eq!(result, MontyObject::string("hello world".to_string()));
}

#[test]
fn input_bytes() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::bytes(vec![1, 2, 3])]).unwrap();
    assert_eq!(result, MontyObject::bytes(vec![1, 2, 3]));
}

#[test]
fn input_list() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::list([MontyObject::int(1), MontyObject::int(2)])])
        .unwrap();
    assert_eq!(result, MontyObject::list([MontyObject::int(1), MontyObject::int(2)]));
}

#[test]
fn input_list_append() {
    let mut ex = MontyRun::new(
        "x.append(3)\nx".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::list([MontyObject::int(1), MontyObject::int(2)])])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::list([MontyObject::int(1), MontyObject::int(2), MontyObject::int(3)])
    );
}

#[test]
fn input_tuple() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::tuple([
            MontyObject::int(1),
            MontyObject::string("two".to_string()),
        ])])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::tuple([MontyObject::int(1), MontyObject::string("two".to_string())])
    );
}

#[test]
fn input_dict() {
    let map = vec![(MontyObject::string("a".to_string()), MontyObject::int(1))];

    let mut ex = MontyRun::new(
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
        MontyObject::dict([(MontyObject::string("a".to_string()), MontyObject::int(1))])
    );
}

#[test]
fn input_dict_get() {
    let map = vec![(MontyObject::string("key".to_string()), MontyObject::int(42))];

    let mut ex = MontyRun::new(
        "x['key']".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::dict(map)]).unwrap();
    assert_eq!(result, MontyObject::int(42));
}

// === Multiple Inputs ===

#[test]
fn multiple_inputs_two() {
    let mut ex = MontyRun::new(
        "x + y".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::int(10), MontyObject::int(32)])
        .unwrap();
    assert_eq!(result, MontyObject::int(42));
}

#[test]
fn multiple_inputs_three() {
    let mut ex = MontyRun::new(
        "x + y + z".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned(), "z".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::int(10), MontyObject::int(20), MontyObject::int(12)])
        .unwrap();
    assert_eq!(result, MontyObject::int(42));
}

#[test]
fn multiple_inputs_mixed_types() {
    // Create a list from two inputs
    let mut ex = MontyRun::new(
        "[x, y]".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::int(1), MontyObject::string("two".to_string())])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::list([MontyObject::int(1), MontyObject::string("two".to_string())])
    );
}

// === Edge Cases ===

#[test]
fn no_inputs() {
    let mut ex = MontyRun::new("42".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::int(42));
}

#[test]
fn nested_list() {
    let mut ex = MontyRun::new(
        "x[0][1]".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::list([MontyObject::list([
            MontyObject::int(1),
            MontyObject::int(2),
        ])])])
        .unwrap();
    assert_eq!(result, MontyObject::int(2));
}

#[test]
fn empty_list_input() {
    let mut ex = MontyRun::new(
        "len(x)".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::list([])]).unwrap();
    assert_eq!(result, MontyObject::int(0));
}

#[test]
fn empty_string_input() {
    let mut ex = MontyRun::new(
        "len(x)".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::string(String::new())]).unwrap();
    assert_eq!(result, MontyObject::int(0));
}

// === Exception Input Tests ===

#[test]
fn input_exception() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::exception(
            ExcType::ValueError,
            Some("test message".to_string()),
        )])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::exception(ExcType::ValueError, Some("test message".to_string()))
    );
}

#[test]
fn input_exception_no_arg() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::exception(ExcType::TypeError, None)])
        .unwrap();
    assert_eq!(result, MontyObject::exception(ExcType::TypeError, None));
}

#[test]
fn input_exception_in_list() {
    let mut ex = MontyRun::new(
        "x[0]".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::list([MontyObject::exception(
            ExcType::KeyError,
            Some("key".to_string()),
        )])])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::exception(ExcType::KeyError, Some("key".to_string()))
    );
}

#[test]
fn input_exception_raise() {
    // Test that an exception passed as input can be raised
    let mut ex = MontyRun::new(
        "raise x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::exception(
        ExcType::ValueError,
        Some("input error".to_string()),
    )]);
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message(), Some("input error"));
}

// === Invalid Input Tests ===

#[test]
fn invalid_input_repr() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![MontyObject::repr("some repr".to_string())]);
    assert!(result.is_err(), "Repr should not be a valid input");
}

#[test]
fn invalid_input_repr_nested_in_list() {
    let mut ex = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // Repr nested inside a list should still be invalid
    let result = ex.run_no_limits(vec![MontyObject::list([MontyObject::repr("nested repr".to_string())])]);
    assert!(result.is_err(), "Repr nested in list should be invalid");
}

// === Error-Path Cleanup Tests ===
// An invalid element placed *after* elements that allocate heap values: the
// partially-built container must release the already-converted values (the
// `memory-model-checks` feature panics on any missed drop) and report the error.

/// Runs `x` bound to `input`, returning the conversion/execution result.
fn run_input(input: MontyObject) -> Result<MontyObject, monty_types::MontyException> {
    let mut ex = MontyRun::new(
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
    MontyObject::list([MontyObject::int(1)])
}

#[test]
fn invalid_input_repr_in_list_after_heap_values() {
    let err = run_input(MontyObject::list([heap_element(), MontyObject::repr("bad".to_owned())])).unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_tuple_after_heap_values() {
    let err = run_input(MontyObject::tuple([
        heap_element(),
        MontyObject::repr("bad".to_owned()),
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
    let err = run_input(MontyObject::dict(vec![
        (MontyObject::string("a".to_owned()), heap_element()),
        (MontyObject::string("b".to_owned()), MontyObject::repr("bad".to_owned())),
    ]))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_set_after_heap_values() {
    let err = run_input(MontyObject::set([
        MontyObject::string("heap string".to_owned()),
        MontyObject::repr("bad".to_owned()),
    ]))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_frozenset_after_heap_values() {
    let err = run_input(MontyObject::frozenset([
        MontyObject::string("heap string".to_owned()),
        MontyObject::repr("bad".to_owned()),
    ]))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

#[test]
fn invalid_input_repr_in_namedtuple_after_heap_values() {
    let err = run_input(MontyObject::named_tuple(
        "nt".to_owned(),
        vec!["a".to_owned(), "b".to_owned()],
        vec![heap_element(), MontyObject::repr("bad".to_owned())],
    ))
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
    let err = run_input(MontyObject::named_tuple(
        "nt".to_owned(),
        vec!["a".to_owned()],
        vec![MontyObject::int(1), MontyObject::int(2)],
    ))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: NamedTuple field_names and values must have the same length")
    );
}

#[test]
fn invalid_input_repr_in_class_instance_attrs() {
    let err = run_input(MontyObject::class_instance(
        MontyObject::class_type("Point", MontyUuid::from_u128(1), true, false, []),
        MontyUuid::from_u128(2),
        [
            (MontyObject::string("a".to_owned()), heap_element()),
            (MontyObject::string("b".to_owned()), MontyObject::repr("bad".to_owned())),
        ],
    ))
    .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
}

/// A host `Point` class-type input carrying one eager class attr (`data`, a
/// mutable list) — the shape used by the host-class-type tests below.
fn host_class_type_input() -> MontyObject {
    MontyObject::class_type(
        "Point",
        MontyUuid::from_u128(1),
        true,
        false,
        [(
            MontyObject::string("data".to_owned()),
            MontyObject::list([MontyObject::int(1)]),
        )],
    )
}

#[test]
fn type_object_missing_attr_uses_type_object_wording() {
    // Non-iterative `run` has no host to answer the AttrLookup suspension, so
    // it must raise the AttributeError locally — with CPython's type-object
    // wording, since the receiver is a class type.
    let mut ex = MontyRun::new(
        "x.missing".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex.run_no_limits(vec![host_class_type_input()]).unwrap_err();
    assert_eq!(err.message(), Some("type object 'Point' has no attribute 'missing'"));
}

#[test]
fn host_class_type_attr_cycle_is_collected() {
    // Sandbox code can reach a container in a host class type's eager attrs
    // and close a cycle back to the type object. The run must still complete
    // and tear down cleanly — under `memory-model-checks` this verifies the
    // GC traces and frees the HostClassType's attrs (a missed
    // `for_each_child_id`/`py_dec_ref_ids` arm leaks or corrupts refcounts).
    let code = "
x.data.append(x)
x = None
1
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex.run_no_limits(vec![host_class_type_input()]).unwrap();
    assert_eq!(result, MontyObject::int(1));
}

#[test]
fn host_class_instance_type_cycle_is_collected() {
    // An instance owns its class entry, whose eager attrs can hold a container
    // the sandbox reaches: instance -> type -> attrs -> instance is a cycle
    // the collector must trace through the new `HostClass` -> class edge.
    let code = "
x.data.append(p)
x = p = None
1
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned(), "p".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // the instance's class branch carries no attrs of its own
    let instance = MontyObject::class_instance(
        MontyObject::class_type("Point", MontyUuid::from_u128(1), true, false, []),
        MontyUuid::from_u128(2),
        [],
    );
    let result = ex.run_no_limits(vec![host_class_type_input(), instance]).unwrap();
    assert_eq!(result, MontyObject::int(1));
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
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5 (input), foo(x * 2) = foo(10), inside foo x=10 (param), returns 11
    let result = ex.run_no_limits(vec![MontyObject::int(5)]).unwrap();
    assert_eq!(result, MontyObject::int(11));
}

#[test]
fn function_param_shadows_input_multiple_params() {
    // Multiple function parameters should all shadow their corresponding inputs
    let code = "
def add(x, y):
    return x + y

add(x * 10, y * 100)
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=2, y=3 (inputs), add(20, 300), inside add x=20, y=300, returns 320
    let result = ex
        .run_no_limits(vec![MontyObject::int(2), MontyObject::int(3)])
        .unwrap();
    assert_eq!(result, MontyObject::int(320));
}

#[test]
fn function_param_shadows_input_but_global_accessible() {
    // Function parameter shadows input, but other inputs are still accessible as globals
    let code = "
def foo(x):
    return x + y

foo(100)
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5, y=3 (inputs), foo(100), inside foo x=100 (param), y=3 (global), returns 103
    let result = ex
        .run_no_limits(vec![MontyObject::int(5), MontyObject::int(3)])
        .unwrap();
    assert_eq!(result, MontyObject::int(103));
}

#[test]
fn function_param_shadows_input_accessible_outside() {
    // Script input should still be accessible outside the function that shadows it
    let code = "
def double(x):
    return x * 2

double(10) + x
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5 (input), double(10) = 20, then 20 + x (global) = 20 + 5 = 25
    let result = ex.run_no_limits(vec![MontyObject::int(5)]).unwrap();
    assert_eq!(result, MontyObject::int(25));
}

#[test]
fn function_param_with_default_shadows_input() {
    // Function parameter with default should shadow input when called with argument
    let code = "
def foo(x=100):
    return x + 1

foo(x * 2)
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=5 (input), foo(10), inside foo x=10 (param), returns 11
    let result = ex.run_no_limits(vec![MontyObject::int(5)]).unwrap();
    assert_eq!(result, MontyObject::int(11));
}

#[test]
fn function_uses_input_as_argument() {
    // Input can be passed as argument, and param shadows inside function
    let code = "
def double(x):
    return x * 2

double(x)
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=7 (input), double(7), inside double x=7 (param from arg), returns 14
    let result = ex.run_no_limits(vec![MontyObject::int(7)]).unwrap();
    assert_eq!(result, MontyObject::int(14));
}

#[test]
fn function_doesnt_uses_input_as_argument() {
    let code = "
def double(x):
    return x * 2

double(2)
";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    // x=7 (input), double(7), inside double x=7 (param from arg), returns 14
    let result = ex.run_no_limits(vec![MontyObject::int(7)]).unwrap();
    assert_eq!(result, MontyObject::int(4));
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
