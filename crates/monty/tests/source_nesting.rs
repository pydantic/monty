//! The pre-parse nesting scan that bounds ruff's stack growth on long sources.
//!
//! Sources over `CompileOptions::source_scan_threshold` bytes are lexed once and
//! rejected when the estimated parser recursion exceeds the nesting limit;
//! shorter sources rely on the converter's exact check.
//!
//! The scan cannot see how deep an AST ruff builds from flat source, so every
//! parse also measures the AST and tears a deep one down without recursion.

use insta::assert_snapshot;
use monty::{
    MontyRepl, MontyRun, ReplContinuationMode, ReplProgress, detect_repl_continuation_mode,
    source_within_nesting_bound, type_check_nesting_exception,
};
use monty_types::{
    CompileOptions, ExcType, MontyException, MontyObject, PrintWriter, ResourceTracker, SOURCE_SCAN_THRESHOLD,
};

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
fn case_pattern_chain_is_rejected() {
    let source = format!("match x:\n    case {}1j:\n        pass\n", "1+".repeat(3000));
    // Monty rejects `match` itself, so check the scan's verdict directly too.
    assert!(!source_within_nesting_bound(&source, 0));
    assert_too_deeply_nested(source);
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

// === flat sources ruff parses to a deep AST ===
//
// None nests a bracket, an indent or a prefix operator, so each passes the
// scan, and dropping its AST once overflowed the native stack.

/// How many links each chain below has: enough to overflow a test thread's
/// stack if the AST were ever dropped or walked recursively.
const LINKS: usize = 50_000;

#[test]
fn operator_chain_is_rejected() {
    assert_too_deeply_nested(format!("x = 1{}", " + 1".repeat(LINKS)));
    assert_too_deeply_nested(format!("x = a{}", " | a".repeat(LINKS)));
    assert_too_deeply_nested(format!("x = 1{}", " * 1 - 1".repeat(LINKS)));
}

#[test]
fn postfix_chain_is_rejected() {
    assert_too_deeply_nested(format!("y = a{}", ".x".repeat(LINKS)));
    assert_too_deeply_nested(format!("y = f{}", "()".repeat(LINKS)));
    assert_too_deeply_nested(format!("y = d{}", "[0]".repeat(LINKS)));
}

#[test]
fn chain_after_another_error_is_rejected() {
    // The converter stops at the first statement, leaving the second to be dropped.
    assert_too_deeply_nested(format!("del x\ny = 1{}", " + 1".repeat(LINKS)));
}

#[test]
fn ast_recovered_from_a_syntax_error_is_rejected() {
    // ruff parses on past each error, nesting once per repeat.
    for unit in [
        "not a < ",
        "*a < ",
        "*a and ",
        "lambda @ ",
        "yield a + ",
        "a if >> / ",
        "- is % ",
    ] {
        assert_too_deeply_nested(format!("{}a", unit.repeat(LINKS)));
    }
    assert_too_deeply_nested(format!("x = 1{}", " |".repeat(LINKS)));
}

#[test]
fn chain_within_the_limit_compiles() {
    assert_compiles("x = a.b(1)[2].c + 3 * 4 - 5 | 6\n".repeat(400));
    assert_compiles(format!("x = [{}]", "a.b + c.d(1), ".repeat(1000)));
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
fn case_variable_compiles() {
    assert_compiles("case = case - 1 + case\n".repeat(400));
}

#[test]
fn case_alternatives_pass_the_scan() {
    // Monty rejects `match` itself, so only the scan's verdict is checked.
    let source = format!("match x:\n    case {}1j:\n        pass\n", "1+1j | -1-".repeat(2000));
    assert!(source_within_nesting_bound(&source, 0));
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
fn eval_and_exec_of_a_long_operator_chain_are_rejected() {
    for code in [
        "eval('1' + '+1' * 400_000)",
        "exec('x = 1' + '+1' * 400_000)",
        // ruff drops the AST of an expression that fails to parse itself
        "eval('1' + '+1' * 400_000 + '+')",
        "eval('not a < ' * 100_000 + 'a')",
    ] {
        let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
        let err = run.run_no_limits(vec![]).expect_err("expected the snippet to fail");
        assert_eq!(err.exc_type(), ExcType::SyntaxError);
        assert_eq!(err.message().unwrap(), "Source is too deeply nested (<string>, line 1)");
    }
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

#[test]
fn repl_feeds_scan_unless_given_a_checked_source() {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    let deep = format!("{}1", "-".repeat(5000));
    let flat = "x = [-1, -2]\n".repeat(400);

    let err = repl
        .feed_run(&deep, vec![], PrintWriter::Disabled)
        .expect_err("expected the feed to reject");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message().unwrap(), "Source is too deeply nested");

    let checked = repl.check_source(&flat).unwrap();
    let progress = repl.feed_start_checked(checked, vec![], PrintWriter::Disabled).unwrap();
    assert!(matches!(progress, ReplProgress::Complete { .. }));
}

#[test]
fn repl_rejects_a_long_operator_chain() {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    let chain = format!("x = 1{}", " + 1".repeat(LINKS));
    // Complete, so that feeding it reports the error.
    assert_eq!(detect_repl_continuation_mode(&chain), ReplContinuationMode::Complete);
    let err = repl
        .feed_run(&chain, vec![], PrintWriter::Disabled)
        .expect_err("expected the feed to reject");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message().unwrap(), "Source is too deeply nested");
}

// === type checking ===

#[track_caller]
fn assert_type_check_rejects(code: &str) {
    let err = type_check_nesting_exception(code, "test.py", SOURCE_SCAN_THRESHOLD).expect_err("expected rejection");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(err.message().unwrap(), "Source is too deeply nested");
}

#[test]
fn type_check_rejects_deep_asts_from_flat_source() {
    assert_type_check_rejects(&format!("x = {}", vec!["1"; 5000].join("+")));
    assert_type_check_rejects(&format!("y = a{}", ".x".repeat(5000)));
    // annotations the compiler drops unchecked
    assert_type_check_rejects(&format!("def f(a: {}): ...", vec!["int"; 1000].join(" | ")));
    assert_type_check_rejects(&format!("def f() -> {}: ...", vec!["int"; 1000].join(" | ")));
    // ty parses string annotations, continuing the depth around them
    assert_type_check_rejects(&format!("x: list['{}']", vec!["int"; 1000].join(" | ")));
    assert_type_check_rejects(&format!("x: \"list['{}']\"", vec!["int"; 1000].join(" | ")));
    // ty also type-checks the AST it recovers from a syntax error
    assert_type_check_rejects(&format!("x = {} +", vec!["1"; 1000].join("+")));
}

#[test]
fn type_check_rejects_asts_too_deep_to_drop() {
    assert_type_check_rejects(&format!("x = 1{}", " + 1".repeat(LINKS)));
    assert_type_check_rejects(&format!("{}a", "not a < ".repeat(LINKS)));
    assert_type_check_rejects(&format!("x: '1{}'", " + 1".repeat(LINKS)));
    // ty drops the AST of an annotation that fails to parse without walking it
    assert_type_check_rejects(&format!("x: '1{} +'", " + 1".repeat(LINKS)));
    assert_type_check_rejects(&format!("x: \"\"\"\n{}\"\"\"", "/usr/lib\n".repeat(LINKS)));
}

#[test]
fn type_check_accepts_data_strings_that_do_not_parse() {
    // Each recovers to a chain, but ty only parses a literal used as an annotation.
    for text in ["/usr/lib/python3 ".repeat(300), "0x1f+0x2a-".repeat(300)] {
        let code = format!("s = '{text}'\n");
        assert!(type_check_nesting_exception(&code, "test.py", SOURCE_SCAN_THRESHOLD).is_ok());
    }
}

#[test]
fn type_check_accepts_shallow_sources() {
    let shallow =
        "def f(a: int | str, b: 'list[int]') -> dict[str, list[int]]:\n    return {'k': [a.x.y for _ in b]}\n";
    assert!(type_check_nesting_exception(&shallow.repeat(200), "test.py", SOURCE_SCAN_THRESHOLD).is_ok());
    // a long string that is not deep as an expression
    let prose = format!("x = '{}'", "word ".repeat(2000));
    assert!(type_check_nesting_exception(&prose, "test.py", SOURCE_SCAN_THRESHOLD).is_ok());
}

#[test]
fn type_check_rejection_is_located() {
    let code = format!("x = 1\ny = {}\n", vec!["1"; 1000].join("+"));
    let err = type_check_nesting_exception(&code, "test.py", SOURCE_SCAN_THRESHOLD).unwrap_err();
    assert_eq!(err.traceback()[0].start.line, 2);
}
