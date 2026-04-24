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
