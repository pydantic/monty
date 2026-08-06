use std::thread;

use insta::assert_snapshot;
use monty_type_checking::{SourceFile, TypeChecker};
use monty_types::{TypeCheckingConfig, TypeCheckingFormat};

/// Type-checks one snippet with a throwaway checker, rendered in `Full`.
fn check(code: &str, path: &str) -> Option<String> {
    render(code, path, TypeCheckingConfig::default())
}

/// The `Concise` rendering of one snippet — one line per diagnostic, which
/// keeps multi-diagnostic snapshots readable.
fn check_concise(code: &str, path: &str) -> Option<String> {
    render(code, path, concise())
}

/// Diagnostics borrow the checker, so every helper renders before returning.
fn render(code: &str, path: &str, config: TypeCheckingConfig) -> Option<String> {
    let mut checker = TypeChecker::default();
    checker
        .run(&SourceFile::new(code, path), None, config)
        .expect("type check should not fail internally")
        .map(|diagnostics| diagnostics.to_string())
}

fn concise() -> TypeCheckingConfig {
    TypeCheckingConfig {
        format: TypeCheckingFormat::Concise,
        color: false,
    }
}

#[test]
fn type_checking_success() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, 2)
    ";

    assert!(check(code, "main.py").is_none());
}

#[test]
fn type_checking_error() {
    let code = "\
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    ";

    let error_diagnostics = check(code, "main.py").expect("expected type errors");
    assert_snapshot!(error_diagnostics, @r#"
    error[invalid-argument-type]: Argument to function `add` is incorrect
     --> main.py:4:17
      |
    2 |     return x + y
    3 |
    4 | result = add(1, '2')
      |                 ^^^ Expected `int`, found `Literal["2"]`
      |
    info: Function defined here
     --> main.py:1:5
      |
    1 | def add(x: int, y: int) -> int:
      |     ^^^         ------ Parameter declared here
    2 |     return x + y
      |
    "#);
}

#[test]
fn type_checking_error_stubs() {
    let stubs = "\
from dataclasses import dataclass

@dataclass
class User:
    name: str
    age: int
";
    let code = "\
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')";

    let mut checker = TypeChecker::default();
    let error_diagnostics = checker
        .run(
            &SourceFile::new(code, "main.py"),
            Some(&SourceFile::new(stubs, "type_stubs.pyi")),
            TypeCheckingConfig::default(),
        )
        .unwrap()
        .expect("expected type errors")
        .to_string();
    assert_snapshot!(error_diagnostics, @r#"
    error[invalid-argument-type]: Argument to function `add` is incorrect
     --> main.py:4:17
      |
    2 |     return x + y
    3 |
    4 | result = add(1, '2')
      |                 ^^^ Expected `int`, found `Literal["2"]`
      |
    info: Function defined here
     --> main.py:1:5
      |
    1 | def add(x: int, y: int) -> int:
      |     ^^^         ------ Parameter declared here
    2 |     return x + y
      |
    "#);
}

/// The format is chosen before the check runs, because the diagnostics borrow
/// the checker's database and so cannot be re-rendered by a caller that only
/// holds the returned string.
#[test]
fn type_checking_error_concise() {
    let code = r"
def add(x: int, y: int) -> int:
    return x + y

result = add(1, '2')
    ";

    assert_snapshot!(
        check_concise(code, "main.py").expect("expected type errors"),
        @r#"main.py:5:17: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["2"]`"#
    );
}

/// Colour is part of the same up-front choice; only `Full` and `Concise`
/// render any.
#[test]
fn type_checking_error_color() {
    let code = "x: int = 'nope'\n";

    let colored = render(
        code,
        "main.py",
        TypeCheckingConfig {
            color: true,
            ..concise()
        },
    )
    .expect("expected type errors");
    assert!(colored.starts_with('\u{1b}'), "expected ANSI escapes, got: {colored}");
}

/// Every format name round-trips through the checker; the machine-readable
/// ones are what a host parses instead of scraping `Full`.
#[test]
fn every_format_renders() {
    let code = "x: int = 'nope'\n";

    let rendered =
        |format| render(code, "main.py", TypeCheckingConfig { format, color: false }).expect("expected type errors");

    assert_snapshot!(rendered(TypeCheckingFormat::Concise), @r#"main.py:1:10: error[invalid-assignment] Object of type `Literal["nope"]` is not assignable to `int`"#);
    assert_snapshot!(rendered(TypeCheckingFormat::Pylint), @r#"main.py:1: [invalid-assignment] Object of type `Literal["nope"]` is not assignable to `int`"#);
    assert_snapshot!(rendered(TypeCheckingFormat::Azure), @r#"##vso[task.logissue type=error;sourcepath=/main.py;linenumber=1;columnnumber=10;code=invalid-assignment;]Object of type `Literal["nope"]` is not assignable to `int`"#);
    assert_snapshot!(rendered(TypeCheckingFormat::Github), @r#"::error title=monty (invalid-assignment),file=/main.py,line=1,col=10,endLine=1,endColumn=16::main.py:1:10: invalid-assignment: Object of type `Literal["nope"]` is not assignable to `int`%0A  main.py:1:4: Declared type"#);
    assert!(rendered(TypeCheckingFormat::Json).starts_with('['));
    assert!(rendered(TypeCheckingFormat::JsonLines).starts_with('{'));
    assert!(rendered(TypeCheckingFormat::Rdjson).starts_with('{'));
    assert!(rendered(TypeCheckingFormat::Gitlab).starts_with('['));
}

#[test]
fn stdlib_datetime_resolves() {
    let code = "import datetime\nprint(datetime.datetime.now())";

    let result = check(code, "main.py");
    assert!(result.is_none(), "Expected no type errors, got: {result:#?}");
}

/// Test that good_types.py type-checks without errors.
///
/// This file uses `assert_type` from typing to verify that inferred types match expected types.
#[test]
fn type_good_types() {
    let result = check(include_str!("good_types.py"), "good_types.py");
    assert!(result.is_none(), "Expected no type errors, got: {result:#?}");
}

/// Test that bad_types.py produces the expected type errors.
///
/// Snapshot stored in `tests/snapshots/main__type_bad_types.snap`.
/// Run `cargo insta review` (or `cargo insta test --accept`) to update.
#[test]
fn type_bad_types() {
    let actual = check_concise(include_str!("bad_types.py"), "bad_types.py").expect("Expected type errors");
    assert_snapshot!(actual);
}

/// Security-critical: a checker reused across runs must evaluate each run
/// against its own source. This is the session case — every feed rewrites the
/// same script path — so a stale Salsa memo would let a name defined by an
/// earlier snippet keep resolving after it was edited away.
#[test]
fn same_path_rerun_does_not_see_previous_source() {
    let mut checker = TypeChecker::default();
    let first = checker
        .run(
            &SourceFile::new("GOOD: int = 1\nresult = GOOD + 1\n", "main.py"),
            None,
            concise(),
        )
        .unwrap()
        .map(|d| d.to_string());
    assert!(first.is_none(), "first run should succeed: {first:#?}");

    // Same path, new source: `GOOD` existed only in the previous run.
    let second = checker
        .run(&SourceFile::new("x: int = GOOD\n", "main.py"), None, concise())
        .unwrap()
        .expect("second run must error — `GOOD` was only defined in the first run")
        .to_string();
    assert_snapshot!(second, @"main.py:1:10: error[unresolved-reference] Name `GOOD` used when not defined");
}

/// Security-critical: `reset` must scrub every file a session wrote, so a
/// worker recycled onto the next session cannot import the previous one's
/// modules.
#[test]
fn reset_removes_previous_modules() {
    let mut checker = TypeChecker::default();
    let first = checker
        .run(&SourceFile::new("SECRET = 'hunter2'\n", "leaky.py"), None, concise())
        .unwrap()
        .map(|d| d.to_string());
    assert!(first.is_none(), "first run should succeed: {first:#?}");
    checker.reset().expect("reset");

    let second = checker
        .run(
            &SourceFile::new("from leaky import SECRET\nprint(SECRET)\n", "main.py"),
            None,
            concise(),
        )
        .unwrap()
        .expect("second run must raise unresolved-import for the scrubbed module")
        .to_string();
    assert_snapshot!(second, @"main.py:1:6: error[unresolved-import] Cannot resolve imported module `leaky`");
}

/// Security-critical: stubs are files too — they must not survive `reset`
/// either, or the next session could import the previous session's stubs.
#[test]
fn reset_removes_stubs() {
    let mut checker = TypeChecker::default();
    let first = checker
        .run(
            &SourceFile::new("from type_stubs import Widget\nw: Widget\n", "main.py"),
            Some(&SourceFile::new("class Widget:\n    x: int\n", "type_stubs.pyi")),
            concise(),
        )
        .unwrap()
        .map(|d| d.to_string());
    assert!(first.is_none(), "first run with stubs should succeed: {first:#?}");
    checker.reset().expect("reset");

    let second = checker
        .run(
            &SourceFile::new("from type_stubs import Widget\n", "main.py"),
            None,
            concise(),
        )
        .unwrap()
        .expect("second run must error — `type_stubs` was only provided in the first run")
        .to_string();
    assert_snapshot!(second, @"main.py:1:6: error[unresolved-import] Cannot resolve imported module `type_stubs`");
}

/// Security-critical: nested paths must be accepted and fully scrubbed
/// (including the intermediate directories) so the next session cannot see
/// the previous one's module structure.
#[test]
fn reset_removes_nested_paths() {
    let mut checker = TypeChecker::default();
    let first = checker
        .run(
            &SourceFile::new("LEAKY: int = 1\n", "sub_dir/leaky.py"),
            None,
            concise(),
        )
        .unwrap()
        .map(|d| d.to_string());
    assert!(first.is_none(), "first nested-path run should succeed: {first:#?}");
    checker.reset().expect("reset");

    // A different nested path importing the scrubbed module: the containing
    // directory must be gone too, not just the file.
    let second = checker
        .run(
            &SourceFile::new("from sub_dir.leaky import LEAKY\n", "other.py"),
            None,
            concise(),
        )
        .unwrap()
        .expect("second run must error — sub_dir/leaky.py was scrubbed by reset")
        .to_string();
    assert_snapshot!(second, @"other.py:1:6: error[unresolved-import] Cannot resolve imported module `sub_dir.leaky`");
}

/// Security-critical: checkers hold no shared state, so concurrent sessions
/// (workers in one process, or just several threads) must never observe each
/// other's files. Guards against a process-wide cache creeping back in.
#[test]
fn concurrent_checkers_stay_isolated() {
    thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|thread_idx| {
                scope.spawn(move || {
                    let mut checker = TypeChecker::default();
                    for iter in 0..5 {
                        let defines = format!("T_{thread_idx}_{iter}: int = 1\n");
                        let ok = checker
                            .run(&SourceFile::new(&defines, "main.py"), None, concise())
                            .unwrap()
                            .map(|d| d.to_string());
                        assert!(ok.is_none(), "ok run must succeed: {ok:#?}");

                        // Same path, so the previous run's definition is gone;
                        // another thread's would only show up through shared state.
                        let probe = format!("x: int = T_{thread_idx}_{iter}\n");
                        let diagnostics = checker
                            .run(&SourceFile::new(&probe, "main.py"), None, concise())
                            .unwrap()
                            .expect("leak probe must error — prior run's name must not be visible")
                            .to_string();
                        assert_eq!(
                            diagnostics,
                            format!(
                                "main.py:1:10: error[unresolved-reference] Name `T_{thread_idx}_{iter}` used when not defined\n"
                            ),
                        );
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
}

/// Snapshot stored in `tests/snapshots/main__reveal_types.snap`.
/// Run `cargo insta review` (or `cargo insta test --accept`) to update.
#[test]
fn test_reveal_types() {
    let actual = check_concise(include_str!("reveal_types.py"), "reveal_types.py").expect("Expected type errors");
    assert_snapshot!(actual);
}

#[test]
fn deeply_nested_parentheses_do_not_stack_overflow() {
    let depth = 500;
    let mut code = String::with_capacity(depth * 2 + 1);
    for _ in 0..depth {
        code.push('(');
    }
    code.push('1');
    for _ in 0..depth {
        code.push(')');
    }

    assert_snapshot!(
        check_concise(&code, "main.py").unwrap(),
        @"
    main.py:1:203: error[invalid-syntax] Source is too deeply nested
    main.py:1:1002: error[invalid-syntax] Expected `)`, found end of file
    "
    );
}

/// The narrowed `collections` stub must expose exactly what Monty implements at
/// runtime — importing the four implemented names type-checks clean.
#[test]
fn collections_implemented_names_type_check() {
    let code = "from collections import deque, Counter, defaultdict, namedtuple\n";
    let result = check(code, "main.py");
    assert!(result.is_none(), "expected no type errors, got: {result:?}");
}

/// Conversely, names Monty does not implement (`OrderedDict`, `ChainMap`, the
/// `User*` wrappers) are removed from the stub, so importing them is a type
/// error rather than passing the checker and then failing at runtime with
/// `ImportError`.
#[test]
fn collections_unimplemented_names_are_unresolved() {
    let code = "from collections import OrderedDict, ChainMap, UserDict, UserList, UserString\n";
    let diagnostics = check(code, "main.py").expect("expected unresolved-import errors");
    assert_snapshot!(diagnostics, @"
    error[unresolved-import]: Module `collections` has no member `OrderedDict`
     --> main.py:1:25
      |
    1 | from collections import OrderedDict, ChainMap, UserDict, UserList, UserString
      |                         ^^^^^^^^^^^
      |

    error[unresolved-import]: Module `collections` has no member `ChainMap`
     --> main.py:1:38
      |
    1 | from collections import OrderedDict, ChainMap, UserDict, UserList, UserString
      |                                      ^^^^^^^^
      |

    error[unresolved-import]: Module `collections` has no member `UserDict`
     --> main.py:1:48
      |
    1 | from collections import OrderedDict, ChainMap, UserDict, UserList, UserString
      |                                                ^^^^^^^^
      |

    error[unresolved-import]: Module `collections` has no member `UserList`
     --> main.py:1:58
      |
    1 | from collections import OrderedDict, ChainMap, UserDict, UserList, UserString
      |                                                          ^^^^^^^^
      |

    error[unresolved-import]: Module `collections` has no member `UserString`
     --> main.py:1:68
      |
    1 | from collections import OrderedDict, ChainMap, UserDict, UserList, UserString
      |                                                                    ^^^^^^^^^^
      |
    ");
}
