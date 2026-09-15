//! Tests for OS function calls.
//!
//! Verifies that Path filesystem methods and os module functions yield
//! `RunProgress::OsCall` with the correct `OsFunction` variant and arguments,
//! and that return values are correctly used by Python code.

use monty::{MontyRepl, MontyRun, ReplProgress, RunProgress};
use monty_types::{
    CallArgs, CompileOptions, ExcType, ExtFunctionResult, FileMode, MontyDate, MontyDateTime, MontyException,
    MontyFileHandle, MontyObject, OsFunctionCall, PrintWriter, ResourceTracker, dir_stat, file_stat,
};

/// Helper to run code and extract the OsCall progress.
///
/// Runs the provided Python code and asserts that it yields an `OsCall`.
/// Returns the OS function name (stable `OsFunctionCall::name` string) and
/// positional args projected via `to_args`. State is resumed with a mock
/// result to properly clean up ref counts.
fn run_to_oscall(code: &str) -> (&'static str, Vec<MontyObject>) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    match progress {
        RunProgress::OsCall(call) => {
            let mock_result = mock_oscall_result(&call.function_call);
            let function = call.function_call.name();
            let args = positional(&call.function_call.clone().to_args());
            let _ = call.resume(mock_result, PrintWriter::Stdout);
            (function, args)
        }
        _ => panic!("expected OsCall, got {progress:?}"),
    }
}

/// Returns a `MontyObject` shaped like a plausible host response for `call`.
fn mock_oscall_result(call: &OsFunctionCall) -> MontyObject {
    match call {
        OsFunctionCall::Exists(_)
        | OsFunctionCall::IsFile(_)
        | OsFunctionCall::IsDir(_)
        | OsFunctionCall::IsSymlink(_) => MontyObject::bool(true),
        OsFunctionCall::ReadText(_) | OsFunctionCall::Resolve(_) | OsFunctionCall::Absolute(_) => {
            MontyObject::string("mock".to_owned())
        }
        OsFunctionCall::ReadBytes(_) => MontyObject::bytes(vec![]),
        OsFunctionCall::Stat(_) => MontyObject::none(),
        OsFunctionCall::Iterdir(_) => MontyObject::list([]),
        OsFunctionCall::WriteText(_)
        | OsFunctionCall::WriteBytes(_)
        | OsFunctionCall::AppendText(_)
        | OsFunctionCall::AppendBytes(_)
        | OsFunctionCall::Mkdir(_)
        | OsFunctionCall::Unlink(_)
        | OsFunctionCall::Rmdir(_)
        | OsFunctionCall::Rename(_) => MontyObject::none(),
        OsFunctionCall::Open(_) => MontyObject::file_handle(MontyFileHandle {
            path: "mock".to_owned(),
            mode: "r".parse::<FileMode>().unwrap(),
            position: 0,
        }),
        OsFunctionCall::Getenv(_) => MontyObject::string("mock_env_value".to_owned()),
        OsFunctionCall::GetEnviron => MontyObject::dict([]),
        OsFunctionCall::DateToday => MontyObject::date(MontyDate {
            year: 2023,
            month: 11,
            day: 14,
        }),
        OsFunctionCall::Time => MontyObject::float(1_700_000_000.0),
        OsFunctionCall::Sleep(_) | OsFunctionCall::AsyncSleep(_) => MontyObject::none(),
        OsFunctionCall::DateTimeNow(_) => MontyObject::datetime(MontyDateTime {
            year: 2023,
            month: 11,
            day: 14,
            hour: 22,
            minute: 13,
            second: 20,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
        }),
        OsFunctionCall::Urandom(args) => MontyObject::bytes(vec![0; usize::try_from(args.size).unwrap()]),
    }
}

/// Helper to run code, provide an OS call result, and get the final value.
fn run_oscall_with_result(code: &str, mock_result: MontyObject) -> (&'static str, Vec<MontyObject>, MontyObject) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    match progress {
        RunProgress::OsCall(call) => {
            let function = call.function_call.name();
            let args = positional(&call.function_call.clone().to_args());
            let resumed = call.resume(mock_result, PrintWriter::Stdout).unwrap();
            let final_result = resumed.into_complete().expect("expected Complete after resume");
            (function, args, final_result)
        }
        _ => panic!("expected OsCall, got {progress:?}"),
    }
}

// =============================================================================
// Verify each OsFunction variant yields correctly
// =============================================================================

#[test]
fn path_exists() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/test.txt').exists()");
    assert_eq!(func, "Path.exists");
    assert_eq!(args, vec![MontyObject::path("/tmp/test.txt".to_owned())]);
}

#[test]
fn path_is_file() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/test.txt').is_file()");
    assert_eq!(func, "Path.is_file");
    assert_eq!(args, vec![MontyObject::path("/tmp/test.txt".to_owned())]);
}

#[test]
fn path_is_dir() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp').is_dir()");
    assert_eq!(func, "Path.is_dir");
    assert_eq!(args, vec![MontyObject::path("/tmp".to_owned())]);
}

#[test]
fn path_is_symlink() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/link').is_symlink()");
    assert_eq!(func, "Path.is_symlink");
    assert_eq!(args, vec![MontyObject::path("/tmp/link".to_owned())]);
}

#[test]
fn path_read_text() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.txt').read_text()");
    assert_eq!(func, "Path.read_text");
    assert_eq!(args, vec![MontyObject::path("/tmp/file.txt".to_owned())]);
}

#[test]
fn path_read_bytes() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.bin').read_bytes()");
    assert_eq!(func, "Path.read_bytes");
    assert_eq!(args, vec![MontyObject::path("/tmp/file.bin".to_owned())]);
}

#[test]
fn path_stat() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.txt').stat()");
    assert_eq!(func, "Path.stat");
    assert_eq!(args, vec![MontyObject::path("/tmp/file.txt".to_owned())]);
}

#[test]
fn path_iterdir() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp').iterdir()");
    assert_eq!(func, "Path.iterdir");
    assert_eq!(args, vec![MontyObject::path("/tmp".to_owned())]);
}

#[test]
fn path_resolve() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('./relative').resolve()");
    assert_eq!(func, "Path.resolve");
    assert_eq!(args, vec![MontyObject::path("/relative".to_owned())]);
}

#[test]
fn path_absolute() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('./relative').absolute()");
    assert_eq!(func, "Path.absolute");
    assert_eq!(args, vec![MontyObject::path("/relative".to_owned())]);
}

// =============================================================================
// Working directory: relative paths resolve against it before reaching the host
// =============================================================================

/// Starts `code` with the working directory set to `cwd` and returns the first
/// typed OS call. The call is answered with a mock result so
/// the run winds down cleanly instead of dropping live stack values.
fn run_to_oscall_in(code: &str, cwd: &str) -> OsFunctionCall {
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.set_cwd(cwd);
    match runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
    {
        RunProgress::OsCall(call) => {
            let mock_result = mock_oscall_result(&call.function_call);
            let function_call = call.function_call.clone();
            let _ = call.resume(mock_result, PrintWriter::Stdout);
            function_call
        }
        progress => panic!("expected OsCall, got {progress:?}"),
    }
}

#[test]
fn relative_paths_resolve_against_cwd() {
    for (code, expected) in [
        ("open('notes.txt')", "/data/notes.txt"),
        (
            "from pathlib import Path; Path('a/./b.txt').read_text()",
            "/data/a/b.txt",
        ),
        ("from pathlib import Path; Path('..').resolve()", "/data/.."),
        ("from pathlib import Path; Path('../../x').resolve()", "/data/../../x"),
        // Absolute paths pass through as written; the host normalizes them.
        ("from pathlib import Path; Path('/a/../b/c').exists()", "/a/../b/c"),
        ("import os; os.mkdir('/data/../x')", "/data/../x"),
        ("import os; os.listdir()", "/data/."),
        ("import os; os.mkdir('sub/')", "/data/sub/"),
        ("import os; os.stat('a//./b/../c/')", "/data/a//./b/../c/"),
        ("open('')", ""),
        ("from pathlib import Path; Path('/abs.txt').exists()", "/abs.txt"),
    ] {
        let call = run_to_oscall_in(code, "/data");
        assert_eq!(call.fs_primary_path(), Some(expected), "{code}");
    }
}

#[test]
fn rename_resolves_both_endpoints() {
    let call = run_to_oscall_in("import os; os.rename('a/../a.txt', 'b/../b.txt')", "/data");
    assert_eq!(call.name(), "Path.rename");
    assert_eq!(call.fs_primary_path(), Some("/data/a/../a.txt"));
    assert_eq!(call.rename_destination(), Some("/data/b/../b.txt"));
}

#[test]
fn getcwd_and_path_cwd_need_no_host() {
    let mut runner = MontyRun::new(
        "import os
from pathlib import Path
(os.getcwd(), Path.cwd(), Path('/unrelated').cwd())"
            .to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    assert_eq!(
        runner.run_no_limits(vec![]).unwrap(),
        MontyObject::tuple([
            MontyObject::string("/".to_owned()),
            MontyObject::path("/".to_owned()),
            MontyObject::path("/".to_owned())
        ])
    );
    runner.set_cwd("/mnt/data");
    assert_eq!(
        runner.run_no_limits(vec![]).unwrap(),
        MontyObject::tuple([
            MontyObject::string("/mnt/data".to_owned()),
            MontyObject::path("/mnt/data".to_owned()),
            MontyObject::path("/mnt/data".to_owned())
        ])
    );
}

/// Runs an `os.chdir` snippet, answering its `Path.stat` call with `reply`.
fn run_chdir(code: &str, reply: impl Into<ExtFunctionResult>) -> Result<MontyObject, MontyException> {
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.set_cwd("/data");
    match runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
    {
        RunProgress::OsCall(call) => {
            assert_eq!(call.function_call.name(), "Path.stat");
            let args = positional(&call.function_call.clone().to_args());
            assert_eq!(args, vec![MontyObject::path("/data/sub".to_owned())]);
            Ok(call
                .resume(reply, PrintWriter::Stdout)?
                .into_complete()
                .expect("expected Complete after resume"))
        }
        progress => panic!("expected OsCall, got {progress:?}"),
    }
}

#[test]
fn os_chdir_adopts_a_directory() {
    let code = "import os\nfrom pathlib import Path\nos.chdir('sub')\n(os.getcwd(), str(Path('x').absolute()))";
    // `absolute()` is a second OS call, answered below.
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.set_cwd("/data");
    let RunProgress::OsCall(call) = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected OsCall");
    };
    let RunProgress::OsCall(absolute) = call.resume(dir_stat(0o755, 0.0), PrintWriter::Stdout).unwrap() else {
        panic!("expected a second OsCall");
    };
    let args = positional(&absolute.function_call.clone().to_args());
    assert_eq!(args, vec![MontyObject::path("/data/sub/x".to_owned())]);
    let result = absolute
        .resume(MontyObject::path("/data/sub/x".to_owned()), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(
        result,
        MontyObject::tuple([
            MontyObject::string("/data/sub".to_owned()),
            MontyObject::string("/data/sub/x".to_owned())
        ])
    );
}

#[test]
fn os_chdir_normalizes_cwd_after_host_acceptance() {
    let mut runner = MontyRun::new(
        "import os\nos.chdir('/data/sub/../sub/')\nos.getcwd()".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    runner.set_cwd("/data");
    let RunProgress::OsCall(call) = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected OsCall");
    };
    assert_eq!(call.function_call.fs_primary_path(), Some("/data/sub/../sub/"));
    let result = call
        .resume(dir_stat(0o755, 0.0), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(result, MontyObject::string("/data/sub".to_owned()));
}

/// Rejected file operations must release their effect's heap reference before raising.
#[test]
fn nul_file_paths_release_pending_effects() {
    for (mode, operation) in [("r", "f.read()"), ("w", "f.write('data')")] {
        let runner = MontyRun::new(
            format!("try:\n    {operation}\nexcept ValueError as e:\n    message = str(e)\nmessage"),
            "test.py",
            vec!["f".to_owned()],
            CompileOptions::default(),
        )
        .unwrap();
        let file = MontyObject::file_handle(MontyFileHandle {
            path: "/bad\0/../x".to_owned(),
            mode: mode.parse().unwrap(),
            position: 0,
        });
        let result = runner
            .start(vec![file], ResourceTracker::default(), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .unwrap();
        assert_eq!(result, MontyObject::string("embedded null byte".to_owned()));
    }
}

#[test]
fn os_chdir_empty_path_raises_without_the_host() {
    let err = MontyRun::new(
        "import os\nos.chdir('')".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![])
    .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::FileNotFoundError);
    assert_eq!(err.message().unwrap(), "[Errno 2] No such file or directory: ''");
}

#[test]
fn os_chdir_rejects_a_malformed_stat_reply() {
    // Directory bits in the first slot are not enough: `st_mode` is found by name.
    let bogus = MontyObject::named_tuple(
        "other".to_owned(),
        vec!["nope".to_owned()],
        vec![MontyObject::int(0o040_755)],
    );
    let err = run_chdir("import os\nos.chdir('sub')", bogus).unwrap_err();
    assert_eq!(err.exc_type(), ExcType::RuntimeError);
    assert_eq!(
        err.message().unwrap(),
        "invalid return type: os.chdir requires the host to return a stat result, got namedtuple"
    );
}

#[test]
fn os_chdir_answered_with_a_future_is_refused() {
    let err = run_chdir("import os\nos.chdir('sub')", ExtFunctionResult::Future(7)).unwrap_err();
    assert_eq!(err.exc_type(), ExcType::RuntimeError);
    assert_eq!(err.message().unwrap(), "os.chdir cannot be answered with a future");
}

#[test]
fn set_cwd_normalizes() {
    let mut runner = MontyRun::new(
        "import os\nos.getcwd()".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    runner.set_cwd("/data/sub/../x//");
    assert_eq!(
        runner.run_no_limits(vec![]).unwrap(),
        MontyObject::string("/data/x".to_owned())
    );
}

#[test]
fn os_chdir_returns_none() {
    assert_eq!(
        run_chdir("import os\nos.chdir('sub')", dir_stat(0o755, 0.0)).unwrap(),
        MontyObject::none()
    );
}

#[test]
fn os_chdir_to_a_file_raises_not_a_directory() {
    let err = run_chdir("import os\nos.chdir('sub')", file_stat(0o644, 3, 0.0)).unwrap_err();
    assert_eq!(err.exc_type(), ExcType::NotADirectoryError);
    // CPython names the argument as spelled, not the resolved path.
    assert_eq!(err.message().unwrap(), "[Errno 20] Not a directory: 'sub'");
}

#[test]
fn os_chdir_host_error_propagates_and_keeps_cwd() {
    let code = "import os\ntry:\n    os.chdir('sub')\nexcept FileNotFoundError as e:\n    result = (str(e), os.getcwd())\nresult";
    let missing = MontyException::new(
        ExcType::FileNotFoundError,
        Some("[Errno 2] No such file or directory: '/data/sub'".to_owned()),
    );
    assert_eq!(
        run_chdir(code, missing).unwrap(),
        MontyObject::tuple([
            MontyObject::string("[Errno 2] No such file or directory: '/data/sub'".to_owned()),
            MontyObject::string("/data".to_owned())
        ])
    );
}

/// The REPL refuses a future answering `os.chdir`'s stat like a one-shot run
/// does, instead of leaving the directory silently unchanged.
#[test]
fn repl_os_chdir_answered_with_a_future_is_refused() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    repl.set_cwd("/data");
    let ReplProgress::OsCall(call) = repl
        .feed_start("import os\nos.chdir('sub')", vec![], PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected OsCall");
    };
    let err = call
        .resume(ExtFunctionResult::Future(7), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(err.error.exc_type(), ExcType::RuntimeError);
    assert_eq!(
        err.error.message().unwrap(),
        "os.chdir cannot be answered with a future"
    );
    let mut repl = err.repl;
    assert_eq!(
        repl.feed_run("os.getcwd()", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::string("/data".to_owned())
    );
}

/// The REPL keeps the directory `os.chdir` left the last snippet in, until
/// the host switches it with `set_cwd`.
#[test]
fn repl_keeps_the_directory_across_snippets() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    repl.set_cwd("/data");
    let ReplProgress::OsCall(call) = repl
        .feed_start("import os\nos.chdir('sub')", vec![], PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected OsCall");
    };
    let ReplProgress::Complete { repl, .. } = call.resume(dir_stat(0o755, 0.0), PrintWriter::Stdout).unwrap() else {
        panic!("expected Complete");
    };
    let mut repl = repl;
    assert_eq!(
        repl.feed_run("os.getcwd()", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::string("/data/sub".to_owned())
    );
    repl.set_cwd("/other");
    assert_eq!(
        repl.feed_run("os.getcwd()", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::string("/other".to_owned())
    );
}

#[test]
fn os_chdir_rejects_non_path() {
    let err = MontyRun::new(
        "import os\nos.chdir(1)".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![])
    .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::TypeError);
    assert_eq!(
        err.message().unwrap(),
        "chdir: path should be string or os.PathLike, not int"
    );
}

// =============================================================================
// Path argument handling (spaces, unicode, concatenation)
// =============================================================================

#[test]
fn path_with_spaces() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/path/with spaces/file.txt').exists()");
    assert_eq!(func, "Path.exists");
    assert_eq!(args[0], MontyObject::path("/path/with spaces/file.txt".to_owned()));
}

#[test]
fn path_with_unicode() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/путь/文件.txt').exists()");
    assert_eq!(func, "Path.exists");
    assert_eq!(args[0], MontyObject::path("/путь/文件.txt".to_owned()));
}

#[test]
fn path_concatenation_yields_correct_path() {
    let (func, args) = run_to_oscall(
        r"
from pathlib import Path
base = Path('/home')
full = base / 'user' / 'file.txt'
full.exists()
",
    );
    assert_eq!(func, "Path.exists");
    assert_eq!(args[0], MontyObject::path("/home/user/file.txt".to_owned()));
}

// =============================================================================
// Round-trip tests: OS call result used by Python code
// =============================================================================

#[test]
fn exists_result_used_in_conditional() {
    let code = r"
from pathlib import Path
'found' if Path('/tmp/test.txt').exists() else 'missing'
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::bool(true));
    assert_eq!(func, "Path.exists");
    assert_eq!(result, MontyObject::string("found".to_owned()));

    // Also test false case
    let (_, _, result) = run_oscall_with_result(code, MontyObject::bool(false));
    assert_eq!(result, MontyObject::string("missing".to_owned()));
}

#[test]
fn read_text_result_concatenated() {
    let code = r"
from pathlib import Path
'Content: ' + Path('/tmp/hello.txt').read_text()
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::string("Hello!".to_owned()));
    assert_eq!(func, "Path.read_text");
    assert_eq!(result, MontyObject::string("Content: Hello!".to_owned()));
}

#[test]
fn read_bytes_result_used() {
    let code = r"
from pathlib import Path
data = Path('/tmp/file.bin').read_bytes()
data[0]
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::bytes(vec![0x42, 0x43, 0x44]));
    assert_eq!(func, "Path.read_bytes");
    assert_eq!(result, MontyObject::int(0x42));
}

#[test]
fn iterdir_result_iterated() {
    let code = r"
from pathlib import Path
entries = Path('/tmp').iterdir()
len(entries)
";
    // Return a list of path strings (simulating directory entries)
    let mock_entries = MontyObject::list([
        MontyObject::string("/tmp/file1.txt".to_owned()),
        MontyObject::string("/tmp/file2.txt".to_owned()),
        MontyObject::string("/tmp/subdir".to_owned()),
    ]);
    let (func, args, result) = run_oscall_with_result(code, mock_entries);

    assert_eq!(func, "Path.iterdir");
    assert_eq!(args[0], MontyObject::path("/tmp".to_owned()));
    assert_eq!(result, MontyObject::int(3));
}

#[test]
fn iterdir_result_indexed() {
    let code = r"
from pathlib import Path
entries = Path('/home/user').iterdir()
entries[0]
";
    let mock_entries = MontyObject::list([
        MontyObject::string("/home/user/documents".to_owned()),
        MontyObject::string("/home/user/downloads".to_owned()),
    ]);
    let (func, args, result) = run_oscall_with_result(code, mock_entries);

    assert_eq!(func, "Path.iterdir");
    assert_eq!(args[0], MontyObject::path("/home/user".to_owned()));
    assert_eq!(result, MontyObject::path("/home/user/documents".to_owned()));
}

/// Display names retain the input spelling while a relative host handle is anchored at open time.
#[test]
fn open_name_and_target_survive_chdir() {
    let mut runner = MontyRun::new(
        "import os\nf = open('./name.txt')\nos.chdir('/other')\n(f.name, f.read())".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    runner.set_cwd("/data");
    let call = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
        .into_os_call()
        .unwrap();
    let call = call
        .resume(mock_file_handle("target.txt"), PrintWriter::Stdout)
        .unwrap()
        .into_os_call()
        .unwrap();
    assert_eq!(call.function_call.fs_primary_path(), Some("/other"));
    let call = call
        .resume(dir_stat(0o755, 0.0), PrintWriter::Stdout)
        .unwrap()
        .into_os_call()
        .unwrap();
    assert_eq!(call.function_call.fs_primary_path(), Some("/data/target.txt"));
    let result = call
        .resume(MontyObject::string("content".to_owned()), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(
        result,
        MontyObject::tuple([
            MontyObject::string("./name.txt".to_owned()),
            MontyObject::string("content".to_owned())
        ])
    );
}

/// Malformed host replies must not leak values or bypass result postprocessing.
#[test]
fn filesystem_result_effects_reject_invalid_replies() {
    for (code, operation) in [
        ("open('./file.txt')", "open"),
        ("import os\nos.listdir('.')", "os.listdir"),
        ("from pathlib import Path\nPath('.').iterdir()", "Path.iterdir"),
    ] {
        let call = run_to_oscall_start(code);
        let err = call
            .resume(MontyObject::list([MontyObject::int(1)]), PrintWriter::Stdout)
            .unwrap_err();
        assert_eq!(err.exc_type(), ExcType::RuntimeError);
        assert!(err.message().unwrap().contains(operation));
        let call = run_to_oscall_start(code);
        let err = call
            .resume(ExtFunctionResult::Future(1), PrintWriter::Stdout)
            .unwrap_err();
        assert_eq!(
            err.message().unwrap(),
            format!("{operation} cannot be answered with a future")
        );
    }
}

#[test]
fn stat_result_st_size() {
    let code = r"
from pathlib import Path
info = Path('/tmp/file.txt').stat()
info.st_size
";
    let (func, args, result) = run_oscall_with_result(code, file_stat(0o644, 1024, 0.0));

    assert_eq!(func, "Path.stat");
    assert_eq!(args[0], MontyObject::path("/tmp/file.txt".to_owned()));
    assert_eq!(result, MontyObject::int(1024));
}

#[test]
fn stat_result_st_mode() {
    let code = r"
from pathlib import Path
info = Path('/tmp/file.txt').stat()
info.st_mode
";
    // 0o755 = rwxr-xr-x (file_stat adds 0o100_000 for regular file type)
    let (func, args, result) = run_oscall_with_result(code, file_stat(0o755, 0, 0.0));

    assert_eq!(func, "Path.stat");
    assert_eq!(args[0], MontyObject::path("/tmp/file.txt".to_owned()));
    assert_eq!(result, MontyObject::int(0o100_755));
}

#[test]
fn stat_result_multiple_fields() {
    let code = r"
from pathlib import Path
info = Path('/var/log/syslog').stat()
(info.st_size, info.st_mode)
";
    // 0o644 = rw-r--r-- (file_stat adds 0o100_000 for regular file type)
    let (func, args, result) = run_oscall_with_result(code, file_stat(0o644, 4096, 0.0));

    assert_eq!(func, "Path.stat");
    assert_eq!(args[0], MontyObject::path("/var/log/syslog".to_owned()));
    assert_eq!(
        result,
        MontyObject::tuple([MontyObject::int(4096), MontyObject::int(0o100_644)])
    );
}

#[test]
fn stat_result_index_access() {
    // stat_result also supports index access like a tuple
    let code = r"
from pathlib import Path
info = Path('/tmp/file.txt').stat()
info[6]  # st_size is at index 6
";
    let (func, args, result) = run_oscall_with_result(code, file_stat(0o644, 2048, 0.0));

    assert_eq!(func, "Path.stat");
    assert_eq!(args[0], MontyObject::path("/tmp/file.txt".to_owned()));
    assert_eq!(result, MontyObject::int(2048));
}

// =============================================================================
// os.getenv tests
// =============================================================================

#[test]
fn os_getenv_yields_oscall() {
    let code = r"
import os
os.getenv('PATH')
";
    let (func, args) = run_to_oscall(code);
    assert_eq!(func, "os.getenv");
    // First arg is key, second is default (None if not provided)
    assert_eq!(args[0], MontyObject::string("PATH".to_owned()));
    assert_eq!(args[1], MontyObject::none());
}

#[test]
fn os_getenv_with_default() {
    let code = r"
import os
os.getenv('MISSING', 'fallback')
";
    let (func, args) = run_to_oscall(code);
    assert_eq!(func, "os.getenv");
    assert_eq!(args[0], MontyObject::string("MISSING".to_owned()));
    assert_eq!(args[1], MontyObject::string("fallback".to_owned()));
}

#[test]
fn os_getenv_result_used() {
    let code = r"
import os
'HOME=' + os.getenv('HOME')
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::string("/home/user".to_owned()));
    assert_eq!(func, "os.getenv");
    assert_eq!(result, MontyObject::string("HOME=/home/user".to_owned()));
}

// =============================================================================
// os.environ tests
// =============================================================================

#[test]
fn os_environ_yields_oscall() {
    let code = r"
import os
os.environ
";
    let (func, args) = run_to_oscall(code);
    assert_eq!(func, "os.environ");
    // GetEnviron takes no arguments
    assert!(args.is_empty(), "expected empty args, got {args:?}");
}

#[test]
fn os_environ_result_is_dict() {
    let code = r"
import os
type(os.environ).__name__
";
    let mock_env = MontyObject::dict(vec![
        (
            MontyObject::string("HOME".to_owned()),
            MontyObject::string("/home/user".to_owned()),
        ),
        (
            MontyObject::string("PATH".to_owned()),
            MontyObject::string("/usr/bin".to_owned()),
        ),
    ]);
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::string("dict".to_owned()));
}

#[test]
fn os_environ_key_access() {
    let code = r"
import os
os.environ['HOME']
";
    let mock_env = MontyObject::dict(vec![(
        MontyObject::string("HOME".to_owned()),
        MontyObject::string("/home/user".to_owned()),
    )]);
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::string("/home/user".to_owned()));
}

#[test]
fn os_environ_get_method() {
    let code = r"
import os
os.environ.get('MISSING', 'default')
";
    let mock_env = MontyObject::dict([]);
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::string("default".to_owned()));
}

#[test]
fn os_environ_len() {
    let code = r"
import os
len(os.environ)
";
    let mock_env = MontyObject::dict(vec![
        (MontyObject::string("A".to_owned()), MontyObject::string("1".to_owned())),
        (MontyObject::string("B".to_owned()), MontyObject::string("2".to_owned())),
        (MontyObject::string("C".to_owned()), MontyObject::string("3".to_owned())),
    ]);
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::int(3));
}

#[test]
fn os_environ_in_check() {
    let code = r"
import os
'HOME' in os.environ
";
    let mock_env = MontyObject::dict(vec![(
        MontyObject::string("HOME".to_owned()),
        MontyObject::string("/home/user".to_owned()),
    )]);
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::bool(true));
}

// =============================================================================
// os module filesystem wrappers
// =============================================================================

/// Runs code expected to raise before any OS call and returns the final
/// `"ExcType: message"` line of the resulting exception (dropping the traceback).
fn run_to_error(code: &str) -> String {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    match runner.start(vec![], ResourceTracker::default(), PrintWriter::Stdout) {
        Err(exc) => exc.to_string().lines().last().unwrap_or_default().to_owned(),
        Ok(progress) => panic!("expected error, got {progress:?}"),
    }
}

#[test]
fn os_listdir_yields_iterdir_and_strips_names() {
    let (func, args, result) = run_oscall_with_result(
        "import os\nos.listdir('/mnt/data')",
        MontyObject::list([
            MontyObject::path("/mnt/data/b.txt".to_owned()),
            MontyObject::path("/mnt/data/a.txt".to_owned()),
            MontyObject::path("/mnt/data/sub".to_owned()),
        ]),
    );
    // os.listdir reuses the Path.iterdir OS call — hosts see that name.
    assert_eq!(func, "Path.iterdir");
    assert_eq!(args, vec![MontyObject::path("/mnt/data".to_owned())]);
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::string("b.txt".to_owned()),
            MontyObject::string("a.txt".to_owned()),
            MontyObject::string("sub".to_owned()),
        ])
    );
}

#[test]
fn os_listdir_default_path_is_cwd() {
    let (func, args, result) = run_oscall_with_result("import os\nos.listdir()", MontyObject::list([]));
    assert_eq!(func, "Path.iterdir");
    assert_eq!(args, vec![MontyObject::path("/".to_owned())]);
    assert_eq!(result, MontyObject::list([]));
}

#[test]
fn os_listdir_accepts_host_strings() {
    // Hosts serving the `Path.iterdir` callback directly may return plain
    // strings instead of paths — names are stripped the same way.
    let (_, _, result) = run_oscall_with_result(
        "import os\nos.listdir('/mnt')",
        MontyObject::list([
            MontyObject::string("/mnt/x.txt".to_owned()),
            MontyObject::string("plain".to_owned()),
        ]),
    );
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::string("x.txt".to_owned()),
            MontyObject::string("plain".to_owned()),
        ])
    );
}

/// Runs code up to its first suspension and returns the raw `OsCall` progress,
/// for tests that inspect the typed `function_call` or resume by hand.
fn run_to_oscall_start(code: &str) -> monty::OsCall {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    match progress {
        RunProgress::OsCall(call) => call,
        _ => panic!("expected OsCall, got {progress:?}"),
    }
}

#[test]
fn os_listdir_rejects_bad_host_result() {
    let call = run_to_oscall_start("import os\nos.listdir('/mnt')");
    let err = call
        .resume(MontyObject::list([MontyObject::int(3)]), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(
        err.to_string().lines().last().unwrap_or_default(),
        "RuntimeError: invalid return type: os.listdir requires the host to return a list of paths, got int"
    );
}

#[test]
fn os_stat_yields_stat_call() {
    let (func, args) = run_to_oscall("import os\nos.stat('/tmp/file.txt')");
    assert_eq!(func, "Path.stat");
    assert_eq!(args, vec![MontyObject::path("/tmp/file.txt".to_owned())]);
}

#[test]
fn os_mkdir_and_makedirs_yield_mkdir_calls() {
    // (code, path, parents, exist_ok)
    let cases = [
        ("import os\nos.mkdir('/mnt/d', 0o700)", "/mnt/d", false, false),
        (
            "import os\nos.makedirs('/mnt/a/b', exist_ok=True)",
            "/mnt/a/b",
            true,
            true,
        ),
    ];
    for (code, path, parents, exist_ok) in cases {
        let call = run_to_oscall_start(code);
        let OsFunctionCall::Mkdir(mkdir) = &call.function_call else {
            panic!("expected Mkdir, got {:?}", call.function_call);
        };
        assert_eq!(mkdir.path.as_str(), path);
        assert_eq!(mkdir.parents, parents);
        assert_eq!(mkdir.exist_ok, exist_ok);
        let _ = call.resume(MontyObject::none(), PrintWriter::Stdout);
    }
}

#[test]
fn os_remove_and_unlink_yield_unlink_call() {
    let (func, args) = run_to_oscall("import os\nos.remove('/mnt/f.txt')");
    assert_eq!(func, "Path.unlink");
    assert_eq!(args, vec![MontyObject::path("/mnt/f.txt".to_owned())]);

    let (func, args) = run_to_oscall("import os\nos.unlink('/mnt/g.txt')");
    assert_eq!(func, "Path.unlink");
    assert_eq!(args, vec![MontyObject::path("/mnt/g.txt".to_owned())]);
}

#[test]
fn os_rmdir_yields_rmdir_call() {
    let (func, args) = run_to_oscall("import os\nos.rmdir('/mnt/d')");
    assert_eq!(func, "Path.rmdir");
    assert_eq!(args, vec![MontyObject::path("/mnt/d".to_owned())]);
}

#[test]
fn os_rename_and_replace_yield_rename_call() {
    let (func, args) = run_to_oscall("import os\nos.rename('/mnt/a', '/mnt/b')");
    assert_eq!(func, "Path.rename");
    assert_eq!(
        args,
        vec![
            MontyObject::path("/mnt/a".to_owned()),
            MontyObject::path("/mnt/b".to_owned())
        ]
    );

    let (func, args) = run_to_oscall("import os\nos.replace('/mnt/a', '/mnt/b')");
    assert_eq!(func, "Path.rename");
    assert_eq!(
        args,
        vec![
            MontyObject::path("/mnt/a".to_owned()),
            MontyObject::path("/mnt/b".to_owned())
        ]
    );
}

/// dir_fd / follow_symlinks are parsed for signature parity but never
/// supported — Linux CPython accepts them, so these stay out of the dual-run
/// test_cases and are pinned here instead (see limitations/os.md).
#[test]
fn os_unsupported_kwargs() {
    let cases = [
        (
            "import os\nos.stat('/x', dir_fd=3)",
            "NotImplementedError: dir_fd unavailable on this platform",
        ),
        (
            "import os\nos.stat('/x', dir_fd='s')",
            "TypeError: argument should be integer or None, not str",
        ),
        (
            "import os\nos.stat('/x', follow_symlinks=False)",
            "NotImplementedError: stat: follow_symlinks unavailable on this platform",
        ),
        (
            "import os\nos.rename('/a', '/b', src_dir_fd=1)",
            "NotImplementedError: rename: src_dir_fd and dst_dir_fd unavailable on this platform",
        ),
        (
            "import os\nos.replace('/a', '/b', dst_dir_fd=1)",
            "NotImplementedError: replace: src_dir_fd and dst_dir_fd unavailable on this platform",
        ),
        (
            "import os\nos.mkdir('/d', 0o777, dir_fd=7)",
            "NotImplementedError: dir_fd unavailable on this platform",
        ),
    ];
    for (code, expected) in cases {
        assert_eq!(run_to_error(code), expected, "code: {code}");
    }
}

/// `bytes` paths and integer fds are the kinds CPython accepts and Monty
/// never will, so the converter drops them from its accepted-types phrase
/// rather than listing the type it just rejected. CPython accepts these
/// calls, so they cannot dual-run in test_cases (see limitations/os.md).
#[test]
fn os_unsupported_path_kinds() {
    let cases = [
        (
            "import os\nos.listdir(b'/x')",
            "TypeError: listdir: path should be string, os.PathLike or None, not bytes",
        ),
        (
            "import os\nos.listdir(1)",
            "TypeError: listdir: path should be string, os.PathLike or None, not int",
        ),
        (
            "import os\nos.stat(b'/x')",
            "TypeError: stat: path should be string or os.PathLike, not bytes",
        ),
        (
            "import os\nos.stat(1)",
            "TypeError: stat: path should be string or os.PathLike, not int",
        ),
        // Bools fd-convert in CPython too (with a RuntimeWarning), so they are
        // narrowed exactly like ints where the converter allows fds.
        (
            "import os\nos.stat(True)",
            "TypeError: stat: path should be string or os.PathLike, not bool",
        ),
        (
            "import os\nos.listdir(True)",
            "TypeError: listdir: path should be string, os.PathLike or None, not bool",
        ),
        (
            "import os\nos.mkdir(b'/x')",
            "TypeError: mkdir: path should be string or os.PathLike, not bytes",
        ),
        (
            "import os\nos.rename('/a', b'/b')",
            "TypeError: rename: dst should be string or os.PathLike, not bytes",
        ),
        // `os.remove` has no fd support in CPython either, so an int keeps the
        // verbatim converter wording — it never listed `integer` to begin with.
        (
            "import os\nos.remove(1)",
            "TypeError: remove: path should be string, bytes or os.PathLike, not int",
        ),
    ];
    for (code, expected) in cases {
        assert_eq!(run_to_error(code), expected, "code: {code}");
    }
}

#[test]
fn os_listdir_rejected_in_sync_context_leaves_no_stale_effect() {
    // Regression: `map()` evaluates its function in a synchronous context
    // that cannot suspend, so the listdir OsCall is rejected and dropped
    // undispatched. The `ListdirNames` effect must travel with the dropped
    // call — if it leaked onto the VM, the next OS call's result (getenv
    // here) would be mangled by the name reduction.
    let code = r"
import os
try:
    list(map(os.listdir, ['/mnt']))
except NotImplementedError:
    pass
os.getenv('PROBE')
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::string("value".to_owned()));
    assert_eq!(func, "os.getenv");
    assert_eq!(result, MontyObject::string("value".to_owned()));
}

/// Drives `code` through a scripted sequence of OS calls, checking each call's
/// stable name and answering it with the paired mock result. Returns the final
/// completed value.
fn run_oscall_sequence(code: &str, steps: Vec<(&str, MontyObject)>) -> MontyObject {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    for (expected, result) in steps {
        let RunProgress::OsCall(call) = progress else {
            panic!("expected OsCall {expected}, got {progress:?}");
        };
        assert_eq!(call.function_call.name(), expected);
        progress = call.resume(result, PrintWriter::Stdout).unwrap();
    }
    progress.into_complete().expect("expected Complete")
}

/// A read-mode handle for `path`, the host's answer to an `open` OS call.
fn mock_file_handle(path: &str) -> MontyObject {
    MontyObject::file_handle(MontyFileHandle {
        path: path.to_owned(),
        mode: "r".parse::<FileMode>().unwrap(),
        position: 0,
    })
}

#[test]
fn buffered_read_rejected_in_sync_context_leaves_no_stale_effect() {
    // Regression: a `sorted()` key runs in a synchronous context, so the
    // buffered read's OsCall is rejected after the nested `run()` already
    // returned `FrameExit::OsCall`. A `BufferStore` outliving that rejection
    // diverts the *next* OS result into the abandoned file's buffer.
    let code = r"
from pathlib import Path

f = open('/data/sample.txt')
try:
    sorted([1], key=lambda x: f.read(5))
except NotImplementedError:
    pass
Path('/data/config.json').read_text()
";
    let result = run_oscall_sequence(
        code,
        vec![
            ("open", mock_file_handle("/data/sample.txt")),
            ("Path.read_text", MontyObject::string("victim contents".to_owned())),
        ],
    );
    assert_eq!(result, MontyObject::string("victim contents".to_owned()));
}

#[test]
fn buffered_read_in_sync_context_reports_the_rejected_os_function() {
    // The rejection users actually see (see `limitations/open.md`): the read
    // never reaches the host, so it names the OS call it would have made.
    let code = r"
f = open('/data/sample.txt')
try:
    sorted([1], key=lambda x: f.read(5))
except NotImplementedError as exc:
    msg = str(exc)
msg
";
    let result = run_oscall_sequence(code, vec![("open", mock_file_handle("/data/sample.txt"))]);
    assert_eq!(
        result,
        MontyObject::string(
            "sorted() key argument: OS function 'Path.read_text' is not yet supported in this context".to_owned()
        )
    );
}

#[test]
fn buffered_readlines_rejected_in_sync_context_does_not_retype_next_result() {
    // The same stale `BufferStore` armed by `readlines()` reshapes the next
    // result, handing Python a `list` where `read_text()` promises a `str`.
    let code = r"
from pathlib import Path

f = open('/data/sample.txt')
try:
    sorted([1], key=lambda x: f.readlines())
except NotImplementedError:
    pass
type(Path('/data/config.json').read_text()).__name__
";
    let result = run_oscall_sequence(
        code,
        vec![
            ("open", mock_file_handle("/data/sample.txt")),
            ("Path.read_text", MontyObject::string("a\nb\n".to_owned())),
        ],
    );
    assert_eq!(result, MontyObject::string("str".to_owned()));
}

#[test]
fn os_call_answered_with_a_future_does_not_strand_its_effect() {
    // A host may answer an OS call with `Future` instead of a value, and that
    // resume never consumes the armed effect. The next OS call must release
    // the stale one rather than overwrite it (leaking the file's pin) — and
    // must not let it reshape its own result, as it did before #711.
    let code = r"
import os
f = open('/data/sample.txt')
f.read(5)
os.getenv('PROBE')
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let RunProgress::OsCall(call) = progress else {
        panic!("expected open, got {progress:?}")
    };
    progress = call
        .resume(mock_file_handle("/data/sample.txt"), PrintWriter::Stdout)
        .unwrap();

    let RunProgress::OsCall(call) = progress else {
        panic!("expected the buffered read, got {progress:?}")
    };
    assert_eq!(call.function_call.name(), "Path.read_text");
    progress = call.resume(ExtFunctionResult::Future(7), PrintWriter::Stdout).unwrap();

    let RunProgress::OsCall(call) = progress else {
        panic!("expected getenv, got {progress:?}")
    };
    assert_eq!(call.function_call.name(), "os.getenv");
    let progress = call
        .resume(MontyObject::string("env-value".to_owned()), PrintWriter::Stdout)
        .unwrap();
    assert_eq!(
        progress.into_complete().expect("expected Complete"),
        MontyObject::string("env-value".to_owned())
    );
}

#[test]
fn future_answered_os_call_does_not_reshape_a_later_external_result() {
    // The same stranded effect, reached through a *non-OS* suspension: the
    // external call's return value must come back whole, not sliced by the
    // abandoned file's `BufferStore`.
    let code = r"
f = open('/data/sample.txt')
f.read(5)
some_external('x')
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::OsCall(call) = progress else {
        panic!("expected open, got {progress:?}")
    };
    progress = call
        .resume(mock_file_handle("/data/sample.txt"), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::OsCall(call) = progress else {
        panic!("expected the buffered read, got {progress:?}")
    };
    assert_eq!(call.function_call.name(), "Path.read_text");
    progress = call.resume(ExtFunctionResult::Future(7), PrintWriter::Stdout).unwrap();

    let RunProgress::FunctionCall(call) = progress else {
        panic!("expected the external call, got {progress:?}")
    };
    assert_eq!(call.function_name, "some_external");
    let progress = call
        .resume(MontyObject::string("external-result".to_owned()), PrintWriter::Stdout)
        .unwrap();
    assert_eq!(
        progress.into_complete().expect("expected Complete"),
        MontyObject::string("external-result".to_owned())
    );
}

// =============================================================================
// time.time() / time.sleep() / asyncio.sleep()
// =============================================================================

#[test]
fn time_time_yields_oscall() {
    let (func, args) = run_to_oscall("import time\ntime.time()");
    assert_eq!(func, "time.time");
    assert!(args.is_empty(), "time.time() takes no arguments, got {args:?}");
}

#[test]
fn time_time_result_used() {
    let code = "import time\ntime.time() + 1";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::float(1_700_000_000.5));
    assert_eq!(func, "time.time");
    assert_eq!(result, MontyObject::float(1_700_000_001.5));
}

#[test]
fn time_sleep_yields_the_delay_as_float_seconds() {
    let (func, args) = run_to_oscall("import time\ntime.sleep(1.5)");
    assert_eq!(func, "time.sleep");
    assert_eq!(args, vec![MontyObject::float(1.5)]);
    // an int length reaches the host as seconds too
    let (_, args) = run_to_oscall("import time\ntime.sleep(2)");
    assert_eq!(args, vec![MontyObject::float(2.0)]);
}

/// Whatever the host answers with, `time.sleep()` evaluates to `None` — the
/// `DiscardResult` effect, which also keeps a careless host from handing
/// sandboxed code a value CPython never produces.
#[test]
fn time_sleep_discards_the_host_answer() {
    let code = "import time\nrepr(time.sleep(0))";
    let (_, _, result) = run_oscall_with_result(code, MontyObject::string("surprise".to_owned()));
    assert_eq!(result, MontyObject::string("None".to_owned()));
}

/// A future would let execution continue before the wait it asked for ended,
/// so the sandbox refuses one rather than carrying on.
#[test]
fn time_sleep_refuses_a_future_answer() {
    let runner = MontyRun::new(
        "import time\ntime.sleep(0)".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::OsCall(call) = progress else {
        panic!("expected the sleep, got {progress:?}")
    };
    let err = call
        .resume(ExtFunctionResult::Future(1), PrintWriter::Stdout)
        .expect_err("a future is not an answer to a blocking wait");
    assert!(
        err.to_string()
            .ends_with("RuntimeError: time.sleep cannot be answered with a future"),
        "unexpected error: {err}"
    );
}

/// Only the delay reaches the host: `result` stays in the sandbox.
#[test]
fn asyncio_sleep_yields_only_the_delay() {
    let (func, args) = run_to_oscall("import asyncio\nasyncio.sleep(0.25, 'value')");
    assert_eq!(func, "asyncio.sleep");
    assert_eq!(args, vec![MontyObject::float(0.25)]);
}

/// The host answered immediately, so the `await` finds a settled awaitable
/// holding `result` rather than the host's answer.
#[test]
fn asyncio_sleep_answered_by_value_is_awaitable() {
    let code = "import asyncio\nasyncio.run(asyncio.sleep(0, 'woken'))";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::none());
    assert_eq!(func, "asyncio.sleep");
    assert_eq!(result, MontyObject::string("woken".to_owned()));
}

/// Both sleeps ignore the host's answer, so one with no sandbox form (a
/// host object's repr) completes them rather than failing conversion.
#[test]
fn sleeps_ignore_an_unconvertible_answer() {
    let repr = || MontyObject::repr("<host object>".to_owned());
    let (_, _, result) = run_oscall_with_result("import time\ntime.sleep(0)", repr());
    assert_eq!(result, MontyObject::none());
    let (_, _, result) = run_oscall_with_result("import asyncio\nasyncio.run(asyncio.sleep(0, 'woken'))", repr());
    assert_eq!(result, MontyObject::string("woken".to_owned()));
}

/// `result` never crosses the host boundary, so a value that has no wire
/// form — a function — survives, as in CPython.
#[test]
fn asyncio_sleep_result_need_not_be_convertible() {
    let code = "import asyncio\ndef f():\n    return 42\nasyncio.run(asyncio.sleep(0, f))()";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::none());
    assert_eq!(func, "asyncio.sleep");
    assert_eq!(result, MontyObject::int(42));
}

/// A host that runs an event loop answers with a future instead and resolves
/// it when the delay elapses; the awaiting task blocks until it does, then
/// produces `result` whatever the host resolved with.
#[test]
fn asyncio_sleep_answered_with_a_future_blocks_until_resolved() {
    let runner = MontyRun::new(
        "import asyncio\nasyncio.run(asyncio.sleep(5, 'late'))".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::OsCall(call) = progress else {
        panic!("expected the sleep, got {progress:?}")
    };
    assert_eq!(call.function_call.name(), "asyncio.sleep");
    let call_id = call.call_id;
    let progress = call
        .resume(ExtFunctionResult::Future(call_id), PrintWriter::Stdout)
        .unwrap();

    let RunProgress::ResolveFutures(state) = progress else {
        panic!("expected the await to block, got {progress:?}")
    };
    assert_eq!(state.pending_call_ids(), vec![call_id]);
    let progress = state
        .resume(vec![(call_id, MontyObject::none().into())], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(
        progress.into_complete().expect("expected Complete"),
        MontyObject::string("late".to_owned())
    );
}

/// An `asyncio.sleep` awaited at once, with nothing else to run, may be
/// answered eagerly: the wait is done, so the `await` finds the sleep
/// settled and no `ResolveFutures` round trip follows.
#[test]
fn asyncio_sleep_awaited_at_once_allows_an_eager_answer() {
    let code = "import asyncio\nasync def main():\n    return await asyncio.sleep(5, 'late')\nasyncio.run(main())";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::OsCall(call) = progress else {
        panic!("expected the sleep, got {progress:?}")
    };
    assert!(call.allow_eager_await);
    let progress = call.resume_eager(Ok(MontyObject::none()), PrintWriter::Stdout).unwrap();
    assert_eq!(
        progress.into_complete().expect("expected Complete"),
        MontyObject::string("late".to_owned())
    );
}

/// The eager hint needs an immediate `await`: a sleep passed to `asyncio.run`
/// is not awaited by the calling frame, and `time.sleep` never is.
#[test]
fn only_an_immediately_awaited_asyncio_sleep_allows_an_eager_answer() {
    for code in [
        "import asyncio\nasyncio.run(asyncio.sleep(0, 'x'))",
        "import time\ntime.sleep(0)",
    ] {
        let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
        let progress = runner
            .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
            .unwrap();
        let RunProgress::OsCall(call) = progress else {
            panic!("expected the sleep, got {progress:?}")
        };
        assert!(!call.allow_eager_await, "{code}");
    }
}

/// A future the host rejects raises at the `await`; `result` is dropped
/// with it rather than leaking.
#[test]
fn asyncio_sleep_answered_with_a_failed_future_raises() {
    let code = "import asyncio\nasync def main():\n    try:\n        await asyncio.sleep(5, [1, 2])\n    except OSError as e:\n        return str(e)\nasyncio.run(main())";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::OsCall(call) = progress else {
        panic!("expected the sleep, got {progress:?}")
    };
    let call_id = call.call_id;
    let progress = call
        .resume(ExtFunctionResult::Future(call_id), PrintWriter::Stdout)
        .unwrap();
    let RunProgress::ResolveFutures(state) = progress else {
        panic!("expected the await to block, got {progress:?}")
    };
    let interrupted = MontyException::new(ExcType::OSError, Some("wait interrupted".to_owned()));
    let progress = state
        .resume(
            vec![(call_id, ExtFunctionResult::Error(interrupted))],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(
        progress.into_complete().expect("expected Complete"),
        MontyObject::string("wait interrupted".to_owned())
    );
}

/// The positional arguments of a call as owned values.
fn positional(call: &CallArgs) -> Vec<MontyObject> {
    call.args().map(|arg| arg.to_owned()).collect()
}
