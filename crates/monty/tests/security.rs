use insta::assert_snapshot;
use monty::MontyRun;

#[test]
fn deeply_nested_parentheses_do_not_stack_overflow() {
    let depth = 5000;
    let mut code = String::with_capacity(depth * 2 + 1);
    for _ in 0..depth {
        code.push('(');
    }
    code.push('1');
    for _ in 0..depth {
        code.push(')');
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected parse error for deeply nested parentheses");
    assert_snapshot!(err.message().unwrap_or(""), @"Source is too deeply nested");
}

/// Ruff parses postfix attribute access iteratively, so `a.x.x.x...` is not caught
/// by ruff's recursion limit even though the resulting AST is deeply nested.
/// Monty's own AST walk must reject it to avoid a stack overflow when recursing
/// through the `Attribute` chain.
#[test]
fn deeply_nested_attribute_access_does_not_stack_overflow() {
    let depth = 200;
    let mut code = String::with_capacity(depth * 2 + 1);
    code.push('a');
    for _ in 0..depth {
        code.push_str(".x");
    }
    let result = MontyRun::new(code, "test.py", vec![]);
    let err = result.expect_err("expected parse error for deeply nested attribute access");
    assert_snapshot!(err.message().unwrap_or(""), @"Source is too deeply nested");
}
