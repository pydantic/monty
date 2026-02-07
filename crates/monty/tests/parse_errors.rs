use std::fmt::Write;

use monty::{ExcType, MontyException, MontyRun};

/// Helper to extract the exception type from a parse error.
fn get_exc_type(result: Result<MontyRun, MontyException>) -> ExcType {
    let err = result.expect_err("expected parse error");
    err.exc_type()
}

#[test]
fn complex_numbers_return_not_implemented_error() {
    let result = MontyRun::new("1 + 2j".to_owned(), "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::NotImplementedError);
}

#[test]
fn complex_numbers_have_descriptive_message() {
    let result = MontyRun::new("1 + 2j".to_owned(), "test.py", vec![], vec![]);
    let exc = result.expect_err("expected parse error");
    assert!(
        exc.message().is_some_and(|m| m.contains("complex")),
        "message should mention 'complex', got: {exc}"
    );
}

#[test]
fn yield_expressions_return_not_implemented_error() {
    // Yield expressions are not supported and fail at parse time
    let result = MontyRun::new("def foo():\n    yield 1".to_owned(), "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::NotImplementedError);
    let result = MontyRun::new("def foo():\n    yield 1".to_owned(), "test.py", vec![], vec![]);
    let exc = result.expect_err("expected parse error");
    assert!(
        exc.message().is_some_and(|m| m.contains("yield")),
        "message should mention 'yield', got: {exc}"
    );
}

#[test]
fn classes_return_not_implemented_error() {
    let result = MontyRun::new("class Foo: pass".to_owned(), "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::NotImplementedError);
}

#[test]
fn unknown_imports_compile_successfully_error_deferred_to_runtime() {
    // Unknown modules (not sys, typing, os, etc.) compile successfully.
    // The ModuleNotFoundError is deferred to runtime, allowing TYPE_CHECKING
    // imports to work without causing compile-time errors.
    let result = MontyRun::new("import foobar".to_owned(), "test.py", vec![], vec![]);
    assert!(result.is_ok(), "unknown import should compile successfully");
}

#[test]
fn with_statement_returns_not_implemented_error() {
    let result = MontyRun::new("with open('f') as f: pass".to_owned(), "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::NotImplementedError);
}

#[test]
fn error_display_format() {
    // Verify the Display format matches Python's exception output with traceback
    let result = MontyRun::new("1 + 2j".to_owned(), "test.py", vec![], vec![]);
    let err = result.expect_err("expected parse error");
    let display = err.to_string();
    // Should start with traceback header
    assert!(
        display.starts_with("Traceback (most recent call last):"),
        "display should start with 'Traceback': got: {display}"
    );
    // Should contain the file/line info
    assert!(
        display.contains("File \"test.py\", line 1"),
        "display should contain file location, got: {display}"
    );
    // Should end with NotImplementedError message
    assert!(
        display.contains("NotImplementedError:"),
        "display should contain 'NotImplementedError:', got: {display}"
    );
    assert!(
        display.contains("monty syntax parser"),
        "display should mention 'monty syntax parser', got: {display}"
    );
}

/// Tests that syntax errors return `SyntaxError` exceptions.

#[test]
fn invalid_fstring_format_spec_returns_syntax_error() {
    let result = MontyRun::new("f'{1:10xyz}'".to_owned(), "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn invalid_fstring_format_spec_str_returns_syntax_error() {
    let result = MontyRun::new("f'{\"hello\":abc}'".to_owned(), "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn syntax_error_display_format() {
    let result = MontyRun::new("f'{1:10xyz}'".to_owned(), "test.py", vec![], vec![]);
    let err = result.expect_err("expected parse error");
    let display = err.to_string();
    assert!(
        display.contains("SyntaxError:"),
        "display should contain 'SyntaxError:', got: {display}"
    );
}

#[test]
fn deeply_nested_tuples_exceed_limit() {
    // Build nested tuple like ((((x,),),),) with depth > 200
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("({code},)");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    let err = result.expect_err("expected parse error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("too many nested parentheses"),
        "error message should match CPython, got: {:?}",
        err.message()
    );
}

#[test]
fn nested_tuples_within_limit_succeed() {
    // Build nested tuple with depth = 20, which is well under the 200 limit.
    // We use a small value because the ruff parser uses significant stack
    // space per nesting level in debug builds.
    let mut code = "x".to_string();
    for _ in 0..20 {
        code = format!("({code},)");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert!(result.is_ok(), "nesting within limit should succeed");
}

#[test]
fn deeply_nested_unpack_assignment_exceeds_limit() {
    // Build nested unpack assignment like ((((x,),),),) = value with depth > 200
    let mut target = "x".to_string();
    for _ in 0..250 {
        target = format!("({target},)");
    }
    let code = format!("{target} = (1,)");
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    let err = result.expect_err("expected parse error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("too many nested parentheses"),
        "error message should match CPython, got: {:?}",
        err.message()
    );
}

#[test]
fn deeply_nested_lists_exceed_limit() {
    // Build nested list like [[[[[x]]]]]
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("[{code}]");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_dicts_exceed_limit() {
    // Build nested dict like {'a': {'a': {'a': ...}}}
    let mut code = "1".to_string();
    for _ in 0..250 {
        code = format!("{{'a': {code}}}");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_function_calls_exceed_limit() {
    // Build nested calls like f(f(f(f(x))))
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("f({code})");
    }
    let code = format!("def f(x): return x\n{code}");
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_binary_ops_exceed_limit() {
    // Build nested binary ops like ((((x + 1) + 1) + 1) + 1)
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("({code} + 1)");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_ternary_if_exceed_limit() {
    // Build nested ternary like (1 if (1 if (1 if ... else 0) else 0) else 0)
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("(1 if {code} else 0)");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_subscripts_exceed_limit() {
    // Build nested subscripts like a[b[c[d[...]]]]
    let mut code = "0".to_string();
    for _ in 0..250 {
        code = format!("a[{code}]");
    }
    let code = format!("a = [1]\n{code}");
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_list_comprehension_exceed_limit() {
    // Build nested list comprehension like [x for x in [y for y in [...]]]
    let mut code = "[1]".to_string();
    for _ in 0..250 {
        code = format!("[x for x in {code}]");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_if_statements_exceed_limit() {
    // Build nested if statements
    let mut code = "x = 1\n".to_string();
    for i in 0..250 {
        let indent = "    ".repeat(i);
        writeln!(code, "{indent}if 1:").unwrap();
    }
    write!(code, "{}pass", "    ".repeat(250)).unwrap();
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_while_loops_exceed_limit() {
    // Build nested while loops
    let mut code = String::new();
    for i in 0..250 {
        let indent = "    ".repeat(i);
        writeln!(code, "{indent}while True:").unwrap();
    }
    write!(code, "{}break", "    ".repeat(250)).unwrap();
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_for_loops_exceed_limit() {
    // Build nested for loops
    let mut code = String::new();
    for i in 0..250 {
        let indent = "    ".repeat(i);
        writeln!(code, "{indent}for x in [1]:").unwrap();
    }
    write!(code, "{}pass", "    ".repeat(250)).unwrap();
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_try_except_exceed_limit() {
    // Build nested try/except blocks
    let mut code = String::new();
    for i in 0..250 {
        let indent = "    ".repeat(i);
        writeln!(code, "{indent}try:").unwrap();
    }
    writeln!(code, "{}pass", "    ".repeat(250)).unwrap();
    for i in (0..250).rev() {
        let indent = "    ".repeat(i);
        writeln!(code, "{indent}except: pass").unwrap();
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_function_defs_exceed_limit() {
    // Build nested function definitions
    let mut code = String::new();
    for i in 0..250 {
        let indent = "    ".repeat(i);
        writeln!(code, "{indent}def f():").unwrap();
    }
    write!(code, "{}pass", "    ".repeat(250)).unwrap();
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_attribute_access_exceed_limit() {
    // Build chained attribute access like a.b.c.d.e...
    let mut code = "a".to_string();
    for _ in 0..250 {
        code.push_str(".x");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_lambdas_exceed_limit() {
    // Build nested lambdas like (lambda: (lambda: (lambda: ... x)))
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("(lambda: {code})");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_unary_not_exceed_limit() {
    // Build nested not operators like not (not (not ... True))
    let mut code = "True".to_string();
    for _ in 0..250 {
        code = format!("not ({code})");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_unary_minus_exceed_limit() {
    // Build nested unary minus like -(-(-... 1))
    let mut code = "1".to_string();
    for _ in 0..250 {
        code = format!("-({code})");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_walrus_operator_exceed_limit() {
    // Build nested walrus operators like (a := (b := (c := ... 1)))
    let mut code = "1".to_string();
    for i in 0..250 {
        code = format!("(x{i} := {code})");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_await_exceed_limit() {
    // Build nested await like await (await (await ... x))
    // We need this in an async function context
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("await ({code})");
    }
    let code = format!("async def f():\n    {code}");
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_boolean_and_exceed_limit() {
    // Build nested boolean and like (True and (True and (True and ...)))
    let mut code = "True".to_string();
    for _ in 0..250 {
        code = format!("(True and {code})");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}

#[test]
fn deeply_nested_boolean_or_exceed_limit() {
    // Build nested boolean or like (False or (False or (False or ...)))
    let mut code = "True".to_string();
    for _ in 0..250 {
        code = format!("(False or {code})");
    }
    let result = MontyRun::new(code, "test.py", vec![], vec![]);
    assert_eq!(get_exc_type(result), ExcType::SyntaxError);
}
