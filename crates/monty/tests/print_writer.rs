//! Tests for `PrintWriter::CollectString` — the output capture mode used by
//! hosts that need to intercept `print()` rather than pass it through to
//! stdout.
//!
//! Expected output is asserted via [`insta::assert_snapshot!`] with inline
//! snapshots — multi-line print output renders naturally in the snapshot
//! literal, which is substantially clearer than an escaped `"a\nb\n"` string.
//! To update after an intentional change, run `cargo insta review` (or set
//! `INSTA_UPDATE=always`).

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CollectedStreams, CompileOptions, PrintStream, PrintWriter, ResourceTracker};

/// Run `code` under Monty with a string-collecting `PrintWriter` and return
/// whatever was printed. Panics on parse/runtime errors — callers only care
/// about the captured output. `CollectString` keeps no stream labels, so
/// stderr output lands in the same buffer; see `run_and_capture_streams`.
fn run_and_capture(code: &str) -> String {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut output = String::new();
    ex.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_string(&mut output),
    )
    .unwrap();
    output
}

#[test]
fn print_single_string() {
    assert_snapshot!(run_and_capture("print('hello')"), @"hello
");
}

#[test]
fn print_multiple_args() {
    assert_snapshot!(run_and_capture("print('hello', 'world')"), @"hello world
");
}

#[test]
fn print_multiple_statements() {
    assert_snapshot!(
        run_and_capture("print('one')\nprint('two')\nprint('three')"),
        @r"
    one
    two
    three
    "
    );
}

#[test]
fn print_empty() {
    assert_snapshot!(run_and_capture("print()"), @"
");
}

#[test]
fn print_integers() {
    assert_snapshot!(run_and_capture("print(1, 2, 3)"), @"1 2 3
");
}

#[test]
fn print_mixed_types() {
    assert_snapshot!(run_and_capture("print('count:', 42, True)"), @"count: 42 True
");
}

#[test]
fn print_in_function() {
    let code = "
def greet(name):
    print('Hello', name)

greet('Alice')
greet('Bob')
";
    assert_snapshot!(run_and_capture(code), @r"
    Hello Alice
    Hello Bob
    ");
}

#[test]
fn print_in_loop() {
    let code = "
for i in range(3):
    print(i)
";
    assert_snapshot!(run_and_capture(code), @r"
    0
    1
    2
    ");
}

#[test]
fn collect_output_accessible_after_run() {
    assert_snapshot!(run_and_capture("print('test')"), @"test
");
}

#[test]
fn writer_reuse_accumulates() {
    let mut output = String::new();

    let ex1 = MontyRun::new(
        "print('first')".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    ex1.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_string(&mut output),
    )
    .unwrap();

    let ex2 = MontyRun::new(
        "print('second')".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    ex2.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_string(&mut output),
    )
    .unwrap();

    assert_snapshot!(output, @r"
    first
    second
    ");
}

#[test]
fn disabled_suppresses_output() {
    let code = "
for i in range(100):
    print('this should be suppressed', i)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    // Should complete without error, output is silently discarded
    let result = ex.run(vec![], ResourceTracker::default(), PrintWriter::Disabled);
    assert!(result.is_ok());
}

// === print() kwargs tests ===

#[test]
fn print_custom_sep() {
    assert_snapshot!(run_and_capture("print('a', 'b', 'c', sep='-')"), @"a-b-c
");
}

#[test]
fn print_custom_end() {
    assert_snapshot!(run_and_capture("print('hello', end='!')"), @"hello!");
}

#[test]
fn print_custom_sep_and_end() {
    assert_snapshot!(
        run_and_capture("print('x', 'y', 'z', sep=', ', end='\\n---\\n')"),
        @r"
    x, y, z
    ---
    "
    );
}

#[test]
fn print_empty_sep() {
    assert_snapshot!(run_and_capture("print('a', 'b', 'c', sep='')"), @"abc
");
}

#[test]
fn print_empty_end() {
    assert_snapshot!(
        run_and_capture("print('first', end='')\nprint('second')"),
        @"firstsecond
    "
    );
}

#[test]
fn print_sep_none() {
    // sep=None should use default space
    assert_snapshot!(run_and_capture("print('a', 'b', sep=None)"), @"a b
");
}

#[test]
fn print_end_none() {
    // end=None should use empty string (our interpretation)
    assert_snapshot!(run_and_capture("print('hello', end=None)"), @"hello
");
}

#[test]
fn print_flush_ignored() {
    // flush=True should be accepted but ignored
    assert_snapshot!(run_and_capture("print('test', flush=True)"), @"test
");
}

#[test]
fn print_kwargs_dict() {
    // Use a dict literal instead of dict() since dict builtin is not implemented
    assert_snapshot!(run_and_capture("print('a', 'b', **{'sep': '-'})"), @"a-b
");
}

#[test]
fn print_only_kwargs_no_args() {
    assert_snapshot!(run_and_capture("print(sep='-', end='!')"), @"!");
}

#[test]
fn print_multiline_sep() {
    assert_snapshot!(run_and_capture("print(1, 2, 3, sep='\\n')"), @r"
    1
    2
    3
    ");
}

// === file= routing ===

/// Run `code` and return the labelled fragments, so a test can tell which
/// stream each one went to.
fn run_and_capture_streams(code: &str) -> Vec<(PrintStream, String)> {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut streams = CollectedStreams::default();
    ex.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_streams(&mut streams),
    )
    .unwrap();
    streams.into_entries()
}

#[test]
fn print_file_stderr_is_labelled_stderr() {
    assert_eq!(
        run_and_capture_streams("import sys\nprint('oops', file=sys.stderr)"),
        vec![(PrintStream::Stderr, "oops\n".to_owned())]
    );
}

#[test]
fn print_file_stdout_matches_the_default() {
    assert_eq!(
        run_and_capture_streams("import sys\nprint('a', file=sys.stdout)\nprint('b')"),
        vec![(PrintStream::Stdout, "a\nb\n".to_owned())]
    );
}

/// Consecutive fragments merge per stream, so alternating between the two
/// produces one entry per switch and keeps them in the order printed.
#[test]
fn print_alternating_streams_keep_their_order() {
    assert_eq!(
        run_and_capture_streams("import sys\nprint('1')\nprint('2', file=sys.stderr)\nprint('3')"),
        vec![
            (PrintStream::Stdout, "1\n".to_owned()),
            (PrintStream::Stderr, "2\n".to_owned()),
            (PrintStream::Stdout, "3\n".to_owned()),
        ]
    );
}

/// `sep` and `end` follow the file the call names, not the default stream.
#[test]
fn print_sep_and_end_follow_the_file() {
    assert_eq!(
        run_and_capture_streams("import sys\nprint('a', 'b', sep='-', end='!', file=sys.stderr)"),
        vec![(PrintStream::Stderr, "a-b!".to_owned())]
    );
}

#[test]
fn print_file_other_than_a_sys_stream_raises() {
    let ex = MontyRun::new(
        "print('x', file=42)".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run(vec![], ResourceTracker::default(), PrintWriter::Disabled)
        .expect_err("only sys.stdout and sys.stderr can be written to");
    assert_snapshot!(
        err.message().unwrap(),
        @"print() 'file' argument must be sys.stdout or sys.stderr, not int"
    );
}
