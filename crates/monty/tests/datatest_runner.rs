use std::{error::Error, ffi::CString, fs, path::Path, sync::mpsc, thread, time::Duration};

use ahash::AHashMap;
use monty::{
    ExcType, ExternalResult, LimitedTracker, MontyException, MontyObject, MontyRun, ResourceLimits, RunProgress,
    StdPrint,
};
use pyo3::{prelude::*, types::PyDict};

/// Recursion limit for test execution.
///
/// Used for both Monty and CPython tests. CPython needs ~5 extra frames
/// for runpy overhead, which is added in run_file_and_get_traceback.
///
/// NOTE this value is chosen to avoid both:
/// * other recursion errors in python (if it's too low)
/// * and, stack overflows in debug rust (if it's too high)
const TEST_RECURSION_LIMIT: usize = 50;

/// Test configuration parsed from directive comments.
///
/// Parsed from an optional first-line comment like `# xfail=monty,cpython` or `# mode: iter`.
/// If not present, defaults to running on both interpreters in standard mode.
///
/// ## Xfail Semantics (Strict)
/// - `xfail=monty` - Test is expected to fail on Monty; if it passes, that's an error
/// - `xfail=cpython` - Test is expected to fail on CPython; if it passes, that's an error
/// - `xfail=monty,cpython` - Expected to fail on both interpreters
#[derive(Debug, Clone, Default)]
struct TestConfig {
    /// When true, test is expected to fail on Monty (strict xfail).
    xfail_monty: bool,
    /// When true, test is expected to fail on CPython (strict xfail).
    xfail_cpython: bool,
    /// When true, use MontyRun with external function support instead of MontyRun.
    iter_mode: bool,
}

/// Represents the expected outcome of a test fixture
#[derive(Debug, Clone)]
enum Expectation {
    /// Expect exception (parse-time or runtime) with specific message
    Raise(String),
    /// Expect successful execution, check py_str() output
    ReturnStr(String),
    /// Expect successful execution, check py_repr() output
    Return(String),
    /// Expect successful execution, check py_type() output
    ReturnType(String),
    /// Expect successful execution, check ref counts of named variables.
    /// Only used when `ref-count-return` feature is enabled; skipped otherwise.
    RefCounts(#[cfg_attr(not(feature = "ref-count-return"), allow(dead_code))] AHashMap<String, usize>),
    /// Expect exception with full traceback comparison.
    /// The expected traceback string should match exactly between Monty and CPython.
    Traceback(String),
    /// Expect successful execution without raising an exception (no return value check).
    /// Used for tests that rely on asserts or just verify code runs.
    NoException,
}

impl Expectation {
    /// Returns the expected value string
    fn expected_value(&self) -> &str {
        match self {
            Expectation::Raise(s)
            | Expectation::ReturnStr(s)
            | Expectation::Return(s)
            | Expectation::ReturnType(s)
            | Expectation::Traceback(s) => s,
            Expectation::RefCounts(_) | Expectation::NoException => "",
        }
    }
}

/// Parse a Python fixture file into code, expected outcome, and test configuration.
///
/// The file may optionally start with a `# xfail=monty,cpython` comment to specify
/// which interpreters the test is expected to fail on. If not present, defaults to
/// running on both and expecting success.
///
/// The file may have an expectation comment as the LAST line:
/// - `# Raise=ExceptionType('message')` - Exception (parse-time or runtime)
/// - `# Return.str=value` - Check py_str() output
/// - `# Return=value` - Check py_repr() output
/// - `# Return.type=typename` - Check py_type() output
/// - `# ref-counts={'var': count, ...}` - Check ref counts of named heap variables
///
/// Or a traceback expectation as a triple-quoted string at the end (uses actual test filename):
/// ```text
/// """TRACEBACK:
/// Traceback (most recent call last):
///   File "my_test.py", line 4, in <module>
///     foo()
/// ValueError: message
/// """
/// ```
///
/// If no expectation comment is present, the test just verifies the code runs without exception.
fn parse_fixture(content: &str) -> (String, Expectation, TestConfig) {
    let lines: Vec<&str> = content.lines().collect();

    assert!(!lines.is_empty(), "Empty fixture file");

    // Check for directives at the start of the file
    // Supports: # xfail=monty,cpython and # mode: iter (can be combined on same line)
    // Note: Directive lines are kept in the code (they're Python comments) to preserve line numbers
    let (config, code_start_idx) = if let Some(first_line) = lines.first() {
        let mut config = TestConfig::default();

        // Check for mode: iter directive
        if first_line.contains("mode: iter") {
            config.iter_mode = true;
        }

        // Check for xfail= directive
        if let Some(xfail_idx) = first_line.find("xfail=") {
            let xfail_str = &first_line[xfail_idx + 6..];
            // Parse until whitespace or end of line
            let xfail_end = xfail_str.find(|c: char| c.is_whitespace()).unwrap_or(xfail_str.len());
            let xfail_str = &xfail_str[..xfail_end];
            config.xfail_monty = xfail_str.contains("monty");
            if xfail_str.contains("cpython") {
                config.xfail_cpython = true;
            }
        }

        (config, 0)
    } else {
        (TestConfig::default(), 0)
    };

    // Check if first code line has an expectation (this is an error)
    if let Some(first_code_line) = lines.get(code_start_idx) {
        assert!(
            !(first_code_line.starts_with("# Return") || first_code_line.starts_with("# Raise")),
            "Expectation comment must be on the LAST line, not the first line"
        );
    }

    // Check for TRACEBACK expectation (triple-quoted string at end of file)
    // Format: """TRACEBACK:\n...\n"""
    if let Some((code, traceback)) = parse_traceback_expectation(content, code_start_idx) {
        return (code, Expectation::Traceback(traceback), config);
    }

    // Get the last line and check if it's an expectation comment
    let last_line = lines.last().unwrap();

    // Parse expectation from comment line if present
    // Note: Check more specific patterns first (Return.str, Return.type, ref-counts) before general Return
    let (expectation, code_lines) = if let Some(expected) = last_line.strip_prefix("# ref-counts=") {
        (
            Expectation::RefCounts(parse_ref_counts(expected)),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Return.str=") {
        (
            Expectation::ReturnStr(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Return.type=") {
        (
            Expectation::ReturnType(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Return=") {
        (
            Expectation::Return(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Raise=") {
        (
            Expectation::Raise(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else {
        // No expectation comment - just run and check it doesn't raise
        (Expectation::NoException, &lines[code_start_idx..])
    };

    // Code is everything except the directive comment (and expectation comment if present)
    let code = code_lines.join("\n");

    (code, expectation, config)
}

/// Parses a TRACEBACK expectation from the end of a fixture file.
///
/// Looks for a triple-quoted string starting with `"""TRACEBACK:` at the end of the file.
/// Returns `Some((code, expected_traceback))` if found, `None` otherwise.
///
/// The traceback string should contain the full expected output including the
/// "Traceback (most recent call last):" header and the exception line.
fn parse_traceback_expectation(content: &str, code_start_idx: usize) -> Option<(String, String)> {
    // Format: """\nTRACEBACK:\n...\n"""
    const MARKER: &str = "\"\"\"\nTRACEBACK:\n";

    // Find the TRACEBACK marker
    let marker_pos = content.find(MARKER)?;

    // Extract the code before the marker
    let code_part = &content[..marker_pos];
    let lines: Vec<&str> = code_part.lines().collect();
    let code = lines[code_start_idx..].join("\n").trim_end().to_string();

    // Extract the traceback content between the markers
    let after_marker = &content[marker_pos + MARKER.len()..];

    // Find the closing triple quotes (preceded by newline)
    let end_pos = after_marker.find("\n\"\"\"")?;
    let traceback_content = &after_marker[..end_pos];

    Some((code, traceback_content.to_string()))
}

/// Parses the ref-counts format: {'var': count, 'var2': count2}
///
/// Supports both single and double quotes for variable names.
/// Example: {'x': 2, 'y': 1} or {"x": 2, "y": 1}
fn parse_ref_counts(s: &str) -> AHashMap<String, usize> {
    let mut counts = AHashMap::new();
    let trimmed = s.trim().trim_start_matches('{').trim_end_matches('}');
    for pair in trimmed.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let parts: Vec<&str> = pair.split(':').collect();
        assert!(
            parts.len() == 2,
            "Invalid ref-counts pair format: {pair}. Expected 'name': count"
        );
        let name = parts[0].trim().trim_matches('\'').trim_matches('"');
        let count: usize = parts[1]
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("Invalid ref count value: {}", parts[1]));
        counts.insert(name.to_string(), count);
    }
    counts
}

/// External function names available in iter mode tests.
///
/// These functions are provided by the test runner when a test uses `# mode: iter`.
const ITER_EXT_FUNCTIONS: &[&str] = &[
    "add_ints",           // (a, b) -> a + b (integers)
    "concat_strings",     // (a, b) -> a + b (strings)
    "return_value",       // (x) -> x (identity)
    "get_list",           // () -> [1, 2, 3]
    "raise_error",        // (exc_type: str, message: str) -> raises exception
    "make_point",         // () -> Dataclass Point(x=1, y=2) (immutable)
    "make_mutable_point", // () -> Dataclass Point(x=1, y=2) (mutable)
    "make_user",          // (name) -> Dataclass User(name=name, active=True) (immutable)
    "make_empty",         // () -> Dataclass Empty() (immutable, no fields)
];

/// Python implementations of external functions for running iter mode tests in CPython.
///
/// These implementations mirror the behavior of `dispatch_external_call` so that
/// iter mode tests produce identical results in both Monty and CPython.
///
/// This is loaded from `scripts/iter_test_methods.py` which is also imported by
/// `scripts/run_traceback.py` to ensure consistency.
const ITER_EXT_FUNCTIONS_PYTHON: &str = include_str!("../../../scripts/iter_test_methods.py");

/// Dispatches an external function call to the appropriate test implementation.
///
/// Returns `ExternalResult::Return` for successful calls, or `ExternalResult::Error`
/// for calls that should raise an exception.
///
/// # Panics
/// Panics if the function name is unknown or arguments are invalid types.
fn dispatch_external_call(name: &str, args: Vec<MontyObject>) -> ExternalResult {
    match name {
        "add_ints" => {
            assert!(args.len() == 2, "add_ints requires 2 arguments");
            let a = i64::try_from(&args[0]).expect("add_ints: first arg must be int");
            let b = i64::try_from(&args[1]).expect("add_ints: second arg must be int");
            MontyObject::Int(a + b).into()
        }
        "concat_strings" => {
            assert!(args.len() == 2, "concat_strings requires 2 arguments");
            let a = String::try_from(&args[0]).expect("concat_strings: first arg must be str");
            let b = String::try_from(&args[1]).expect("concat_strings: second arg must be str");
            MontyObject::String(a + &b).into()
        }
        "return_value" => {
            assert!(args.len() == 1, "return_value requires 1 argument");
            args.into_iter().next().unwrap().into()
        }
        "get_list" => {
            assert!(args.is_empty(), "get_list requires no arguments");
            MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2), MontyObject::Int(3)]).into()
        }
        "raise_error" => {
            // raise_error(exc_type: str, message: str) -> raises exception
            assert!(args.len() == 2, "raise_error requires 2 arguments");
            let exc_type_str = String::try_from(&args[0]).expect("raise_error: first arg must be str");
            let message = String::try_from(&args[1]).expect("raise_error: second arg must be str");
            let exc_type = match exc_type_str.as_str() {
                "ValueError" => ExcType::ValueError,
                "TypeError" => ExcType::TypeError,
                "KeyError" => ExcType::KeyError,
                "RuntimeError" => ExcType::RuntimeError,
                _ => panic!("raise_error: unsupported exception type: {exc_type_str}"),
            };
            MontyException::new(exc_type, Some(message)).into()
        }
        "make_point" => {
            assert!(args.is_empty(), "make_point requires no arguments");
            // Return an immutable Point(x=1, y=2) dataclass
            MontyObject::Dataclass {
                name: "Point".to_string(),
                field_names: vec!["x".to_string(), "y".to_string()],
                attrs: vec![
                    (MontyObject::String("x".to_string()), MontyObject::Int(1)),
                    (MontyObject::String("y".to_string()), MontyObject::Int(2)),
                ]
                .into(),
                methods: vec![],
                frozen: true,
            }
            .into()
        }
        "make_mutable_point" => {
            assert!(args.is_empty(), "make_mutable_point requires no arguments");
            // Return a mutable Point(x=1, y=2) dataclass
            MontyObject::Dataclass {
                name: "MutablePoint".to_string(),
                field_names: vec!["x".to_string(), "y".to_string()],
                attrs: vec![
                    (MontyObject::String("x".to_string()), MontyObject::Int(1)),
                    (MontyObject::String("y".to_string()), MontyObject::Int(2)),
                ]
                .into(),
                methods: vec![],
                frozen: false,
            }
            .into()
        }
        "make_user" => {
            assert!(args.len() == 1, "make_user requires 1 argument");
            let name = String::try_from(&args[0]).expect("make_user: first arg must be str");
            // Return an immutable User(name=name, active=True) dataclass
            MontyObject::Dataclass {
                name: "User".to_string(),
                field_names: vec!["name".to_string(), "active".to_string()],
                attrs: vec![
                    (MontyObject::String("name".to_string()), MontyObject::String(name)),
                    (MontyObject::String("active".to_string()), MontyObject::Bool(true)),
                ]
                .into(),
                methods: vec![],
                frozen: true,
            }
            .into()
        }
        "make_empty" => {
            assert!(args.is_empty(), "make_empty requires no arguments");
            // Return an immutable empty dataclass with no fields
            MontyObject::Dataclass {
                name: "Empty".to_string(),
                field_names: vec![],
                attrs: vec![].into(),
                methods: vec![],
                frozen: true,
            }
            .into()
        }
        _ => panic!("Unknown external function: {name}"),
    }
}

/// Represents a test failure with details about expected vs actual values.
#[derive(Debug)]
struct TestFailure {
    test_name: String,
    kind: String,
    expected: String,
    actual: String,
}

impl std::fmt::Display for TestFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} mismatch\n  expected: {}\n  actual: {}",
            self.test_name, self.kind, self.expected, self.actual
        )
    }
}

/// Try to run a test, returning Ok(()) on success or Err with failure details.
///
/// This function executes Python code via the MontyRun and validates the result
/// against the expected outcome specified in the fixture.
fn try_run_test(path: &Path, code: &str, expectation: &Expectation) -> Result<(), TestFailure> {
    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    // Handle ref-count-return tests separately since they need run_ref_counts()
    #[cfg(feature = "ref-count-return")]
    if let Expectation::RefCounts(expected) = expectation {
        match MontyRun::new(code.to_owned(), &test_name, vec![], vec![]) {
            Ok(ex) => {
                let result = ex.run_ref_counts(vec![]);
                match result {
                    Ok(monty::RefCountOutput {
                        counts,
                        unique_refs,
                        heap_count,
                        ..
                    }) => {
                        // Strict matching: verify all heap objects are accounted for by variables
                        if unique_refs != heap_count {
                            return Err(TestFailure {
                                test_name,
                                kind: "Strict matching".to_string(),
                                expected: format!("{heap_count} heap objects"),
                                actual: format!("{unique_refs} referenced by variables, counts: {counts:?}"),
                            });
                        }
                        if &counts != expected {
                            return Err(TestFailure {
                                test_name,
                                kind: "ref-counts".to_string(),
                                expected: format!("{expected:?}"),
                                actual: format!("{counts:?}"),
                            });
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(TestFailure {
                            test_name,
                            kind: "Runtime".to_string(),
                            expected: "success".to_string(),
                            actual: e.to_string(),
                        });
                    }
                }
            }
            Err(parse_err) => {
                return Err(TestFailure {
                    test_name,
                    kind: "Parse".to_string(),
                    expected: "success".to_string(),
                    actual: parse_err.to_string(),
                });
            }
        }
    }

    match MontyRun::new(code.to_owned(), &test_name, vec![], vec![]) {
        Ok(ex) => {
            let limits = ResourceLimits::new().max_recursion_depth(Some(TEST_RECURSION_LIMIT));
            let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);
            match result {
                Ok(obj) => match expectation {
                    Expectation::ReturnStr(expected) => {
                        let output = obj.to_string();
                        if output != *expected {
                            return Err(TestFailure {
                                test_name,
                                kind: "str()".to_string(),
                                expected: expected.clone(),
                                actual: output,
                            });
                        }
                    }
                    Expectation::Return(expected) => {
                        let output = obj.py_repr();
                        if output != *expected {
                            return Err(TestFailure {
                                test_name,
                                kind: "py_repr()".to_string(),
                                expected: expected.clone(),
                                actual: output,
                            });
                        }
                    }
                    Expectation::ReturnType(expected) => {
                        let output = obj.type_name();
                        if output != expected {
                            return Err(TestFailure {
                                test_name,
                                kind: "type_name()".to_string(),
                                expected: expected.clone(),
                                actual: output.to_string(),
                            });
                        }
                    }
                    #[cfg(not(feature = "ref-count-return"))]
                    Expectation::RefCounts(_) => {
                        // Skip ref-count tests when feature is disabled
                    }
                    Expectation::NoException => {
                        // Success - code ran without exception as expected
                    }
                    Expectation::Raise(expected) | Expectation::Traceback(expected) => {
                        return Err(TestFailure {
                            test_name,
                            kind: "Exception".to_string(),
                            expected: expected.clone(),
                            actual: "no exception raised".to_string(),
                        });
                    }
                    #[cfg(feature = "ref-count-return")]
                    Expectation::RefCounts(_) => unreachable!(),
                },
                Err(e) => {
                    if let Expectation::Raise(expected) = expectation {
                        let output = e.py_repr();
                        if output != *expected {
                            return Err(TestFailure {
                                test_name,
                                kind: "Exception".to_string(),
                                expected: expected.clone(),
                                actual: output,
                            });
                        }
                    } else if let Expectation::Traceback(expected) = expectation {
                        let output = e.to_string();
                        if output != *expected {
                            return Err(TestFailure {
                                test_name,
                                kind: "Traceback".to_string(),
                                expected: expected.clone(),
                                actual: output,
                            });
                        }
                    } else {
                        return Err(TestFailure {
                            test_name,
                            kind: "Unexpected error".to_string(),
                            expected: "success".to_string(),
                            actual: e.to_string(),
                        });
                    }
                }
            }
        }
        Err(parse_err) => {
            if let Expectation::Raise(expected) = expectation {
                let output = parse_err.py_repr();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "Parse error".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
            } else if let Expectation::Traceback(expected) = expectation {
                let output = parse_err.to_string();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "Traceback".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
            } else {
                return Err(TestFailure {
                    test_name,
                    kind: "Unexpected parse error".to_string(),
                    expected: "success".to_string(),
                    actual: parse_err.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Try to run a test using MontyRun with external function support.
///
/// This function handles tests marked with `# mode: iter` directive by using the
/// iterative executor API and providing implementations for predefined external functions.
fn try_run_iter_test(path: &Path, code: &str, expectation: &Expectation) -> Result<(), TestFailure> {
    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    // Ref-counting tests not supported in iter mode
    #[cfg(feature = "ref-count-return")]
    if matches!(expectation, Expectation::RefCounts(_)) {
        return Err(TestFailure {
            test_name,
            kind: "Configuration".to_string(),
            expected: "non-refcount test".to_string(),
            actual: "ref-counts tests are not supported in iter mode".to_string(),
        });
    }

    let ext_functions: Vec<String> = ITER_EXT_FUNCTIONS.iter().copied().map(str::to_string).collect();

    let exec = match MontyRun::new(code.to_owned(), &test_name, vec![], ext_functions) {
        Ok(e) => e,
        Err(parse_err) => {
            if let Expectation::Raise(expected) = expectation {
                let output = parse_err.py_repr();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "Parse error".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
                return Ok(());
            } else if let Expectation::Traceback(expected) = expectation {
                let output = parse_err.to_string();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "Traceback".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
                return Ok(());
            }
            return Err(TestFailure {
                test_name,
                kind: "Unexpected parse error".to_string(),
                expected: "success".to_string(),
                actual: parse_err.to_string(),
            });
        }
    };

    // Run execution loop, handling external function calls until complete
    let result = run_iter_loop(exec);

    match result {
        Ok(obj) => match expectation {
            Expectation::ReturnStr(expected) => {
                let output = obj.to_string();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "str()".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
            }
            Expectation::Return(expected) => {
                let output = obj.py_repr();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "py_repr()".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
            }
            Expectation::ReturnType(expected) => {
                let output = obj.type_name();
                if output != expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "type_name()".to_string(),
                        expected: expected.clone(),
                        actual: output.to_string(),
                    });
                }
            }
            #[cfg(not(feature = "ref-count-return"))]
            Expectation::RefCounts(_) => {}
            Expectation::NoException => {}
            Expectation::Raise(expected) | Expectation::Traceback(expected) => {
                return Err(TestFailure {
                    test_name,
                    kind: "Exception".to_string(),
                    expected: expected.clone(),
                    actual: "no exception raised".to_string(),
                });
            }
            #[cfg(feature = "ref-count-return")]
            Expectation::RefCounts(_) => unreachable!(),
        },
        Err(e) => {
            if let Expectation::Raise(expected) = expectation {
                let output = e.py_repr();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "Exception".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
            } else if let Expectation::Traceback(expected) = expectation {
                let output = e.to_string();
                if output != *expected {
                    return Err(TestFailure {
                        test_name,
                        kind: "Traceback".to_string(),
                        expected: expected.clone(),
                        actual: output,
                    });
                }
            } else {
                return Err(TestFailure {
                    test_name,
                    kind: "Unexpected error".to_string(),
                    expected: "success".to_string(),
                    actual: e.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Execute the iter loop, dispatching external function calls until complete.
///
/// When `ref-count-panic` feature is NOT enabled, this function also tests
/// serialization round-trips by dumping and loading the execution state at
/// each external function call boundary.
fn run_iter_loop(exec: MontyRun) -> Result<MontyObject, MontyException> {
    let limits = ResourceLimits::new().max_recursion_depth(Some(TEST_RECURSION_LIMIT));
    let mut progress = exec.start(vec![], LimitedTracker::new(limits), &mut StdPrint)?;

    loop {
        // Test serialization round-trip at each step (skip when ref-count-panic is enabled
        // since the old RunProgress would panic on drop without proper cleanup)
        #[cfg(not(feature = "ref-count-panic"))]
        {
            let bytes = progress.dump().expect("failed to dump RunProgress");
            progress = RunProgress::load(&bytes).expect("failed to load RunProgress");
        }

        match progress {
            RunProgress::Complete(result) => return Ok(result),
            RunProgress::FunctionCall {
                function_name,
                args,
                kwargs: _,
                state,
            } => {
                let return_value = dispatch_external_call(&function_name, args);
                progress = state.run(return_value, &mut StdPrint)?;
            }
        }
    }
}

/// Split Python code into statements and a final expression to evaluate.
///
/// For Return expectations, the last non-empty line is the expression to evaluate.
/// For Raise/NoException, the entire code is statements (returns None for expression).
///
/// Returns (statements_code, optional_final_expression).
fn split_code_for_module(code: &str, need_return_value: bool) -> (String, Option<String>) {
    let lines: Vec<&str> = code.lines().collect();

    // Find the last non-empty line
    let last_idx = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("Empty code");

    if need_return_value {
        let last_line = lines[last_idx].trim();

        // Check if the last line is a statement (can't be evaluated as an expression)
        // Matches both `assert expr` and `assert(expr)` forms
        if last_line.starts_with("assert ") || last_line.starts_with("assert(") {
            // All code is statements, no expression to evaluate
            (lines[..=last_idx].join("\n"), None)
        } else {
            // Everything except last line is statements, last line is the expression
            let statements = lines[..last_idx].join("\n");
            let expr = last_line.to_string();
            (statements, Some(expr))
        }
    } else {
        // All code is statements (for exception tests or NoException)
        (lines[..=last_idx].join("\n"), None)
    }
}

/// Run the traceback script to get CPython's traceback output for a test file.
///
/// This imports scripts/run_traceback.py via pyo3 and calls `run_file_and_get_traceback()`
/// which executes the file via runpy.run_path() to ensure full traceback information
/// (including caret lines) is preserved.
///
/// When `iter_mode` is true, external function implementations are injected into the
/// file's globals before execution.
fn run_traceback_script(path: &Path, iter_mode: bool) -> String {
    Python::attach(|py| {
        let run_traceback = import_run_traceback(py);

        // Get absolute path for the test file
        let abs_path = path.canonicalize().expect("Failed to get absolute path");
        let path_str = abs_path.to_str().expect("Invalid UTF-8 in path");

        // Call run_file_and_get_traceback with the recursion limit and iter_mode flag
        let result = run_traceback
            .call_method1(
                "run_file_and_get_traceback",
                (path_str, TEST_RECURSION_LIMIT, iter_mode),
            )
            .expect("Failed to call run_file_and_get_traceback");

        // Handle None return (no exception raised)
        if result.is_none() {
            String::new()
        } else {
            result
                .extract()
                .expect("Failed to extract string from return value of run_file_and_get_traceback")
        }
    })
}

fn format_traceback(py: Python<'_>, exc: PyErr) -> String {
    let run_traceback = import_run_traceback(py);
    let exc_value = exc.value(py);
    let return_value = run_traceback
        .call_method1("format_full_traceback", (exc_value,))
        .expect("Failed to call format_full_traceback");
    return_value
        .extract()
        .expect("failed to extract string from return value of format_full_traceback")
}

/// Import the run_traceback module
fn import_run_traceback(py: Python<'_>) -> Bound<'_, PyModule> {
    // Add scripts directory to sys.path (tests run from crates/monty/)
    let sys = py.import("sys").expect("Failed to import sys");
    let sys_path = sys.getattr("path").expect("Failed to get sys.path");
    sys_path
        .call_method1("insert", (0, "../../scripts"))
        .expect("Failed to add scripts to sys.path");

    // Import the run_traceback module
    py.import("run_traceback").expect("Failed to import run_traceback")
}

/// Result from CPython execution - either a value to compare, or an early return.
enum CpythonResult {
    /// Value to compare against expectation
    Value(String),
    /// No value to compare (NoException test succeeded)
    NoValue,
    /// Test failed with this error
    Failed(TestFailure),
}

/// Try to run a test through CPython, returning Ok(()) on success or Err with failure details.
///
/// This function executes the same Python code via CPython (using pyo3) and
/// compares the result with the expected value. This ensures Monty behaves
/// identically to CPython.
///
/// Code is executed at module level (not wrapped in a function) so that
/// `global` keyword semantics work correctly.
///
/// RefCounts tests are skipped as they're Monty-specific.
/// Traceback tests use scripts/run_traceback.py for reliable caret line support.
fn try_run_cpython_test(
    path: &Path,
    code: &str,
    expectation: &Expectation,
    iter_mode: bool,
) -> Result<(), TestFailure> {
    // Skip RefCounts tests - only relevant for Monty
    if matches!(expectation, Expectation::RefCounts(_)) {
        return Ok(());
    }

    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    // Traceback tests use the external script for reliable caret line support
    if let Expectation::Traceback(expected) = expectation {
        let result = run_traceback_script(path, iter_mode);
        if result != *expected {
            return Err(TestFailure {
                test_name,
                kind: "CPython traceback".to_string(),
                expected: expected.clone(),
                actual: result,
            });
        }
        return Ok(());
    }

    let need_return_value = matches!(
        expectation,
        Expectation::Return(_) | Expectation::ReturnStr(_) | Expectation::ReturnType(_)
    );
    let (statements, maybe_expr) = split_code_for_module(code, need_return_value);

    let result: CpythonResult = Python::attach(|py| {
        // Execute statements at module level
        let globals = PyDict::new(py);

        // For iter mode tests, inject external function implementations into globals
        if iter_mode {
            let ext_funcs_cstr = CString::new(ITER_EXT_FUNCTIONS_PYTHON).expect("Invalid C string in ext funcs");
            py.run(&ext_funcs_cstr, Some(&globals), None)
                .expect("Failed to define external functions for iter mode");
        }

        // Run the statements
        let statements_cstr = CString::new(statements.as_str()).expect("Invalid C string in statements");
        let stmt_result = py.run(&statements_cstr, Some(&globals), None);

        // Handle exception during statement execution
        if let Err(e) = stmt_result {
            if matches!(expectation, Expectation::NoException) {
                return CpythonResult::Failed(TestFailure {
                    test_name: test_name.clone(),
                    kind: "CPython unexpected exception".to_string(),
                    expected: "no exception".to_string(),
                    actual: format_traceback(py, e),
                });
            }
            if matches!(expectation, Expectation::Raise(_)) {
                return CpythonResult::Value(format_cpython_exception(py, &e));
            }
            return CpythonResult::Failed(TestFailure {
                test_name: test_name.clone(),
                kind: "CPython unexpected exception".to_string(),
                expected: "success".to_string(),
                actual: format_traceback(py, e),
            });
        }

        // If we have an expression to evaluate, evaluate it
        if let Some(expr) = maybe_expr {
            let expr_cstr = CString::new(expr.as_str()).expect("Invalid C string in expr");
            match py.eval(&expr_cstr, Some(&globals), None) {
                Ok(result) => {
                    // Code returned successfully - format based on expectation type
                    match expectation {
                        Expectation::Return(_) => CpythonResult::Value(result.repr().unwrap().to_string()),
                        Expectation::ReturnStr(_) => CpythonResult::Value(result.str().unwrap().to_string()),
                        Expectation::ReturnType(_) => {
                            CpythonResult::Value(result.get_type().name().unwrap().to_string())
                        }
                        Expectation::Raise(expected) => CpythonResult::Failed(TestFailure {
                            test_name: test_name.clone(),
                            kind: "CPython exception".to_string(),
                            expected: expected.clone(),
                            actual: "no exception raised".to_string(),
                        }),
                        // Traceback tests are handled by run_traceback_script above
                        Expectation::Traceback(_) | Expectation::NoException | Expectation::RefCounts(_) => {
                            unreachable!()
                        }
                    }
                }
                Err(e) => {
                    // Expression raised an exception
                    if matches!(expectation, Expectation::NoException) {
                        return CpythonResult::Failed(TestFailure {
                            test_name: test_name.clone(),
                            kind: "CPython unexpected exception".to_string(),
                            expected: "no exception".to_string(),
                            actual: format_traceback(py, e),
                        });
                    }
                    if matches!(expectation, Expectation::Raise(_)) {
                        return CpythonResult::Value(format_cpython_exception(py, &e));
                    }
                    // Traceback tests are handled by run_traceback_script above
                    CpythonResult::Failed(TestFailure {
                        test_name: test_name.clone(),
                        kind: "CPython unexpected exception".to_string(),
                        expected: "success".to_string(),
                        actual: format_traceback(py, e),
                    })
                }
            }
        } else {
            // No expression to evaluate
            // Traceback tests are handled by run_traceback_script above
            if let Expectation::Raise(expected) = expectation {
                return CpythonResult::Failed(TestFailure {
                    test_name: test_name.clone(),
                    kind: "CPython exception".to_string(),
                    expected: expected.clone(),
                    actual: "no exception raised".to_string(),
                });
            }
            CpythonResult::NoValue // NoException expectation - success
        }
    });

    match result {
        CpythonResult::Value(actual) => {
            let expected = expectation.expected_value();
            if actual != expected {
                return Err(TestFailure {
                    test_name,
                    kind: "CPython result".to_string(),
                    expected: expected.to_string(),
                    actual,
                });
            }
            Ok(())
        }
        CpythonResult::NoValue => Ok(()),
        CpythonResult::Failed(failure) => Err(failure),
    }
}

/// Format a CPython exception into the expected format.
fn format_cpython_exception(py: Python<'_>, e: &PyErr) -> String {
    let exc_type = e.get_type(py).name().unwrap();
    let exc_message: String = e
        .value(py)
        .getattr("args")
        .and_then(|args| args.get_item(0))
        .and_then(|item| item.extract())
        .unwrap_or_default();

    if exc_message.is_empty() {
        format!("{exc_type}()")
    } else if exc_message.contains('\'') {
        // Use double quotes when message contains single quotes (like Python's repr)
        format!("{exc_type}(\"{exc_message}\")")
    } else {
        // Use single quotes (default Python repr format)
        format!("{exc_type}('{exc_message}')")
    }
}

/// Timeout duration for Monty tests.
///
/// Tests that exceed this duration are considered to be hanging (infinite loop)
/// and will fail with a timeout error.
const TEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs a closure with a timeout, returning an error if it exceeds the duration.
///
/// Spawns the closure in a separate thread and waits for the result with a timeout.
/// If the timeout is exceeded, returns an error message. Note that the spawned thread
/// will continue running in the background (Rust doesn't support killing threads),
/// but the test will fail immediately.
fn run_with_timeout<F, T>(timeout: Duration, f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = f();
        let _ = tx.send(result);
    });

    rx.recv_timeout(timeout)
        .map_err(|_| format!("test timed out after {timeout:?} (possible infinite loop)"))
}

/// Test function that runs each fixture through Monty.
///
/// Handles xfail with strict semantics: if a test is marked `xfail=monty`, it must fail.
/// If an xfail test passes unexpectedly, that's an error.
fn run_test_cases_monty(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let (code, expectation, config) = parse_fixture(&content);
    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    // Clone data for the closure since it needs 'static lifetime
    let path_owned = path.to_owned();
    let code_owned = code.clone();
    let expectation_owned = expectation.clone();
    let iter_mode = config.iter_mode;

    let result = run_with_timeout(TEST_TIMEOUT, move || {
        if iter_mode {
            try_run_iter_test(&path_owned, &code_owned, &expectation_owned)
        } else {
            try_run_test(&path_owned, &code_owned, &expectation_owned)
        }
    });

    // Handle timeout error
    let result = match result {
        Ok(inner_result) => inner_result,
        Err(timeout_msg) => Err(TestFailure {
            test_name: test_name.clone(),
            kind: "Timeout".to_string(),
            expected: "completion within 5s".to_string(),
            actual: timeout_msg,
        }),
    };

    if config.xfail_monty {
        // Strict xfail: test must fail; if it passed, xfail should be removed
        assert!(
            result.is_err(),
            "[{test_name}] Test marked xfail=monty passed unexpectedly. Remove xfail if the test is now fixed."
        );
    } else if let Err(failure) = result {
        panic!("{failure}");
    }
    Ok(())
}

/// Test function that runs each fixture through CPython.
///
/// Handles xfail with strict semantics: if a test is marked `xfail=cpython`, it must fail.
/// If an xfail test passes unexpectedly, that's an error.
fn run_test_cases_cpython(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let (code, expectation, config) = parse_fixture(&content);
    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    let result = try_run_cpython_test(path, &code, &expectation, config.iter_mode);

    if config.xfail_cpython {
        // Strict xfail: test must fail; if it passed, xfail should be removed
        assert!(
            result.is_err(),
            "[{test_name}] Test marked xfail=cpython passed unexpectedly. Remove xfail if the test is now fixed."
        );
    } else if let Err(failure) = result {
        panic!("{failure}");
    }
    Ok(())
}

// Generate tests for all fixture files using datatest-stable harness macro
datatest_stable::harness!(
    run_test_cases_monty,
    "test_cases",
    r"^.*\.py$",
    run_test_cases_cpython,
    "test_cases",
    r"^.*\.py$",
);
