use monty::{ExcType, MontyException, MontyRun};

/// Tests that unimplemented features return `NotImplementedError` exceptions.
mod not_implemented_error {
    use super::*;

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
    fn async_functions_return_not_implemented_error() {
        let result = MontyRun::new("async def foo(): pass".to_owned(), "test.py", vec![], vec![]);
        assert_eq!(get_exc_type(result), ExcType::NotImplementedError);
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
    fn unknown_imports_return_module_not_found_error() {
        // Unknown modules (not sys or typing) return a ModuleNotFoundError
        let result = MontyRun::new("import os".to_owned(), "test.py", vec![], vec![]);
        assert_eq!(get_exc_type(result), ExcType::ModuleNotFoundError);
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
}

/// Tests that syntax errors return `SyntaxError` exceptions.
mod syntax_error {
    use super::*;

    /// Helper to extract the exception type from a parse error.
    fn get_exc_type(result: Result<MontyRun, MontyException>) -> ExcType {
        let err = result.expect_err("expected parse error");
        err.exc_type()
    }

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
}
