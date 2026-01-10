//! Tests for bytecode operand overflow limits.
//!
//! These tests verify that the bytecode compiler handles cases where operands
//! exceed the u8/u16 limits of the bytecode encoding. The compiler should either:
//! - Emit wide instructions (e.g., `LoadLocalW` instead of `LoadLocal`)
//! - Return a compile-time error with a clear message
//!
//! Most likely overflow scenarios (u8 = 256 limit):
//! - Local variable slots: 256+ locals in a function
//! - Function argument counts: 256+ positional args
//! - Keyword argument counts: 256+ keyword args

use std::fmt::Write;

use monty::MontyRun;

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
        // 256 locals (slots 0-255) - boundary case for u8
        let code = generate_many_locals(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "256 locals should compile successfully");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "256 locals should run successfully");
    }

    #[test]
    fn locals_exceeding_u8_requires_wide_instructions() {
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
    fn positional_args_at_u8_boundary() {
        // 256 positional args - boundary case for u8
        let code = generate_many_positional_args(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        // Should either compile with wide encoding or return a clear error
        if let Ok(run) = result {
            let result = run.run_no_limits(vec![]);
            assert!(
                result.is_ok(),
                "256 positional args should run if it compiles: {:?}",
                result.err()
            );
        }
        // If it errors, that's also acceptable - just shouldn't panic or corrupt
    }

    #[test]
    fn positional_args_exceeding_u8_limit() {
        // 257 positional args - exceeds u8 capacity
        let code = generate_many_positional_args(257);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        // Should either handle gracefully or return a compile error
        // Must NOT panic or produce incorrect results
        if let Ok(run) = result {
            let exec_result = run.run_no_limits(vec![]);
            // If it runs, it should return the correct count
            if let Ok(value) = exec_result {
                // The function returns len(args), which should be 257
                assert!(
                    format!("{value:?}").contains("257"),
                    "should return 257 args, got {value:?}"
                );
            }
        }
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
    fn keyword_args_at_u8_boundary() {
        // 256 keyword args - boundary case for u8
        let code = generate_many_keyword_args(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        // Should either compile with wide encoding or return a clear error
        if let Ok(run) = result {
            let result = run.run_no_limits(vec![]);
            assert!(
                result.is_ok(),
                "256 keyword args should run if it compiles: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn keyword_args_exceeding_u8_limit() {
        // 257 keyword args - exceeds u8 capacity
        let code = generate_many_keyword_args(257);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        // Should either handle gracefully or return a compile error
        if let Ok(run) = result {
            let exec_result = run.run_no_limits(vec![]);
            if let Ok(value) = exec_result {
                assert!(
                    format!("{value:?}").contains("257"),
                    "should return 257 kwargs, got {value:?}"
                );
            }
        }
    }
}

mod function_parameter_limits {
    use super::*;

    #[test]
    fn parameters_under_u8_limit_succeeds() {
        // 255 parameters should work
        let code = generate_many_parameters(255);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        assert!(result.is_ok(), "255 parameters should compile successfully");

        let run = result.unwrap();
        let result = run.run_no_limits(vec![]);
        assert!(result.is_ok(), "255 parameters should run successfully");
    }

    #[test]
    fn parameters_at_u8_boundary() {
        // 256 parameters - boundary case
        let code = generate_many_parameters(256);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        if let Ok(run) = result {
            let result = run.run_no_limits(vec![]);
            assert!(
                result.is_ok(),
                "256 parameters should run if it compiles: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn parameters_exceeding_u8_limit() {
        // 257 parameters - exceeds u8, needs wide local slots
        let code = generate_many_parameters(257);
        let result = MontyRun::new(code, "test.py", vec![], vec![]);
        if let Ok(run) = result {
            let exec_result = run.run_no_limits(vec![]);
            if let Ok(value) = exec_result {
                // The function returns p256, which should be 256
                assert!(
                    format!("{value:?}").contains("256"),
                    "should return p256=256, got {value:?}"
                );
            }
        }
    }
}
