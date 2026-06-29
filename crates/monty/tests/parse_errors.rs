use std::fmt::Write;

use insta::assert_snapshot;
use monty::{ExcType, MontyException, MontyRun};

/// Helper to extract the exception from a parse error.
fn get_parse_err(code: impl Into<String>) -> MontyException {
    let result = MontyRun::new(code.into(), "test.py", vec![]);
    result.expect_err("expected parse error")
}

#[test]
fn complex_numbers_return_not_implemented_error() {
    let err = get_parse_err("1 + 2j");
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    assert_snapshot!(err.message().unwrap(), @"The monty syntax parser does not yet support complex constants");
}

#[test]
fn yield_expressions_return_not_implemented_error() {
    let err = get_parse_err("def foo():\n    yield 1");
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    assert_snapshot!(err.message().unwrap(), @"The monty syntax parser does not yet support yield expressions");
}

#[test]
fn classes_return_not_implemented_error() {
    let err = get_parse_err("class Foo: pass");
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    assert_snapshot!(err.message().unwrap(), @"The monty syntax parser does not yet support class definitions");
}

#[test]
fn unknown_imports_compile_successfully_error_deferred_to_runtime() {
    // Unknown modules (not sys, typing, os, etc.) compile successfully.
    // The ModuleNotFoundError is deferred to runtime, allowing TYPE_CHECKING
    // imports to work without causing compile-time errors.
    let result = MontyRun::new("import foobar".to_owned(), "test.py", vec![]);
    assert!(result.is_ok(), "unknown import should compile successfully");
}

#[test]
fn async_with_statement_returns_not_implemented_error() {
    // Plain `with` is supported (see `test_cases/with__all.py`); only `async with`
    // is still rejected at parse time.
    let result = MontyRun::new(
        "async def f():\n    async with open('f') as g: pass\n".to_owned(),
        "test.py",
        vec![],
    );
    let err = result.expect_err("expected parse error");
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
}

#[test]
fn error_display_format() {
    // Verify the Display format matches Python's exception output with traceback
    let err = get_parse_err("1 + 2j");
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        1 + 2j
            ~~
    NotImplementedError: The monty syntax parser does not yet support complex constants
    "#);
}

/// Tests that syntax errors return `SyntaxError` exceptions.

#[test]
fn invalid_fstring_format_spec_returns_syntax_error() {
    let err = get_parse_err("f'{1:10xyz}'");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Invalid format specifier '10xyz'");
}

#[test]
fn invalid_fstring_format_spec_str_returns_syntax_error() {
    let err = get_parse_err("f'{\"hello\":abc}'");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Invalid format specifier 'abc'");
}

#[test]
fn format_spec_width_overflow_returns_syntax_error() {
    // 22 nines overflows usize; verify the parser surfaces this rather than
    // silently clamping to 0.
    let result = MontyRun::new("f'{42:9999999999999999999999d}'".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected parse error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert!(
        exc.message().is_some_and(|m| m.contains("overflows usize")),
        "message should mention overflow, got: {exc}"
    );
}

#[test]
fn syntax_error_display_format() {
    let err = get_parse_err("f'{1:10xyz}'");
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 1
        f'{1:10xyz}'
             ~~~~~
    SyntaxError: Invalid format specifier '10xyz'
    "#);
}

#[test]
fn deeply_nested_tuples_exceed_limit() {
    // Build nested tuple like ((((x,),),),) with depth > 200
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("({code},)");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
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
    let result = MontyRun::new(code, "test.py", vec![]);
    assert!(result.is_ok(), "nesting within limit should succeed");
}

#[test]
fn deeply_nested_unpack_assignment_exceeds_limit() {
    // Build nested unpack assignment like ((((x,),),),) = value with depth > 200
    let mut target = "x".to_string();
    for _ in 0..250 {
        target = format!("({target},)");
    }
    let err = get_parse_err(format!("{target} = (1,)"));
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_lists_exceed_limit() {
    // Build nested list like [[[[[x]]]]]
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("[{code}]");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_dicts_exceed_limit() {
    // Build nested dict like {'a': {'a': {'a': ...}}}
    let mut code = "1".to_string();
    for _ in 0..250 {
        code = format!("{{'a': {code}}}");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_function_calls_exceed_limit() {
    // Build nested calls like f(f(f(f(x))))
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("f({code})");
    }
    let err = get_parse_err(format!("def f(x): return x\n{code}"));
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_binary_ops_exceed_limit() {
    // Build nested binary ops like ((((x + 1) + 1) + 1) + 1)
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("({code} + 1)");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_ternary_if_exceed_limit() {
    // Build nested ternary like (1 if (1 if (1 if ... else 0) else 0) else 0)
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("(1 if {code} else 0)");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_subscripts_exceed_limit() {
    // Build nested subscripts like a[b[c[d[...]]]]
    let mut code = "0".to_string();
    for _ in 0..250 {
        code = format!("a[{code}]");
    }
    let err = get_parse_err(format!("a = [1]\n{code}"));
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_list_comprehension_exceed_limit() {
    // Build nested list comprehension like [x for x in [y for y in [...]]]
    let mut code = "[1]".to_string();
    for _ in 0..250 {
        code = format!("[x for x in {code}]");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
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
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
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
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
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
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
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
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
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
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_attribute_access_exceed_limit() {
    // Build chained attribute access like a.b.c.d.e...
    let mut code = "a".to_string();
    for _ in 0..250 {
        code.push_str(".x");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_lambdas_exceed_limit() {
    // Build nested lambdas like (lambda: (lambda: (lambda: ... x)))
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("(lambda: {code})");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_unary_not_exceed_limit() {
    // Build nested not operators like not (not (not ... True))
    let mut code = "True".to_string();
    for _ in 0..250 {
        code = format!("not ({code})");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_unary_minus_exceed_limit() {
    // Build nested unary minus like -(-(-... 1))
    let mut code = "1".to_string();
    for _ in 0..250 {
        code = format!("-({code})");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_walrus_operator_exceed_limit() {
    // Build nested walrus operators like (a := (b := (c := ... 1)))
    let mut code = "1".to_string();
    for i in 0..250 {
        code = format!("(x{i} := {code})");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_await_exceed_limit() {
    // Build nested await like await (await (await ... x))
    // We need this in an async function context
    let mut code = "x".to_string();
    for _ in 0..250 {
        code = format!("await ({code})");
    }
    let err = get_parse_err(format!("async def f():\n    {code}"));
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_boolean_and_exceed_limit() {
    // Build nested boolean and like (True and (True and (True and ...)))
    let mut code = "True".to_string();
    for _ in 0..250 {
        code = format!("(True and {code})");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn deeply_nested_boolean_or_exceed_limit() {
    // Build nested boolean or like (False or (False or (False or ...)))
    let mut code = "True".to_string();
    for _ in 0..250 {
        code = format!("(False or {code})");
    }
    let err = get_parse_err(code);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

// === Runtime NotImplementedError tests ===
// These test that unimplemented features return proper errors instead of panicking.

/// Helper to run code and get the exception from a runtime error.
fn run_and_get_err(code: &str) -> MontyException {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).expect("should parse");
    runner.run_no_limits(vec![]).expect_err("expected runtime error")
}

#[test]
fn matrix_multiplication_returns_not_implemented_error() {
    // The @ operator (matrix multiplication) is not supported at runtime
    let err = run_and_get_err("1 @ 2");
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
}

#[test]
fn matrix_multiplication_augmented_assignment_returns_syntax_error() {
    // The @= operator (augmented matrix multiplication) is not supported at compile time
    let err = get_parse_err("a = 1\na @= 2");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"matrix multiplication augmented assignment (@=) is not yet supported");
}

#[test]
fn del_statement_returns_not_implemented_error() {
    // The del statement is not supported at parse time
    let err = get_parse_err("x = 1\ndel x");
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    assert_snapshot!(err.message().unwrap(), @"The monty syntax parser does not yet support the 'del' statement");
}

#[test]
fn duplicate_positional_parameter_returns_syntax_error() {
    // https://github.com/pydantic/monty/issues/377
    //
    // Ruff's parser accepts `def f(x, x)` though CPython rejects it at compile time.
    // Without an explicit check, `Prepare::new_function` would size the frame from
    // the unique-name count (HashMap::len) while resolving the duplicate to a
    // positional NamespaceId that points past the allocated stack region, panicking
    // `load_local` at call time.
    let result = MontyRun::new("def f(x, x): return x\nf(1, 2)".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected compile error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_eq!(exc.message(), Some("duplicate argument 'x' in function definition"));
}

#[test]
fn duplicate_keyword_only_parameter_returns_syntax_error() {
    let result = MontyRun::new("def f(*, x, x): return x".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected compile error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_eq!(exc.message(), Some("duplicate argument 'x' in function definition"));
}

#[test]
fn duplicate_mixed_positional_and_keyword_only_parameter_returns_syntax_error() {
    let result = MontyRun::new("def f(x, *, x=1): return x".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected compile error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_eq!(exc.message(), Some("duplicate argument 'x' in function definition"));
}

#[test]
fn duplicate_lambda_parameter_returns_syntax_error() {
    let result = MontyRun::new("f = lambda x, x: x".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected compile error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_eq!(exc.message(), Some("duplicate argument 'x' in function definition"));
}

#[test]
fn long_source_line_does_not_overflow_column() {
    // https://github.com/pydantic/monty/issues/341
    //
    // (code locations was previously limited to u16 values for line / col)
    let code = format!("x = \"{}\"\nassert len(x) == 65530", "a".repeat(65530));
    let run = MontyRun::new(code, "test.py", vec![]).expect("long line should parse without panicking");
    let result = run.run_no_limits(vec![]);
    assert!(result.is_ok(), "long line should run: {result:?}");
}

// === Parse error messages must not leak ruff_python_ast Debug formatting ===
//
// These snapshot the full error message for each trigger so any future
// regression that reintroduces Debug formatting of AST nodes (struct
// names, `node_index`, `range`, `ctx: Store`, etc.) fails the snapshot
// diff loudly.

#[test]
fn starred_name_target_has_clean_message() {
    // `*a = [1, 2]`: Ruff parses the LHS as a bare starred target, which
    // Monty rejects at `parse_identifier`.
    let result = MontyRun::new("*a = [1, 2]".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected parse error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(exc.message().expect("has message"), @"Expected name, got starred expression");
}

#[test]
fn starred_attribute_target_has_clean_message() {
    // `*x.y = 1`: starred target wrapping an attribute. Same rejection
    // path, different inner node shape.
    let result = MontyRun::new("*x.y = 1".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected parse error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(exc.message().expect("has message"), @"Expected name, got starred expression");
}

#[test]
fn starred_subscript_target_has_clean_message() {
    // `*x[0] = 1`: starred target wrapping a subscript.
    let result = MontyRun::new("*x[0] = 1".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected parse error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(exc.message().expect("has message"), @"Expected name, got starred expression");
}

#[test]
fn for_loop_attribute_target_has_clean_message() {
    // `for x.y in [1]: pass`: attribute as a for-loop target. CPython
    // accepts this; Monty currently rejects at `parse_unpack_target_impl`.
    // That rejection of valid Python is a separate issue; this test locks
    // only that the error message does not leak `ExprAttribute` Debug.
    let result = MontyRun::new("for x.y in [1]: pass".to_owned(), "test.py", vec![]);
    let exc = result.expect_err("expected parse error");
    assert_eq!(exc.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(exc.message().expect("has message"), @"invalid unpacking target: attribute");
}

#[test]
fn many_elif_clauses_exceed_limit() {
    // A long flat chain of `elif` clauses folds into a deeply right-nested
    // `Node::If` tree that the prepare and compile phases walk recursively.
    // Each clause is counted against the parser's nesting-depth budget so the
    // result is a SyntaxError rather than a native stack overflow downstream.
    let mut code = "if 0:\n    pass\n".to_owned();
    for _ in 0..400 {
        code.push_str("elif 0:\n    pass\n");
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected parse error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("Source is too deeply nested"),
        "error message should match the parser depth-limit message, got: {:?}",
        err.message()
    );
}

#[test]
fn moderate_elif_chain_within_limit() {
    let mut code = "if 0:\n    pass\n".to_owned();
    for _ in 0..20 {
        code.push_str("elif 0:\n    pass\n");
    }
    code.push_str("else:\n    pass\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    assert!(result.is_ok(), "moderate elif chain should succeed: {result:?}");
}

#[test]
fn many_with_items_exceed_limit() {
    // A single syntactically-flat `with` statement with many items lowers to
    // nested `Node::With` values. The synthetic nesting must consume the same
    // parser depth budget as explicit nesting so we fail with SyntaxError
    // instead of overflowing the host stack in prepare/compile.
    let mut code = "with 0".to_owned();
    for _ in 0..400 {
        code.push_str(", 0");
    }
    code.push_str(":\n    pass\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected parse error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("Source is too deeply nested"),
        "error message should match the parser depth-limit message, got: {:?}",
        err.message()
    );
}

#[test]
fn moderate_with_items_within_limit() {
    let mut code = "with 0".to_owned();
    for _ in 0..20 {
        code.push_str(", 0");
    }
    code.push_str(":\n    pass\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    assert!(result.is_ok(), "moderate with-item chain should succeed: {result:?}");
}

#[test]
fn many_bool_op_operands_exceed_limit() {
    // A long chain of `and`/`or` operands folds into a deeply right-nested
    // `Expr::Op` tree. Each fold step is counted against the parser's
    // nesting-depth budget.
    let mut code = "x = 1".to_owned();
    for _ in 0..400 {
        code.push_str(" and 1");
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected parse error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
}

#[test]
fn moderate_bool_op_chain_within_limit() {
    let mut code = "1".to_owned();
    for _ in 0..20 {
        code.push_str(" and 1");
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    assert!(result.is_ok(), "moderate bool-op chain should succeed: {result:?}");
}

#[test]
fn function_with_too_many_locals_and_except_as_returns_syntax_error() {
    let mut code = "def f():\n".to_owned();
    for i in 0..256 {
        writeln!(code, "    l{i} = 0").unwrap();
    }
    code.push_str("    try:\n        1/0\n    except Exception as e:\n        pass\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("cannot delete local variable in function with more than 256 locals (slot 256)"),
    );
}

#[test]
fn function_with_oversized_jump_offset_returns_syntax_error() {
    let mut code = "def f(x):\n    if x:\n".to_owned();
    for i in 0..20_000 {
        writeln!(code, "        a{i} = 1").unwrap();
    }
    code.push_str("    return 0\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message(), Some("function too large: jump offset exceeds i16 range"));
}

#[test]
fn module_with_too_many_names_returns_syntax_error() {
    // 70 000 distinct top-level names is enough to overflow u16 even after
    // any future small per-module reservations.
    let mut code = String::with_capacity(700_000);
    for i in 0..70_000 {
        writeln!(code, "a{i} = 1").unwrap();
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("too many distinct names in scope; maximum is 65536 per scope"),
    );
}

#[test]
fn module_with_too_many_interned_strings_returns_syntax_error() {
    // 60 000 distinct attribute references push the user-intern pool past its
    // `u16::MAX - INTERN_STRING_ID_OFFSET` cap.
    let mut code = "x = None\n".to_owned();
    for i in 0..60_000 {
        writeln!(code, "x.a{i}").unwrap();
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("module has too many distinct names; the bytecode format supports up to 65536 interned strings"),
    );
}

#[test]
fn oversized_tuple_literal_returns_syntax_error() {
    let mut code = "x = (".to_owned();
    for _ in 0..70_000 {
        code.push_str("1, ");
    }
    code.push_str(")\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("function too large: required stack exceeds u16::MAX")
    );
}

#[test]
fn oversized_unpacking_call_returns_syntax_error() {
    let mut code = "def f(*args): return 0\nxs = ()\nf(".to_owned();
    for _ in 0..70_000 {
        code.push_str("1, ");
    }
    code.push_str("*xs)\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message(),
        Some("function too large: required stack exceeds u16::MAX")
    );
}

#[test]
fn function_with_too_many_defaults_returns_syntax_error() {
    let mut code = "def f(".to_owned();
    for i in 0..256 {
        if i > 0 {
            code.push_str(", ");
        }
        write!(code, "a{i}=0").unwrap();
    }
    code.push_str("): pass\n");
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message(), Some("more than 255 default parameter values (256)"));
}

#[test]
fn function_with_too_many_closure_variables_returns_syntax_error() {
    // Each `xN` reference in `inner` captures the enclosing local as a free
    // variable, so 256 distinct references push `MakeClosure`'s cell-count
    // operand past `u8`. Flat per-statement references avoid hitting the
    // parser's nested-parens depth limit before the closure-count limit.
    let mut code = "def outer():\n".to_owned();
    for i in 0..256 {
        writeln!(code, "    x{i} = 0").unwrap();
    }
    code.push_str("    def inner():\n");
    for i in 0..256 {
        writeln!(code, "        _ = x{i}").unwrap();
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message(), Some("more than 255 closure variables (256)"));
}
