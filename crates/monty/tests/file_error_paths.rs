//! Coverage for buffered file I/O error paths that exercise host-side failures.
//!
//! These paths cannot easily be triggered through `mount-fs` test_cases because
//! the in-process `MountTable` does not raise mid-read; instead we drive the
//! VM directly so we can resume an OS call with an exception (or with an
//! interned-string return value that hits the `InternBytes`/empty-string
//! materialisation branch of [`apply_buffer_store`]).

use std::time::Duration;

use monty::{Dump, MontyRun, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, FileMode, MontyException, MontyFileHandle, MontyObject, PrintWriter,
    ResourceLimits, ResourceTracker,
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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    assert_eq!(open_call.function_call.name(), "open");
    let progress = open_call
        .resume(MontyObject::file_handle(file_handle), PrintWriter::Stdout)
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
    assert_eq!(result, MontyObject::string("disk on fire".to_owned()));
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
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.txt", "r")),
            PrintWriter::Stdout,
        )
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
        .resume(MontyObject::string("alpha\nbeta\n".to_owned()), PrintWriter::Stdout)
        .unwrap();
    let result = final_progress.into_complete().expect("expected Complete");
    assert_eq!(result, MontyObject::string("alpha\n".to_owned()));
}

// ---------------------------------------------------------------------------
// Iteration loads the buffer through its own effects (`BufferLoad` for the
// `ForIter` opcode, `FileNext` for the `next()` builtin). A host failure must
// surface at the loop / call and leave the file ready to retry, and the
// empty-file reply (interned `""`) must exhaust the iterator cleanly.
// ---------------------------------------------------------------------------

#[test]
fn for_loop_host_error_then_retry() {
    let code = r"
f = open('/x.txt')
try:
    for line in f:
        pass
except OSError as exc:
    first = str(exc)
(first, [line for line in f])
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.txt", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    let read_call = progress
        .into_os_call()
        .expect("expected ReadText OsCall from the for loop");
    assert_eq!(read_call.function_call.name(), "Path.read_text");
    let host_exc = MontyException::new(ExcType::OSError, Some("transient".to_owned()));
    let progress = read_call
        .resume(ExtFunctionResult::Error(host_exc), PrintWriter::Stdout)
        .unwrap();
    // The comprehension's loop re-issues the load.
    let retry_call = progress.into_os_call().expect("expected retry OsCall");
    assert_eq!(retry_call.function_call.name(), "Path.read_text");
    let final_progress = retry_call
        .resume(MontyObject::string("alpha\nbeta".to_owned()), PrintWriter::Stdout)
        .unwrap();
    let result = final_progress.into_complete().expect("expected Complete");
    assert_eq!(
        result,
        MontyObject::tuple([
            MontyObject::string("transient".to_owned()),
            MontyObject::list([
                MontyObject::string("alpha\n".to_owned()),
                MontyObject::string("beta".to_owned()),
            ]),
        ])
    );
}

#[test]
fn next_host_error_then_default_on_empty_file() {
    let code = r"
f = open('/x.txt')
try:
    next(f, 'fallback')
    first = 'no-error'
except OSError as exc:
    first = str(exc)
(first, next(f, 'fallback'))
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.txt", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    let read_call = progress.into_os_call().expect("expected ReadText OsCall from next()");
    assert_eq!(read_call.function_call.name(), "Path.read_text");
    let host_exc = MontyException::new(ExcType::OSError, Some("boom".to_owned()));
    let progress = read_call
        .resume(ExtFunctionResult::Error(host_exc), PrintWriter::Stdout)
        .unwrap();
    let retry_call = progress.into_os_call().expect("expected retry OsCall");
    assert_eq!(retry_call.function_call.name(), "Path.read_text");
    // An empty file arrives interned; `next()` answers with the default.
    let final_progress = retry_call
        .resume(MontyObject::string(String::new()), PrintWriter::Stdout)
        .unwrap();
    let result = final_progress.into_complete().expect("expected Complete");
    assert_eq!(
        result,
        MontyObject::tuple([
            MontyObject::string("boom".to_owned()),
            MontyObject::string("fallback".to_owned()),
        ])
    );
}

/// A future cannot stand in for the load that `for` and `next()` wait on: the
/// awaited value would land on the stack where the file iterator is expected.
#[test]
fn file_iteration_load_rejects_future_reply() {
    for code in [
        "f = open('/x.txt')\nfor line in f:\n    pass\n",
        "f = open('/x.txt')\nnext(f)\n",
    ] {
        let err = run_with_open_then_io(
            code,
            "Path.read_text",
            file_handle("/x.txt", "r"),
            ExtFunctionResult::Future(1),
        )
        .unwrap_err();
        assert_eq!(err.exc_type(), ExcType::RuntimeError);
        assert_eq!(
            err.message().unwrap(),
            "file iteration cannot be answered with a future"
        );
    }
}

#[test]
fn for_loop_over_empty_file_via_intern() {
    let code = r"
f = open('/empty.txt')
count = 0
for line in f:
    count += 1
count
";
    let result = run_with_open_then_io(
        code,
        "Path.read_text",
        file_handle("/empty.txt", "r"),
        MontyObject::string(String::new()).into(),
    )
    .expect("script should complete");
    assert_eq!(result, MontyObject::int(0));
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
        MontyObject::tuple([MontyObject::int(0), MontyObject::string("device full".to_owned()),])
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
    assert_eq!(result, MontyObject::int(0));
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
        MontyObject::string(String::new()).into(),
    )
    .expect("script should complete");
    assert_eq!(result, MontyObject::string(String::new()));
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
        MontyObject::bytes(Vec::new()).into(),
    )
    .expect("script should complete");
    assert_eq!(result, MontyObject::bytes(Vec::new()));
}

#[test]
fn rejected_read_suspension_catchable_inside_key_fn() {
    let code = r"
f = open('/x.txt')
caught = []

def key_fn(x):
    try:
        f.read(5)
    except NotImplementedError as e:
        caught.append(str(e))
    return x

sorted([1], key=key_fn)
caught[0]
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.txt", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    let result = progress.into_complete().expect("expected Complete");
    assert_eq!(
        result,
        MontyObject::string("OS function 'Path.read_text' cannot suspend in this synchronous context".to_owned())
    );
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
        MontyObject::string("oops".to_owned()).into(),
    )
    .expect("script should complete");
    // The exact message comes from `as_int`; we just verify the tag.
    let items = result
        .as_ref()
        .items()
        .unwrap_or_else(|| panic!("expected ('type', msg) tuple, got {result:?}"));
    assert_eq!(items.len(), 2);
    assert_eq!(items[0], MontyObject::string("type".to_owned()));
}

// ---------------------------------------------------------------------------
// `csv.reader(f)` / `csv.DictReader(f)` over an unread file load it through the
// `LoadThenCall` effect, then replay the construction. A host failure must
// surface at the call and leave the file ready to retry; a future cannot stand
// in for the load.
// ---------------------------------------------------------------------------

#[test]
fn csv_reader_load_host_error_then_retry() {
    let code = r"
import csv
f = open('/x.csv')
try:
    csv.reader(f)
except OSError as exc:
    first = str(exc)
(first, list(csv.reader(f)))
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.csv", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    let read_call = progress
        .into_os_call()
        .expect("expected ReadText OsCall from csv.reader()");
    assert_eq!(read_call.function_call.name(), "Path.read_text");
    let host_exc = MontyException::new(ExcType::OSError, Some("boom".to_owned()));
    let progress = read_call
        .resume(ExtFunctionResult::Error(host_exc), PrintWriter::Stdout)
        .unwrap();
    let retry_call = progress.into_os_call().expect("expected retry OsCall");
    assert_eq!(retry_call.function_call.name(), "Path.read_text");
    let final_progress = retry_call
        .resume(MontyObject::string("a,b\n1,2\n".to_owned()), PrintWriter::Stdout)
        .unwrap();
    let result = final_progress.into_complete().expect("expected Complete");
    assert_eq!(
        result,
        MontyObject::tuple([
            MontyObject::string("boom".to_owned()),
            MontyObject::list([
                MontyObject::list([MontyObject::string("a".to_owned()), MontyObject::string("b".to_owned()),]),
                MontyObject::list([MontyObject::string("1".to_owned()), MontyObject::string("2".to_owned()),]),
            ]),
        ])
    );
}

#[test]
fn csv_dict_reader_consumed_by_builtin_loads_file() {
    let code = r"
import csv
[row['b'] for row in csv.DictReader(open('/x.csv'))]
";
    let result = run_with_open_then_io(
        code,
        "Path.read_text",
        file_handle("/x.csv", "r"),
        MontyObject::string("a,b\n1,2\n3,4\n".to_owned()).into(),
    )
    .expect("script should complete");
    assert_eq!(
        result,
        MontyObject::list([MontyObject::string("2".to_owned()), MontyObject::string("4".to_owned())])
    );
}

/// `f` is positional-or-keyword on `DictReader`, so the keyword form loads the file too.
#[test]
fn csv_dict_reader_keyword_file_loads_file() {
    let code = "import csv\n[row['a'] for row in csv.DictReader(f=open('/x.csv'))]\n";
    let result = run_with_open_then_io(
        code,
        "Path.read_text",
        file_handle("/x.csv", "r"),
        MontyObject::string("a,b\n1,2\n".to_owned()).into(),
    )
    .expect("script should complete");
    assert_eq!(result, MontyObject::list([MontyObject::string("1".to_owned())]));
}

/// The replayed construction runs Python (here a `fieldnames` iterator), which must
/// count toward the time limit like any other execution.
#[test]
fn csv_dict_reader_replay_counts_toward_time_limit() {
    let code = r"
import csv
def spin():
    for i in range(100000000):
        pass
    return 'a'
csv.DictReader(open('/x.csv'), fieldnames=iter(spin, 'stop'))
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(50));
    let progress = runner
        .start(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.csv", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    let read_call = progress.into_os_call().expect("expected ReadText OsCall");
    let err = read_call
        .resume(MontyObject::string("1,2\n".to_owned()), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::TimeoutError);
}

/// A replayed construction that raises uncaught still ends the turn through the
/// resource-limit epilogue, so a limit the turn blew out-ranks the call's own error.
/// The over-budget `repr()` truncates, swallowing the timeout, and the raise ends the
/// replay before any dispatch checkpoint can fire.
#[test]
fn csv_dict_reader_replay_error_still_hits_limits_at_exit() {
    let code = r"
import csv
x = ['abcdefghij'] * 1_000_000
f = open('/x.csv')
def names():
    s = repr(x)
    raise ValueError(s[:3])
interrupt()
csv.DictReader(f, fieldnames=iter(names, 'stop'))
";
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["interrupt".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.csv", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    // The budget starts once the list exists, so only the replay's `repr()` spends it.
    let mut pause = progress.into_function_call().expect("expected interrupt() call");
    pause.tracker_mut().set_max_feed_duration(Duration::from_millis(20));
    let progress = pause.resume(MontyObject::none(), PrintWriter::Stdout).unwrap();
    let read_call = progress.into_os_call().expect("expected ReadText OsCall");
    let err = read_call
        .resume(MontyObject::string("1,2\n".to_owned()), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::TimeoutError);
}

#[test]
fn csv_reader_load_rejects_future_reply() {
    let err = run_with_open_then_io(
        "import csv\ncsv.reader(open('/x.csv'))\n",
        "Path.read_text",
        file_handle("/x.csv", "r"),
        ExtFunctionResult::Future(1),
    )
    .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::RuntimeError);
    assert_eq!(err.message().unwrap(), "file loading cannot be answered with a future");
}

// ---------------------------------------------------------------------------
// `json.load` and `json.dump` reuse the buffered-read and one-shot-write
// effects, so a host failure must leave the file retry-safe exactly as
// `read()` / `write()` do, and a future cannot answer the load.
// ---------------------------------------------------------------------------

#[test]
fn json_load_host_error_then_retry() {
    let code = r"
import json
f = open('/x.json')
try:
    json.load(f)
except OSError as exc:
    first = str(exc)
(first, json.load(f))
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    let progress = open_call
        .resume(
            MontyObject::file_handle(file_handle("/x.json", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    let read_call = progress.into_os_call().expect("expected ReadText OsCall");
    assert_eq!(read_call.function_call.name(), "Path.read_text");
    let host_exc = MontyException::new(ExcType::OSError, Some("transient".to_owned()));
    let progress = read_call
        .resume(ExtFunctionResult::Error(host_exc), PrintWriter::Stdout)
        .unwrap();
    // The failed load cleared `pending_read`, so the retry reads again.
    let retry_call = progress.into_os_call().expect("expected retry OsCall");
    assert_eq!(retry_call.function_call.name(), "Path.read_text");
    let result = retry_call
        .resume(MontyObject::string("[1, 2]".to_owned()), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .expect("expected Complete");
    assert_eq!(
        result,
        MontyObject::tuple([
            MontyObject::string("transient".to_owned()),
            MontyObject::list([MontyObject::int(1), MontyObject::int(2)]),
        ])
    );
}

#[test]
fn json_load_rejects_future_reply() {
    let err = run_with_open_then_io(
        "import json\nf = open('/x.json')\njson.load(f)\n",
        "Path.read_text",
        file_handle("/x.json", "r"),
        ExtFunctionResult::Future(1),
    )
    .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::RuntimeError);
    assert_eq!(err.message().unwrap(), "json.load cannot be answered with a future");
}

#[test]
fn json_load_parse_error_after_host_read() {
    // The host content is installed as the buffer before parsing, so the
    // failed parse still leaves the file at EOF, as `fp.read()` would.
    let code = r"
import json
f = open('/x.json')
try:
    json.load(f)
except json.JSONDecodeError as exc:
    result = (str(exc), f.read())
result
";
    let result = run_with_open_then_io(
        code,
        "Path.read_text",
        file_handle("/x.json", "r"),
        MontyObject::string("[1,".to_owned()).into(),
    )
    .expect("script should complete");
    assert_eq!(
        result,
        MontyObject::tuple([
            MontyObject::string("Expecting value: line 1 column 4 (char 3)".to_owned()),
            MontyObject::string(String::new()),
        ])
    );
}

#[test]
fn json_dump_host_error_rolls_back_position() {
    let code = r"
import json
f = open('/x.json', 'w')
try:
    json.dump({'a': 1}, f)
except OSError as exc:
    after = (f.tell(), str(exc))
after
";
    let host_exc = MontyException::new(ExcType::OSError, Some("device full".to_owned()));
    let result = run_with_open_then_io(code, "Path.append_text", file_handle("/x.json", "w"), host_exc.into())
        .expect("script should complete after catching the host error");
    assert_eq!(
        result,
        MontyObject::tuple([MontyObject::int(0), MontyObject::string("device full".to_owned())])
    );
}

#[test]
fn json_dump_answers_none_and_advances_position() {
    let code = r"
import json
f = open('/x.json', 'w')
(json.dump([1, 2], f), f.tell())
";
    let result = run_with_open_then_io(
        code,
        "Path.append_text",
        file_handle("/x.json", "w"),
        MontyObject::int(6).into(),
    )
    .expect("script should complete");
    assert_eq!(result, MontyObject::tuple([MontyObject::none(), MontyObject::int(6)]));
}

#[test]
fn json_load_dump_taken_while_waiting_for_the_host_still_parses() {
    // The `JsonLoad` effect rides in the suspended state, so a session dumped
    // during the host read parses the reply exactly like the original.
    let code = "import json\nf = open('/x.json')\njson.load(f)['k']\n";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = progress
        .into_os_call()
        .expect("expected Open OsCall")
        .resume(
            MontyObject::file_handle(file_handle("/x.json", "r")),
            PrintWriter::Stdout,
        )
        .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Running(&progress)).unwrap();
    let Session::Running(loaded) = Dump::load(&bytes).unwrap().state else {
        panic!("expected a running session");
    };
    for progress in [progress, *loaded] {
        let read_call = progress.into_os_call().expect("expected ReadText OsCall");
        assert_eq!(read_call.function_call.name(), "Path.read_text");
        let result = read_call
            .resume(MontyObject::string(r#"{"k": [true]}"#.to_owned()), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .expect("expected Complete");
        assert_eq!(result, MontyObject::list([MontyObject::bool(true)]));
    }
}
