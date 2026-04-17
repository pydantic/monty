// Monty does not yet implement `__traceback__`, so this test cannot be a datatest.

use monty::{ExcType, MontyRun};

#[test]
fn non_ascii_earlier_line_does_not_shift_column() {
    // "x = 'é'\nundefined_name": 'é' is two UTF-8 bytes but one character,
    // so the buggy char-indexed line table reported column 2 for
    // `undefined_name`; the correct column is 1 (start of line 2).
    let code = "x = 'é'\nundefined_name".to_string();
    let run = MontyRun::new(code, "test.py", vec![]).expect("should parse");
    let err = run.run_no_limits(vec![]).expect_err("should raise NameError");
    assert_eq!(err.exc_type(), ExcType::NameError);
    let frame = err.traceback().last().expect("traceback has at least one frame");

    assert_eq!(frame.start.line, 2);
    assert_eq!(frame.start.column, 1);
    assert_eq!(frame.end.column, 15);
}

#[test]
fn non_ascii_char_column_location() {
    // "'é' + undefined_name": the non-ASCII char is on the same line as the error,
    // the nameerror should report on column 7, even though the 'é' is two UTF-8 bytes
    let code = "'é' + undefined_name".to_string();
    let run = MontyRun::new(code, "test.py", vec![]).expect("should parse");
    let err = run.run_no_limits(vec![]).expect_err("should raise NameError");
    assert_eq!(err.exc_type(), ExcType::NameError);
    let frame = err.traceback().last().expect("traceback has at least one frame");

    assert_eq!(frame.start.line, 1);
    assert_eq!(frame.start.column, 7);
    assert_eq!(frame.end.column, 21);
}
