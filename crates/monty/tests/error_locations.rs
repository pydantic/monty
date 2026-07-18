// Monty does not yet implement `__traceback__`, so this test cannot be a datatest.

use std::sync::Arc;

use monty::{CompileOptions, ExcType, LimitedTracker, MontyRun, PrintWriter, ResourceLimits};

#[test]
fn non_ascii_earlier_line_does_not_shift_column() {
    // "x = 'é'\nundefined_name": 'é' is two UTF-8 bytes but one character,
    // so the buggy char-indexed line table reported column 2 for
    // `undefined_name`; the correct column is 1 (start of line 2).
    let code = "x = 'é'\nundefined_name".to_string();
    let run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("should parse");
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
    let run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("should parse");
    let err = run.run_no_limits(vec![]).expect_err("should raise NameError");
    assert_eq!(err.exc_type(), ExcType::NameError);
    let frame = err.traceback().last().expect("traceback has at least one frame");

    assert_eq!(frame.start.line, 1);
    assert_eq!(frame.start.column, 7);
    assert_eq!(frame.end.column, 21);
}

#[test]
fn recursive_frames_share_preview_line_allocation() {
    // Frames at the same source location must share a single `Arc<str>`
    // backing `preview_line`. Without sharing, a 1 MiB line on the error
    // site with 1000 recursive frames would allocate ~1 GiB during
    // exception construction — outside the VM's resource accounting, since
    // these allocations are in standard Rust memory rather than the managed
    // heap. Arc sharing keeps the worst case at ~1 MiB.
    let code = r"
def recurse(n):
    return recurse(n - 1)
recurse(50)
";
    let run = MontyRun::new(code.to_string(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    let limits = ResourceLimits::new().max_recursion_depth(Some(10));
    let err = run
        .run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .expect_err("should exceed recursion depth");

    assert_eq!(err.exc_type(), ExcType::RecursionError);

    let recurse_frames: Vec<_> = err
        .traceback()
        .iter()
        .filter(|f| f.frame_name.as_deref() == Some("recurse"))
        .collect();
    assert!(
        recurse_frames.len() >= 5,
        "expected several recursive frames, got {}",
        recurse_frames.len()
    );

    let first = recurse_frames[0].preview_line.as_ref().expect("preview line present");
    for frame in &recurse_frames[1..] {
        let other = frame.preview_line.as_ref().expect("preview line present");
        assert!(
            Arc::ptr_eq(first, other),
            "frames at the same source line should share a single Arc<str> allocation",
        );
    }
}
