//! The pre-parse nesting scan that bounds ruff's stack growth on long sources.
//!
//! Sources over `CompileOptions::source_scan_threshold` bytes are lexed once and
//! rejected when the estimated parser recursion exceeds the nesting limit;
//! shorter sources rely on the converter's exact check.

use insta::assert_snapshot;
use monty::{MontyRepl, MontyRun};
use monty_types::{CompileOptions, ExcType, MontyException, MontyObject, ResourceTracker, SOURCE_SCAN_THRESHOLD};

/// Compiles `code` with the given options and returns the exception it fails with.
fn parse_err_with(code: String, options: CompileOptions) -> MontyException {
    assert!(code.len() > SOURCE_SCAN_THRESHOLD || options.source_scan_threshold < SOURCE_SCAN_THRESHOLD);
    MontyRun::new(code, "test.py", vec![], options).expect_err("expected parse error")
}

#[track_caller]
fn assert_too_deeply_nested(code: String) {
    let err = parse_err_with(code, CompileOptions::default());
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message().unwrap(), "Source is too deeply nested");
}

#[track_caller]
fn assert_compiles(code: String) {
    assert!(
        code.len() > SOURCE_SCAN_THRESHOLD,
        "case must exceed the threshold to be scanned"
    );
    MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("expected the source to compile");
}

// === chains ruff recurses on, each far over the threshold ===

#[test]
fn unary_chain_is_rejected() {
    assert_too_deeply_nested(format!("{}1", "-".repeat(5000)));
}

#[test]
fn power_chain_is_rejected() {
    assert_too_deeply_nested(format!("{}2", "2 ** ".repeat(2000)));
}

#[test]
fn lambda_chain_is_rejected() {
    assert_too_deeply_nested(format!("{}1", "lambda: ".repeat(1000)));
}

#[test]
fn conditional_chain_is_rejected() {
    assert_too_deeply_nested(format!("{}1", "1 if 1 else ".repeat(1000)));
}

#[test]
fn bracket_chain_is_rejected() {
    assert_too_deeply_nested(format!("{}1{}", "(".repeat(5000), ")".repeat(5000)));
}

#[test]
fn format_spec_chain_is_rejected() {
    assert_too_deeply_nested(format!("f'{}{{x}}{}'", "{x:".repeat(1500), "}".repeat(1500)));
}

#[test]
fn indentation_chain_is_rejected() {
    let mut code = String::new();
    for depth in 0..300 {
        code.push_str(&"    ".repeat(depth));
        code.push_str("if 1:\n");
    }
    code.push_str(&"    ".repeat(300));
    code.push_str("pass\n");
    assert_too_deeply_nested(code);
}

#[test]
fn string_annotation_chain_is_rejected() {
    // ty parses a forward reference from the text between the quotes.
    assert_too_deeply_nested(format!("x: '{}1{}'", "(".repeat(5000), ")".repeat(5000)));
}

#[test]
fn triple_quoted_string_annotation_chain_is_rejected() {
    assert_too_deeply_nested(format!("x: \"\"\"\n{}1{}\"\"\"", "(".repeat(5000), ")".repeat(5000)));
}

// === long but flat sources the scan must not reject ===

#[test]
fn flat_program_compiles() {
    assert_compiles("x = 1\n".repeat(2000));
}

#[test]
fn negative_number_list_compiles() {
    assert_compiles(format!("x = [{}]", "-1, ".repeat(3000)));
}

#[test]
fn lambda_dict_compiles() {
    assert_compiles(format!("x = {{{}}}", "'k': lambda a, b: 0, ".repeat(500)));
}

#[test]
fn power_list_compiles() {
    assert_compiles(format!("x = [{}]", "2 ** 2, ".repeat(2000)));
}

#[test]
fn ordinary_strings_compile() {
    let strings = "s = 'call(a, (b)) [x]'\nt = b'((((('\nu = \"\"\"((( '(((' )))\"\"\"\n";
    assert_compiles(strings.repeat(200));
    // Four quote styles is as deep as raw source can nest string literals.
    assert_compiles(format!("x: \"\"\"'''\"'int'\"'''\"\"\"\n{}", "y = 1\n".repeat(1000)));
}

#[test]
fn function_bodies_compile() {
    let function =
        "def f(x):\n    if x:\n        return [i ** 2 for i in range(x) if -i < 0]\n    return {'k': (1, -2)}\n";
    assert_compiles(function.repeat(100));
}

// === the threshold ===

#[test]
fn zero_threshold_scans_short_sources() {
    let options = CompileOptions {
        source_scan_threshold: 0,
        ..CompileOptions::default()
    };
    // Unclosed, so only the scan reports nesting rather than ruff's missing `)`.
    let err = parse_err_with(format!("{}1", "(".repeat(250)), options);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
}

#[test]
fn max_threshold_disables_the_scan() {
    let options = CompileOptions {
        source_scan_threshold: usize::MAX,
        ..CompileOptions::default()
    };
    let err = parse_err_with(format!("{}1", "(".repeat(5000)), options);
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"unexpected EOF while parsing");
}

// === runtime compilation ===

#[test]
fn exec_of_a_long_nested_string_is_rejected() {
    let mut run = MontyRun::new(
        "exec('(' * 100_000 + '1' + ')' * 100_000)".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let err = run.run_no_limits(vec![]).expect_err("expected exec to fail");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested (<string>, line 1)");
}

#[test]
fn eval_of_a_long_nested_string_is_rejected() {
    let mut run = MontyRun::new(
        "eval('  ' + '-' * 100_000 + '1')".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let err = run.run_no_limits(vec![]).expect_err("expected eval to fail");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested (<string>, line 1)");
}

#[test]
fn exec_of_a_long_flat_string_runs() {
    let mut run = MontyRun::new(
        "exec('x = [' + '-1, ' * 3000 + ']')\nlen(x)".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::int(3000));
}

// === REPL sessions scan once, up front ===

#[test]
fn repl_check_source_rejects_before_feeding() {
    let repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    let err = repl
        .check_source(&format!("{}1", "-".repeat(5000)))
        .expect_err("expected the scan to reject");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_snapshot!(err.message().unwrap(), @"Source is too deeply nested");
    assert_eq!(err.traceback()[0].filename, "main.py");
    assert!(repl.check_source("x = [-1, -2]").is_ok());
}
