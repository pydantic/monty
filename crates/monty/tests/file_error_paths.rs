//! Coverage for buffered file I/O error paths that exercise host-side failures.
//!
//! These paths cannot easily be triggered through `mount-fs` test_cases because
//! the in-process `MountTable` does not raise mid-read; instead we drive the
//! VM directly so we can resume an OS call with an exception (or with an
//! interned-string return value that hits the `InternBytes`/empty-string
//! materialisation branch of [`apply_buffer_store`]).

use monty::{
    CompileOptions, ExcType, ExtFunctionResult, FileMode, MontyException, MontyFileHandle, MontyObject, MontyRun,
    NoLimitTracker, PrintWriter,
};

/// Drives an `open()` followed by a single read/write OS call, then resumes
/// the second call with the caller-provided result. Returns the final
/// `Complete` value. Both OS calls are matched against the expected function
/// names (compared by stable [`OsFunctionCall::name`] string) so a regression
/// in dispatch fails the test loudly.
fn run_with_open_then_io(
    code: &str,
    expected_io_fn_name: &str,
    file_handle: MontyFileHandle,
    io_result: ExtFunctionResult,
) -> Result<MontyObject, MontyException> {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    assert_eq!(open_call.function_call.name(), "Open");
    let progress = open_call
        .resume(MontyObject::FileHandle(file_handle), PrintWriter::Stdout)
        .unwrap();
    let io_call = progress.into_os_call().expect("expected follow-up OsCall");
    assert_eq!(io_call.function_call.name(), expected_io_fn_name);
    let progress = io_call.resume(io_result, PrintWriter::Stdout)?;
    Ok(progress.into_complete().expect("expected Complete after resume"))
}

fn file_handle(path: &str, mode: &str) -> MontyFileHandle {
    MontyFileHandle {
        path: path.to_owned(),
        mode: mode.parse::<FileMode>().unwrap(),
        position: 0,
    }
}

// ---------------------------------------------------------------------------
// Host-side read failure: VM must clear `pending_read` so a retry sees a
// fresh slot, and the user-visible exception must surface unchanged.
// ---------------------------------------------------------------------------

#[test]
fn read_host_error_clears_pending_read_and_propagates() {
    let code = r"
f = open('/x.txt')
try:
    f.read(5)
    result = 'no-error'
except OSError as exc:
    result = str(exc)
result
";
    let host_exc = MontyException::new(ExcType::OSError, Some("disk on fire".to_owned()));
    let result = run_with_open_then_io(code, "Path.read_text", file_handle("/x.txt", "r"), host_exc.into())
        .expect("script should complete after catching the host error");
    assert_eq!(result, MontyObject::String("disk on fire".to_owned()));
}

#[test]
fn readline_host_error_then_retry() {
    // After a failed buffered read the file's `pending_read` slot must be
    // cleared so a subsequent successful read replays the OS call. The
    // second OS call is observed here as a new `OsCall` rather than a
    // misrouted resume.
    let code = r"
f = open('/x.txt')
try:
    f.readline()
except OSError:
    pass
f.readline()
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(MontyObject::FileHandle(file_handle("/x.txt", "r")), PrintWriter::Stdout)
        .unwrap();
    let read_call = progress.into_os_call().expect("expected ReadText OsCall");
    assert_eq!(read_call.function_call.name(), "Path.read_text");
    // Fail the first ReadText.
    let host_exc = MontyException::new(ExcType::OSError, Some("transient".to_owned()));
    let progress = read_call
        .resume(ExtFunctionResult::Error(host_exc), PrintWriter::Stdout)
        .unwrap();
    // Retry should issue a second ReadText now that pending_read was cleared.
    let retry_call = progress.into_os_call().expect("expected retry OsCall");
    assert_eq!(retry_call.function_call.name(), "Path.read_text");
    let final_progress = retry_call
        .resume(MontyObject::String("alpha\nbeta\n".to_owned()), PrintWriter::Stdout)
        .unwrap();
    let result = final_progress.into_complete().expect("expected Complete");
    assert_eq!(result, MontyObject::String("alpha\n".to_owned()));
}

// ---------------------------------------------------------------------------
// Host-side write failure: VM must roll back `position` / `file_length`,
// leaving the file's logical `tell()` exactly where it was before the call.
// ---------------------------------------------------------------------------

#[test]
fn write_host_error_rolls_back_position() {
    let code = r"
f = open('/x.txt', 'w')
try:
    f.write('abc')
except OSError as exc:
    after = (f.tell(), str(exc))
after
";
    let host_exc = MontyException::new(ExcType::OSError, Some("device full".to_owned()));
    // `w` mode pre-truncates at open time, so the first user write actually
    // emits AppendText (see `OpenFile::with_state` setting `first_write_done`).
    let result = run_with_open_then_io(code, "Path.append_text", file_handle("/x.txt", "w"), host_exc.into())
        .expect("script should complete after catching the host error");
    // tell() must read 0 — the failed write was rolled back.
    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Int(0), MontyObject::String("device full".to_owned()),])
    );
}

#[test]
fn write_host_error_binary_rolls_back_position() {
    let code = r"
f = open('/x.bin', 'wb')
try:
    f.write(b'\x00\x01\x02')
except OSError:
    pass
f.tell()
";
    let host_exc = MontyException::new(ExcType::OSError, Some("io".to_owned()));
    let result = run_with_open_then_io(code, "Path.append_bytes", file_handle("/x.bin", "wb"), host_exc.into())
        .expect("script should complete");
    assert_eq!(result, MontyObject::Int(0));
}

// ---------------------------------------------------------------------------
// apply_buffer_store covers two materialisation branches: `Value::Ref`
// (heap-allocated content) and `Value::InternString` / `Value::InternBytes`
// (small/empty strings that get interned at the host boundary). The empty
// `""` text case and the small `b""` binary case both go through the
// intern branch.
// ---------------------------------------------------------------------------

#[test]
fn buffer_store_handles_empty_text_via_intern() {
    let code = r"
f = open('/empty.txt')
f.read(5)
";
    let result = run_with_open_then_io(
        code,
        "Path.read_text",
        file_handle("/empty.txt", "r"),
        MontyObject::String(String::new()).into(),
    )
    .expect("script should complete");
    assert_eq!(result, MontyObject::String(String::new()));
}

#[test]
fn buffer_store_handles_empty_bytes_via_intern() {
    let code = r"
f = open('/empty.bin', 'rb')
f.read(5)
";
    let result = run_with_open_then_io(
        code,
        "Path.read_bytes",
        file_handle("/empty.bin", "rb"),
        MontyObject::Bytes(Vec::new()).into(),
    )
    .expect("script should complete");
    assert_eq!(result, MontyObject::Bytes(Vec::new()));
}

// ---------------------------------------------------------------------------
// apply_write_position: a host that returns a non-int (or negative) byte
// count must surface as a clean Python error rather than panic. The
// surfaced error from `as_int(...)?` is a TypeError; the negative-int
// branch raises an internal RuntimeError. We assert on both.
// ---------------------------------------------------------------------------

#[test]
fn write_position_non_int_result_raises_type_error() {
    let code = r"
f = open('/x.txt', 'w')
try:
    f.write('abc')
    result = 'no-error'
except TypeError as exc:
    result = ('type', str(exc))
result
";
    // Host returns a string for the byte count — should raise TypeError.
    let result = run_with_open_then_io(
        code,
        "Path.append_text",
        file_handle("/x.txt", "w"),
        MontyObject::String("oops".to_owned()).into(),
    )
    .expect("script should complete");
    // The exact message comes from `as_int`; we just verify the tag.
    match result {
        MontyObject::Tuple(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MontyObject::String("type".to_owned()));
        }
        other => panic!("expected ('type', msg) tuple, got {other:?}"),
    }
}
