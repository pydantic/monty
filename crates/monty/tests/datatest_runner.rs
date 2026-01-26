use std::{
    error::Error,
    ffi::CString,
    fs,
    panic::{self, AssertUnwindSafe},
    path::Path,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use ahash::AHashMap;
use monty::{
    ExcType, ExternalResult, LimitedTracker, MontyException, MontyFuture, MontyObject, MontyRun, ResourceLimits,
    RunProgress, StdPrint,
};
use pyo3::{prelude::*, types::PyDict};
use similar::TextDiff;

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
/// Parsed from an optional first-line comment like `# xfail=monty,cpython` or `# call-external`.
/// If not present, defaults to running on both interpreters in standard mode.
///
/// ## Xfail Semantics (Strict)
/// - `xfail=monty` - Test is expected to fail on Monty; if it passes, that's an error
/// - `xfail=cpython` - Test is expected to fail on CPython; if it passes, that's an error
/// - `xfail=monty,cpython` - Expected to fail on both interpreters
#[derive(Debug, Clone, Default)]
#[expect(clippy::struct_excessive_bools)]
struct TestConfig {
    /// When true, test is expected to fail on Monty (strict xfail).
    xfail_monty: bool,
    /// When true, test is expected to fail on CPython (strict xfail).
    xfail_cpython: bool,
    /// When true, use MontyRun with external function support instead of MontyRun.
    iter_mode: bool,
    /// When true, wrap code in async context for CPython execution.
    /// Used for tests with top-level await which Monty supports but CPython doesn't.
    async_mode: bool,
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
    RefCounts(#[cfg_attr(not(feature = "ref-count-return"), expect(dead_code))] AHashMap<String, usize>),
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
            Self::Raise(s) | Self::ReturnStr(s) | Self::Return(s) | Self::ReturnType(s) | Self::Traceback(s) => s,
            Self::RefCounts(_) | Self::NoException => "",
        }
    }
}

/// Parse a Python fixture file into code, expected outcome, and test configuration.
///
/// The file may optionally contain a `# xfail=monty,cpython` comment to specify
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

    // comment lines with leading # and spaces stripped
    let comment_lines = lines
        .iter()
        .filter(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim())
        .collect::<Vec<_>>();

    let mut config = TestConfig {
        iter_mode: comment_lines.iter().any(|line| line.starts_with("call-external")),
        async_mode: comment_lines.iter().any(|line| line.starts_with("run-async")),
        ..Default::default()
    };
    // Check for "xfail=" directive
    if let Some(&xfail_line) = comment_lines.iter().find(|line| line.starts_with("xfail=")) {
        // Parse until whitespace or end of line
        let xfail_end = xfail_line.find(|c: char| c.is_whitespace()).unwrap_or(xfail_line.len());
        let xfail_str = &xfail_line[..xfail_end];
        config.xfail_monty = xfail_str.contains("monty");
        config.xfail_cpython = xfail_str.contains("cpython");
    }

    // Check for TRACEBACK expectation (triple-quoted string at end of file)
    // Format: """TRACEBACK:\n...\n"""
    if let Some((code, traceback)) = parse_traceback_expectation(content) {
        return (code, Expectation::Traceback(traceback), config);
    }

    // Get the last line and check if it's an expectation comment
    let last_line = lines.last().unwrap();

    // Parse expectation from comment line if present
    // Note: Check more specific patterns first (Return.str, Return.type, ref-counts) before general Return
    let (expectation, code_lines) = if let Some(expected) = last_line.strip_prefix("# ref-counts=") {
        (
            Expectation::RefCounts(parse_ref_counts(expected)),
            &lines[..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Return.str=") {
        (Expectation::ReturnStr(expected.to_string()), &lines[..lines.len() - 1])
    } else if let Some(expected) = last_line.strip_prefix("# Return.type=") {
        (Expectation::ReturnType(expected.to_string()), &lines[..lines.len() - 1])
    } else if let Some(expected) = last_line.strip_prefix("# Return=") {
        (Expectation::Return(expected.to_string()), &lines[..lines.len() - 1])
    } else if let Some(expected) = last_line.strip_prefix("# Raise=") {
        (Expectation::Raise(expected.to_string()), &lines[..lines.len() - 1])
    } else {
        // No expectation comment - just run and check it doesn't raise
        (Expectation::NoException, &lines[..])
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
fn parse_traceback_expectation(content: &str) -> Option<(String, String)> {
    // Format: """\nTRACEBACK:\n...\n"""
    const MARKER: &str = "\"\"\"\nTRACEBACK:\n";

    // Find the TRACEBACK marker
    let marker_pos = content.find(MARKER)?;

    // Extract the code before the marker
    let code_part = &content[..marker_pos];
    let lines: Vec<&str> = code_part.lines().collect();
    let code = lines.join("\n").trim_end().to_string();

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
/// These functions are provided by the test runner when a test uses `# call-external`.
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
    "async_call",         // (x) -> async: returns x (coroutine that returns its argument)
];

/// Python implementations of external functions for running iter mode tests in CPython.
///
/// These implementations mirror the behavior of `dispatch_external_call` so that
/// iter mode tests produce identical results in both Monty and CPython.
///
/// This is loaded from `scripts/iter_test_methods.py` which is also imported by
/// `scripts/run_traceback.py` to ensure consistency.
const ITER_EXT_FUNCTIONS_PYTHON: &str = include_str!("../../../scripts/iter_test_methods.py");

/// Result from dispatching an external function call.
///
/// Distinguishes between synchronous calls (return immediately) and
/// asynchronous calls (return a future that needs later resolution).
enum DispatchResult {
    /// Synchronous result - pass directly to `state.run()`.
    Sync(ExternalResult),
    /// Asynchronous call - use `state.run_pending()` and resolve later.
    /// Contains the value to resolve the future with.
    Async(MontyObject),
}

/// Dispatches an external function call to the appropriate test implementation.
///
/// Returns `DispatchResult::Sync` for synchronous calls or `DispatchResult::Async`
/// for coroutine calls that should use `run_pending()`.
///
/// # Panics
/// Panics if the function name is unknown or arguments are invalid types.
fn dispatch_external_call(name: &str, args: Vec<MontyObject>) -> DispatchResult {
    match name {
        "add_ints" => {
            assert!(args.len() == 2, "add_ints requires 2 arguments");
            let a = i64::try_from(&args[0]).expect("add_ints: first arg must be int");
            let b = i64::try_from(&args[1]).expect("add_ints: second arg must be int");
            DispatchResult::Sync(MontyObject::Int(a + b).into())
        }
        "concat_strings" => {
            assert!(args.len() == 2, "concat_strings requires 2 arguments");
            let a = String::try_from(&args[0]).expect("concat_strings: first arg must be str");
            let b = String::try_from(&args[1]).expect("concat_strings: second arg must be str");
            DispatchResult::Sync(MontyObject::String(a + &b).into())
        }
        "return_value" => {
            assert!(args.len() == 1, "return_value requires 1 argument");
            DispatchResult::Sync(args.into_iter().next().unwrap().into())
        }
        "get_list" => {
            assert!(args.is_empty(), "get_list requires no arguments");
            DispatchResult::Sync(
                MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2), MontyObject::Int(3)]).into(),
            )
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
            DispatchResult::Sync(MontyException::new(exc_type, Some(message)).into())
        }
        "make_point" => {
            assert!(args.is_empty(), "make_point requires no arguments");
            // Return an immutable Point(x=1, y=2) dataclass
            DispatchResult::Sync(
                MontyObject::Dataclass {
                    name: "Point".to_string(),
                    type_id: 0, // Test fixture has no real Python type
                    field_names: vec!["x".to_string(), "y".to_string()],
                    attrs: vec![
                        (MontyObject::String("x".to_string()), MontyObject::Int(1)),
                        (MontyObject::String("y".to_string()), MontyObject::Int(2)),
                    ]
                    .into(),
                    methods: vec![],
                    frozen: true,
                }
                .into(),
            )
        }
        "make_mutable_point" => {
            assert!(args.is_empty(), "make_mutable_point requires no arguments");
            // Return a mutable Point(x=1, y=2) dataclass
            DispatchResult::Sync(
                MontyObject::Dataclass {
                    name: "MutablePoint".to_string(),
                    type_id: 0, // Test fixture has no real Python type
                    field_names: vec!["x".to_string(), "y".to_string()],
                    attrs: vec![
                        (MontyObject::String("x".to_string()), MontyObject::Int(1)),
                        (MontyObject::String("y".to_string()), MontyObject::Int(2)),
                    ]
                    .into(),
                    methods: vec![],
                    frozen: false,
                }
                .into(),
            )
        }
        "make_user" => {
            assert!(args.len() == 1, "make_user requires 1 argument");
            let name = String::try_from(&args[0]).expect("make_user: first arg must be str");
            // Return an immutable User(name=name, active=True) dataclass
            DispatchResult::Sync(
                MontyObject::Dataclass {
                    name: "User".to_string(),
                    type_id: 0, // Test fixture has no real Python type
                    field_names: vec!["name".to_string(), "active".to_string()],
                    attrs: vec![
                        (MontyObject::String("name".to_string()), MontyObject::String(name)),
                        (MontyObject::String("active".to_string()), MontyObject::Bool(true)),
                    ]
                    .into(),
                    methods: vec![],
                    frozen: true,
                }
                .into(),
            )
        }
        "make_empty" => {
            assert!(args.is_empty(), "make_empty requires no arguments");
            // Return an immutable empty dataclass with no fields
            DispatchResult::Sync(
                MontyObject::Dataclass {
                    name: "Empty".to_string(),
                    type_id: 0, // Test fixture has no real Python type
                    field_names: vec![],
                    attrs: vec![].into(),
                    methods: vec![],
                    frozen: true,
                }
                .into(),
            )
        }
        "async_call" => {
            // async_call(x) -> coroutine that returns x
            // This is an async function - use run_pending() and resolve later
            assert!(args.len() == 1, "async_call requires 1 argument");
            DispatchResult::Async(args.into_iter().next().unwrap())
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
        writeln!(
            f,
            "[{}] {} mismatch\ngot {:?}\ndiff:",
            self.test_name, self.kind, self.actual
        )?;

        for change in TextDiff::from_lines(&self.expected, &self.actual).iter_all_changes() {
            write!(f, "{}{}", change.tag(), change)?;
        }
        Ok(())
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
/// This function handles tests marked with `# call-external` directive by using the
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
///
/// Supports both synchronous and asynchronous external functions:
/// - Sync functions: result is passed immediately via `state.run()`
/// - Async functions: `state.run_pending()` creates a future, resolved via `ResolveFutures`
fn run_iter_loop(exec: MontyRun) -> Result<MontyObject, MontyException> {
    let limits = ResourceLimits::new().max_recursion_depth(Some(TEST_RECURSION_LIMIT));
    let mut progress = exec.start(vec![], LimitedTracker::new(limits), &mut StdPrint)?;

    // Track pending async calls: (call_id, result_value)
    let mut pending_results: Vec<(u32, MontyObject)> = Vec::new();

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
                call_id,
                state,
            } => {
                let dispatch_result = dispatch_external_call(&function_name, args);
                match dispatch_result {
                    DispatchResult::Sync(return_value) => {
                        progress = state.run(return_value, &mut StdPrint)?;
                    }
                    DispatchResult::Async(result_value) => {
                        // Store the result for later resolution
                        pending_results.push((call_id, result_value));
                        // Continue execution with a pending future
                        progress = state.run(MontyFuture, &mut StdPrint)?;
                    }
                }
            }
            RunProgress::ResolveFutures(state) => {
                // Resolve all pending futures that we have results for
                let results: Vec<(u32, ExternalResult)> = state
                    .pending_call_ids()
                    .iter()
                    .filter_map(|p| {
                        pending_results.iter().position(|(id, _)| id == p).map(|idx| {
                            let (call_id, value) = pending_results.remove(idx);
                            (call_id, ExternalResult::Return(value))
                        })
                    })
                    .collect();

                assert!(
                    !results.is_empty(),
                    "ResolveFutures: no results available for pending calls: {:?}",
                    state.pending_call_ids().iter().collect::<Vec<_>>()
                );

                progress = state.resume(results, &mut StdPrint)?;
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

/// Wraps code in an async context for CPython execution.
///
/// Monty supports top-level `await`, but CPython does not. This function transforms code
/// like:
///
/// ```python
/// async def foo():
///     return 1
/// result = await foo()
/// ```
///
/// Into:
///
/// ```python
/// import asyncio
/// async def __test_main():
///     async def foo():
///         return 1
///     result = await foo()
///     return result  # if need_return_value
/// __test_result__ = asyncio.run(__test_main())
/// ```
fn wrap_code_for_async(code: &str, need_return_value: bool) -> (String, Option<String>) {
    let lines: Vec<&str> = code.lines().collect();

    // Find the last non-empty, non-comment line
    let last_idx = lines
        .iter()
        .rposition(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .expect("Empty code");

    // Indent all code by 4 spaces for the function body
    let indented: String = lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let return_stmt = if need_return_value {
        // The last non-empty, non-comment line is the expression to return
        let last_line = lines[last_idx].trim();
        format!("\n    return {last_line}")
    } else {
        String::new()
    };

    let wrapped = format!(
        "import asyncio\nasync def __test_main():\n{indented}{return_stmt}\n__test_result__ = asyncio.run(__test_main())"
    );

    if need_return_value {
        (wrapped, Some("__test_result__".to_string()))
    } else {
        (wrapped, None)
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
///
/// When `async_mode` is true, code is wrapped in an async context before execution.
fn run_traceback_script(path: &Path, iter_mode: bool, async_mode: bool) -> String {
    Python::attach(|py| {
        let run_traceback = import_run_traceback(py);

        // Get absolute path for the test file
        let abs_path = path.canonicalize().expect("Failed to get absolute path");
        let path_str = abs_path.to_str().expect("Invalid UTF-8 in path");

        // Call run_file_and_get_traceback with the recursion limit, iter_mode, and async_mode flags
        let result = run_traceback
            .call_method1(
                "run_file_and_get_traceback",
                (path_str, TEST_RECURSION_LIMIT, iter_mode, async_mode),
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

fn format_traceback(py: Python<'_>, exc: &PyErr) -> String {
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
    async_mode: bool,
) -> Result<(), TestFailure> {
    // Skip RefCounts tests - only relevant for Monty
    if matches!(expectation, Expectation::RefCounts(_)) {
        return Ok(());
    }

    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    // Traceback tests use the external script for reliable caret line support
    if let Expectation::Traceback(expected) = expectation {
        let result = run_traceback_script(path, iter_mode, async_mode);
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

    // Use async wrapper for tests with top-level await
    let (statements, maybe_expr) = if async_mode {
        wrap_code_for_async(code, need_return_value)
    } else {
        split_code_for_module(code, need_return_value)
    };

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
                    actual: format_traceback(py, &e),
                });
            }
            if matches!(expectation, Expectation::Raise(_)) {
                return CpythonResult::Value(format_cpython_exception(py, &e));
            }
            return CpythonResult::Failed(TestFailure {
                test_name: test_name.clone(),
                kind: "CPython unexpected exception".to_string(),
                expected: "success".to_string(),
                actual: format_traceback(py, &e),
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
                            actual: format_traceback(py, &e),
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
                        actual: format_traceback(py, &e),
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

/// Result from running a test with a timeout.
enum TimeoutResult<T> {
    /// The closure completed successfully.
    Ok(T),
    /// The closure panicked with the given message.
    Panicked(String),
    /// The timeout was exceeded.
    TimedOut,
}

/// Runs a closure with a timeout, returning an error if it exceeds the duration or panics.
///
/// Spawns the closure in a separate thread and waits for the result with a timeout.
/// Distinguishes between three cases:
/// - Success: the closure returned normally
/// - Panic: the closure panicked (detected via channel disconnect + catch_unwind)
/// - Timeout: the timeout was exceeded (possible infinite loop)
///
/// Note that if a timeout occurs, the spawned thread will continue running in the
/// background (Rust doesn't support killing threads), but the test will fail immediately.
fn run_with_timeout<F, T>(timeout: Duration, f: F) -> TimeoutResult<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        // Catch panics so we can report them properly instead of as timeouts
        let result = panic::catch_unwind(AssertUnwindSafe(f));
        match result {
            Ok(value) => {
                let _ = tx.send(Ok(value));
            }
            Err(panic_payload) => {
                // Extract panic message from the payload
                let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                let _ = tx.send(Err(msg));
            }
        }
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(value)) => TimeoutResult::Ok(value),
        Ok(Err(panic_msg)) => TimeoutResult::Panicked(panic_msg),
        Err(RecvTimeoutError::Timeout) => TimeoutResult::TimedOut,
        // Disconnected without sending means something went very wrong
        Err(RecvTimeoutError::Disconnected) => {
            TimeoutResult::Panicked("thread terminated without sending result".to_string())
        }
    }
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

    // Handle timeout/panic errors from the test thread
    let result = match result {
        TimeoutResult::Ok(inner_result) => inner_result,
        TimeoutResult::Panicked(panic_msg) => Err(TestFailure {
            test_name: test_name.clone(),
            kind: "Panic".to_string(),
            expected: "no panic".to_string(),
            actual: format!("test panicked: {panic_msg}"),
        }),
        TimeoutResult::TimedOut => Err(TestFailure {
            test_name: test_name.clone(),
            kind: "Timeout".to_string(),
            expected: format!("completion within {TEST_TIMEOUT:?}"),
            actual: format!("test timed out after {TEST_TIMEOUT:?} (possible infinite loop)"),
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

    let result = try_run_cpython_test(path, &code, &expectation, config.iter_mode, config.async_mode);

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
