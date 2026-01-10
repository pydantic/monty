//! Tests for bytecode operand overflow limits.
//!
//! These tests verify that the bytecode compiler handles cases where operands
//! exceed the u8/u16 limits of the bytecode encoding:
//!
//! - Local variable slots: Use wide instructions (u16), so up to 65535 locals work
//! - Function call arguments: Limited to 255 (u8 operand) - returns SyntaxError if exceeded
//! - Keyword argument counts: Limited to 255 (u8 operand) - returns SyntaxError if exceeded

use std::fmt::Write;

use monty::{ExcType, MontyRun};

/// Generates Python code with N local variables in a function.
///
/// Creates: `def f(): v0=0; v1=1; ...; v{n-1}={n-1}; return v{n-1}`
fn generate_many_locals(count: usize) -> String {
    let mut code = String::from("def f():\n");
    for i in 0..count {
        writeln!(code, "    v{i} = {i}").unwrap();
    }
    writeln!(code, "    return v{}", count - 1).unwrap();
    code.push_str("f()");
    code
}

/// Generates Python code calling a function with N positional arguments.
///
/// Creates: `def f(*args): return len(args)\nf(0, 1, 2, ..., n-1)`
fn generate_many_positional_args(count: usize) -> String {
    let mut code = String::from("def f(*args): return len(args)\nf(");
    for i in 0..count {
        if i > 0 {
            code.push_str(", ");
        }
        code.push_str(&i.to_string());
    }
    code.push(')');
    code
}

/// Generates Python code calling a function with N keyword arguments.
///
/// Creates: `def f(**kw): return len(kw)\nf(k0=0, k1=1, ..., k{n-1}={n-1})`
fn generate_many_keyword_args(count: usize) -> String {
    let mut code = String::from("def f(**kw): return len(kw)\nf(");
    for i in 0..count {
        if i > 0 {
            code.push_str(", ");
        }
        write!(code, "k{i}={i}").unwrap();
    }
    code.push(')');
    code
}

/// Generates Python code with a function that has N parameters.
///
/// Creates: `def f(p0, p1, ..., p{n-1}): return p{n-1}\nf(0, 1, ..., n-1)`
fn generate_many_parameters(count: usize) -> String {
    let mut code = String::from("def f(");
    for i in 0..count {
        if i > 0 {
            code.push_str(", ");
        }
        write!(code, "p{i}").unwrap();
    }
    code.push_str("):\n");
    writeln!(code, "    return p{}", count - 1).unwrap();
    code.push_str("f(");
    for i in 0..count {
        if i > 0 {
            code.push_str(", ");
        }
        code.push_str(&i.to_string());
    }
    code.push(')');
    code
}

/// Asserts that a MontyRun result is a SyntaxError with a message containing the expected text.
fn assert_syntax_error(result: Result<MontyRun, monty::MontyException>, expected_msg: &str) {
    let err = result.expect_err("expected SyntaxError");
    assert_eq!(
        err.exc_type(),
        ExcType::SyntaxError,
        "expected SyntaxError, got {:?}: {:?}",
        err.exc_type(),
        err.message()
    );
    let msg = err.message().expect("SyntaxError should have message");
    assert!(
        msg.contains(expected_msg),
        "expected message containing '{expected_msg}', got: {msg}"
    );
}

mod local_variable_limits {
    use super::*;

    #[test]
    fn locals_under_u8_limit_succeeds() {
        // 255 locals should work with u8 slots (0-254)
        let code = generate_many_locals(255);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "255 locals should compile successfully");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "255 locals should run successfully");
    }

    #[test]
    fn locals_at_u8_boundary_succeeds() {
        // 256 locals (slots 0-255) - uses wide instructions for slot 255+
        let code = generate_many_locals(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(
            result.is_ok(),
            "256 locals should compile successfully (wide instructions)"
        );

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "256 locals should run successfully");
    }

    #[test]
    fn locals_exceeding_u8_uses_wide_instructions() {
        // 257 locals requires LoadLocalW/StoreLocalW for slot 256
        let code = generate_many_locals(257);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "257 locals should compile (using wide instructions)");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "257 locals should run correctly with wide instructions");
    }

    #[test]
    fn locals_well_over_u8_limit() {
        // 300 locals - well into wide instruction territory
        let code = generate_many_locals(300);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "300 locals should compile successfully");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "300 locals should run successfully");
    }
}

mod function_argument_limits {
    use super::*;

    #[test]
    fn positional_args_under_u8_limit_succeeds() {
        // 255 positional args should work
        let code = generate_many_positional_args(255);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "255 positional args should compile successfully");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "255 positional args should run successfully");
    }

    #[test]
    fn positional_args_at_u8_boundary_returns_syntax_error() {
        // 256 positional args - exceeds u8 limit, should return SyntaxError
        let code = generate_many_positional_args(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert_syntax_error(result, "more than 255 positional arguments");
    }

    #[test]
    fn positional_args_exceeding_u8_limit_returns_syntax_error() {
        // 257 positional args - clearly exceeds u8 capacity
        let code = generate_many_positional_args(257);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert_syntax_error(result, "more than 255 positional arguments");
    }
}

mod keyword_argument_limits {
    use super::*;

    #[test]
    fn keyword_args_under_u8_limit_succeeds() {
        // 255 keyword args should work
        let code = generate_many_keyword_args(255);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "255 keyword args should compile successfully");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "255 keyword args should run successfully");
    }

    #[test]
    fn keyword_args_at_u8_boundary_returns_syntax_error() {
        // 256 keyword args - exceeds u8 limit, should return SyntaxError
        let code = generate_many_keyword_args(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert_syntax_error(result, "more than 255 keyword arguments");
    }

    #[test]
    fn keyword_args_exceeding_u8_limit_returns_syntax_error() {
        // 257 keyword args - clearly exceeds u8 capacity
        let code = generate_many_keyword_args(257);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert_syntax_error(result, "more than 255 keyword arguments");
    }
}

mod function_parameter_limits {
    use super::*;

    #[test]
    fn parameters_under_u8_limit_succeeds() {
        // 255 parameters should work - both definition and call
        let code = generate_many_parameters(255);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "255 parameters should compile successfully");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "255 parameters should run successfully");
    }

    #[test]
    fn parameters_at_u8_boundary_returns_syntax_error_for_call() {
        // 256 parameters - the function definition uses locals (wide instructions ok),
        // but the call site has 256 positional args which exceeds the limit
        let code = generate_many_parameters(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert_syntax_error(result, "more than 255 positional arguments");
    }

    #[test]
    fn parameters_exceeding_u8_limit_returns_syntax_error_for_call() {
        // 257 parameters - same issue, call site has too many args
        let code = generate_many_parameters(257);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert_syntax_error(result, "more than 255 positional arguments");
    }
}
