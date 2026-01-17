use monty::{CollectStringPrint, MontyRun, NoLimitTracker, NoPrint};

#[test]
fn print_single_string() {
    let ex = MontyRun::new("print('hello')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "hello\n");
}

#[test]
fn print_multiple_args() {
    let ex = MontyRun::new("print('hello', 'world')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "hello world\n");
}

#[test]
fn print_multiple_statements() {
    let ex = MontyRun::new(
        "print('one')\nprint('two')\nprint('three')".to_owned(),
        "test.py",
        vec![],
        vec![],
    )
    .unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "one\ntwo\nthree\n");
}

#[test]
fn print_empty() {
    let ex = MontyRun::new("print()".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "\n");
}

#[test]
fn print_integers() {
    let ex = MontyRun::new("print(1, 2, 3)".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "1 2 3\n");
}

#[test]
fn print_mixed_types() {
    let ex = MontyRun::new("print('count:', 42, True)".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "count: 42 True\n");
}

#[test]
fn print_in_function() {
    let code = "
def greet(name):
    print('Hello', name)

greet('Alice')
greet('Bob')
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "Hello Alice\nHello Bob\n");
}

#[test]
fn print_in_loop() {
    let code = "
for i in range(3):
    print(i)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "0\n1\n2\n");
}

#[test]
fn into_output_consumes_writer() {
    let ex = MontyRun::new("print('test')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    let output: String = writer.into_output();
    assert_eq!(output, "test\n");
}

#[test]
fn writer_reuse_accumulates() {
    let mut writer = CollectStringPrint::new();

    let ex1 = MontyRun::new("print('first')".to_owned(), "test.py", vec![], vec![]).unwrap();
    ex1.run(vec![], NoLimitTracker, &mut writer).unwrap();

    let ex2 = MontyRun::new("print('second')".to_owned(), "test.py", vec![], vec![]).unwrap();
    ex2.run(vec![], NoLimitTracker, &mut writer).unwrap();

    assert_eq!(writer.output(), "first\nsecond\n");
}

#[test]
fn no_print_suppresses_output() {
    let code = "
for i in range(100):
    print('this should be suppressed', i)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = NoPrint;
    // Should complete without error, output is silently discarded
    let result = ex.run(vec![], NoLimitTracker, &mut writer);
    assert!(result.is_ok());
}

// === print() kwargs tests ===

#[test]
fn print_custom_sep() {
    let ex = MontyRun::new("print('a', 'b', 'c', sep='-')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "a-b-c\n");
}

#[test]
fn print_custom_end() {
    let ex = MontyRun::new("print('hello', end='!')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "hello!");
}

#[test]
fn print_custom_sep_and_end() {
    let ex = MontyRun::new(
        "print('x', 'y', 'z', sep=', ', end='\\n---\\n')".to_owned(),
        "test.py",
        vec![],
        vec![],
    )
    .unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "x, y, z\n---\n");
}

#[test]
fn print_empty_sep() {
    let ex = MontyRun::new("print('a', 'b', 'c', sep='')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "abc\n");
}

#[test]
fn print_empty_end() {
    let code = "print('first', end='')\nprint('second')";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "firstsecond\n");
}

#[test]
fn print_sep_none() {
    // sep=None should use default space
    let ex = MontyRun::new("print('a', 'b', sep=None)".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    // In Python, sep=None means use default, but we treat it as empty string for simplicity
    // This matches: print('a', 'b', sep=None) outputs "ab\n" with our impl
    assert_eq!(writer.output(), "a b\n");
}

#[test]
fn print_end_none() {
    // end=None should use empty string (our interpretation)
    let ex = MontyRun::new("print('hello', end=None)".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "hello\n");
}

#[test]
fn print_flush_ignored() {
    // flush=True should be accepted but ignored
    let ex = MontyRun::new("print('test', flush=True)".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "test\n");
}

#[test]
fn print_kwargs_dict() {
    // Use a dict literal instead of dict() since dict builtin is not implemented
    let ex = MontyRun::new("print('a', 'b', **{'sep': '-'})".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "a-b\n");
}

#[test]
fn print_only_kwargs_no_args() {
    let ex = MontyRun::new("print(sep='-', end='!')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "!");
}

#[test]
fn print_multiline_sep() {
    let ex = MontyRun::new("print(1, 2, 3, sep='\\n')".to_owned(), "test.py", vec![], vec![]).unwrap();
    let mut writer = CollectStringPrint::new();
    ex.run(vec![], NoLimitTracker, &mut writer).unwrap();
    assert_eq!(writer.output(), "1\n2\n3\n");
}
