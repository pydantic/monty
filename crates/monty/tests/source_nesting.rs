//! The pre-parse nesting scan that bounds ruff's stack growth on long sources.
//!
//! Sources over `CompileOptions::source_scan_threshold` bytes are lexed once and
//! rejected when the estimated parser recursion exceeds the nesting limit;
//! shorter sources rely on the converter's exact check.

use insta::assert_snapshot;
use monty::{MontyRepl, MontyRun, ReplProgress, source_within_nesting_bound, type_check_nesting_exception};
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
fn yield_from_chain_keeps_recursive_operands_charged() {
    let source = format!("{}value", "yield from ".repeat(10_000));
    assert!(!source_within_nesting_bound(&source, 0));
    assert_too_deeply_nested(source);
}

#[test]
fn long_postfix_spines_are_rejected_before_ast_construction() {
    for source in [
        format!("value{}", ".x".repeat(10_000)),
        format!("value{}", "()".repeat(10_000)),
        format!("value{}", "[0]".repeat(10_000)),
        format!("def f[T: value{}](): pass", ".x".repeat(10_000)),
        format!("import a as b{}", ".x".repeat(10_000)),
        format!("(value{}\n)", "()\n".repeat(10_000)),
        format!("import a as b{}", format!("{}(0, 0)", ".x".repeat(32)).repeat(150)),
    ] {
        assert!(!source_within_nesting_bound(&source, 0));
        assert_too_deeply_nested(source);
    }
}

#[test]
fn grouping_keeps_completed_postfix_operands_charged() {
    let groups = if cfg!(debug_assertions) { 29 } else { 199 };
    for separator in [")", ", 0)", ", value=0)"] {
        let mut source = if separator.contains("value=") {
            "call(".repeat(groups)
        } else {
            "(".repeat(groups)
        };
        source.push_str("value");
        for depth in 1..=groups {
            source.push_str(&".x".repeat(depth));
            source.push_str(separator);
        }
        source.push_str(&".x".repeat(groups + 1));
        source.push_str("\n#");
        source.push_str(&"padding".repeat(700));
        assert!(!source_within_nesting_bound(&source, 0));
        assert_too_deeply_nested(source);
    }

    let shallow = format!("{}value.x{}", "(".repeat(groups), ")".repeat(groups));
    assert!(source_within_nesting_bound(&shallow, 0));
}

#[test]
fn recovered_keyword_attributes_keep_postfix_depth_charged() {
    for keyword in [
        "as", "from", "if", "for", "in", "is", "and", "or", "not", "else", "case", "await",
    ] {
        let source = format!("value{}", format!(".{keyword}").repeat(10_000));
        assert!(!source_within_nesting_bound(&source, 0), "{keyword}");
        assert_too_deeply_nested(source);
    }
}

#[test]
fn soft_keywords_end_postfix_operands() {
    let depth = if cfg!(debug_assertions) { 18 } else { 110 };
    for name in ["case", "lazy", "match", "type"] {
        for suffix in [format!("().{name}"), format!("[0].{name}")] {
            let source = format!("{name}{}", suffix.repeat(depth));
            assert!(!source_within_nesting_bound(&source, 0), "{name}");
        }
    }
}

#[test]
fn independent_postfix_expressions_do_not_accumulate_depth() {
    let depth = if cfg!(debug_assertions) { 18 } else { 110 };
    let left = format!("a{}", ".x".repeat(depth));
    let right = format!("b{}", ".y".repeat(depth));
    for source in [
        format!("{left} = {right}"),
        format!("{left} += {right}"),
        format!("with {left} as {right}: pass"),
        format!("raise {left} from {right}"),
    ] {
        assert!(source_within_nesting_bound(&source, 0), "{source}");
        MontyRun::new(
            source.clone(),
            "test.py",
            vec![],
            CompileOptions {
                source_scan_threshold: 0,
                ..CompileOptions::default()
            },
        )
        .expect(&source);
    }
}

#[test]
fn long_import_paths_are_not_postfix_spines() {
    let component = "module".repeat(100);
    let module = vec![component.as_str(); 32].join(".");
    for source in [
        format!("import {module}"),
        format!("from {module} import value"),
        format!("import {module}, {module} as alias, {module} as other"),
    ] {
        for prefix in ["", "\n", "# comment\n"] {
            assert_compiles(format!("{prefix}{source}"));
        }
    }
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
fn type_check_bounds_short_format_specs_and_patterns() {
    for prefix in ["f", "t"] {
        let source = format!("{prefix}'{}{{x}}{}'", "{x:".repeat(400), "}".repeat(400));
        assert!(source.len() < SOURCE_SCAN_THRESHOLD);
        assert_type_check_rejects(&source);
    }
    let source = format!("match x:\n    case {}y{}: pass", "[".repeat(250), "]".repeat(250));
    assert!(source.len() < SOURCE_SCAN_THRESHOLD);
    assert_type_check_rejects(&source);
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
