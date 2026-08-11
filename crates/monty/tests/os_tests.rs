//! Tests for OS function calls.
//!
//! Verifies that Path filesystem methods and os module functions yield
//! `RunProgress::OsCall` with the correct `OsFunction` variant and arguments,
//! and that return values are correctly used by Python code.

use monty::{MontyRun, RunProgress};
use monty_types::{
    CompileOptions, FileMode, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, OsFunctionCall, PrintWriter,
    ResourceTracker, file_stat,
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
            let (args, _) = call.function_call.clone().to_args();
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
        | OsFunctionCall::IsSymlink(_) => MontyObject::Bool(true),
        OsFunctionCall::ReadText(_) | OsFunctionCall::Resolve(_) | OsFunctionCall::Absolute(_) => {
            MontyObject::String("mock".to_owned())
        }
        OsFunctionCall::ReadBytes(_) => MontyObject::Bytes(vec![]),
        OsFunctionCall::Stat(_) => MontyObject::None,
        OsFunctionCall::Iterdir(_) => MontyObject::List(vec![]),
        OsFunctionCall::WriteText(_)
        | OsFunctionCall::WriteBytes(_)
        | OsFunctionCall::AppendText(_)
        | OsFunctionCall::AppendBytes(_)
        | OsFunctionCall::Mkdir(_)
        | OsFunctionCall::Unlink(_)
        | OsFunctionCall::Rmdir(_)
        | OsFunctionCall::Rename(_) => MontyObject::None,
        OsFunctionCall::Open(_) => MontyObject::FileHandle(MontyFileHandle {
            path: "mock".to_owned(),
            mode: "r".parse::<FileMode>().unwrap(),
            position: 0,
        }),
        OsFunctionCall::Getenv(_) => MontyObject::String("mock_env_value".to_owned()),
        OsFunctionCall::GetEnviron => MontyObject::Dict(vec![].into()),
        OsFunctionCall::DateToday => MontyObject::Date(MontyDate {
            year: 2023,
            month: 11,
            day: 14,
        }),
        OsFunctionCall::DateTimeNow(_) => MontyObject::DateTime(MontyDateTime {
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
            let (args, _) = call.function_call.clone().to_args();
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
    assert_eq!(args, vec![MontyObject::Path("/tmp/test.txt".to_owned())]);
}

#[test]
fn path_is_file() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/test.txt').is_file()");
    assert_eq!(func, "Path.is_file");
    assert_eq!(args, vec![MontyObject::Path("/tmp/test.txt".to_owned())]);
}

#[test]
fn path_is_dir() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp').is_dir()");
    assert_eq!(func, "Path.is_dir");
    assert_eq!(args, vec![MontyObject::Path("/tmp".to_owned())]);
}

#[test]
fn path_is_symlink() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/link').is_symlink()");
    assert_eq!(func, "Path.is_symlink");
    assert_eq!(args, vec![MontyObject::Path("/tmp/link".to_owned())]);
}

#[test]
fn path_read_text() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.txt').read_text()");
    assert_eq!(func, "Path.read_text");
    assert_eq!(args, vec![MontyObject::Path("/tmp/file.txt".to_owned())]);
}

#[test]
fn path_read_bytes() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.bin').read_bytes()");
    assert_eq!(func, "Path.read_bytes");
    assert_eq!(args, vec![MontyObject::Path("/tmp/file.bin".to_owned())]);
}

#[test]
fn path_stat() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.txt').stat()");
    assert_eq!(func, "Path.stat");
    assert_eq!(args, vec![MontyObject::Path("/tmp/file.txt".to_owned())]);
}

#[test]
fn path_iterdir() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp').iterdir()");
    assert_eq!(func, "Path.iterdir");
    assert_eq!(args, vec![MontyObject::Path("/tmp".to_owned())]);
}

#[test]
fn path_resolve() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('./relative').resolve()");
    assert_eq!(func, "Path.resolve");
    assert_eq!(args, vec![MontyObject::Path("relative".to_owned())]);
}

#[test]
fn path_absolute() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('./relative').absolute()");
    assert_eq!(func, "Path.absolute");
    assert_eq!(args, vec![MontyObject::Path("relative".to_owned())]);
}

// =============================================================================
// Path argument handling (spaces, unicode, concatenation)
// =============================================================================

#[test]
fn path_with_spaces() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/path/with spaces/file.txt').exists()");
    assert_eq!(func, "Path.exists");
    assert_eq!(args[0], MontyObject::Path("/path/with spaces/file.txt".to_owned()));
}

#[test]
fn path_with_unicode() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/путь/文件.txt').exists()");
    assert_eq!(func, "Path.exists");
    assert_eq!(args[0], MontyObject::Path("/путь/文件.txt".to_owned()));
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
    assert_eq!(args[0], MontyObject::Path("/home/user/file.txt".to_owned()));
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
    let (func, _, result) = run_oscall_with_result(code, MontyObject::Bool(true));
    assert_eq!(func, "Path.exists");
    assert_eq!(result, MontyObject::String("found".to_owned()));

    // Also test false case
    let (_, _, result) = run_oscall_with_result(code, MontyObject::Bool(false));
    assert_eq!(result, MontyObject::String("missing".to_owned()));
}

#[test]
fn read_text_result_concatenated() {
    let code = r"
from pathlib import Path
'Content: ' + Path('/tmp/hello.txt').read_text()
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::String("Hello!".to_owned()));
    assert_eq!(func, "Path.read_text");
    assert_eq!(result, MontyObject::String("Content: Hello!".to_owned()));
}

#[test]
fn read_bytes_result_used() {
    let code = r"
from pathlib import Path
data = Path('/tmp/file.bin').read_bytes()
data[0]
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::Bytes(vec![0x42, 0x43, 0x44]));
    assert_eq!(func, "Path.read_bytes");
    assert_eq!(result, MontyObject::Int(0x42));
}

#[test]
fn iterdir_result_iterated() {
    let code = r"
from pathlib import Path
entries = Path('/tmp').iterdir()
len(entries)
";
    // Return a list of path strings (simulating directory entries)
    let mock_entries = MontyObject::List(vec![
        MontyObject::String("/tmp/file1.txt".to_owned()),
        MontyObject::String("/tmp/file2.txt".to_owned()),
        MontyObject::String("/tmp/subdir".to_owned()),
    ]);
    let (func, args, result) = run_oscall_with_result(code, mock_entries);

    assert_eq!(func, "Path.iterdir");
    assert_eq!(args[0], MontyObject::Path("/tmp".to_owned()));
    assert_eq!(result, MontyObject::Int(3));
}

#[test]
fn iterdir_result_indexed() {
    let code = r"
from pathlib import Path
entries = Path('/home/user').iterdir()
entries[0]
";
    let mock_entries = MontyObject::List(vec![
        MontyObject::String("/home/user/documents".to_owned()),
        MontyObject::String("/home/user/downloads".to_owned()),
    ]);
    let (func, args, result) = run_oscall_with_result(code, mock_entries);

    assert_eq!(func, "Path.iterdir");
    assert_eq!(args[0], MontyObject::Path("/home/user".to_owned()));
    assert_eq!(result, MontyObject::String("/home/user/documents".to_owned()));
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
    assert_eq!(args[0], MontyObject::Path("/tmp/file.txt".to_owned()));
    assert_eq!(result, MontyObject::Int(1024));
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
    assert_eq!(args[0], MontyObject::Path("/tmp/file.txt".to_owned()));
    assert_eq!(result, MontyObject::Int(0o100_755));
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
    assert_eq!(args[0], MontyObject::Path("/var/log/syslog".to_owned()));
    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Int(4096), MontyObject::Int(0o100_644)])
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
    assert_eq!(args[0], MontyObject::Path("/tmp/file.txt".to_owned()));
    assert_eq!(result, MontyObject::Int(2048));
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
    assert_eq!(args[0], MontyObject::String("PATH".to_owned()));
    assert_eq!(args[1], MontyObject::None);
}

#[test]
fn os_getenv_with_default() {
    let code = r"
import os
os.getenv('MISSING', 'fallback')
";
    let (func, args) = run_to_oscall(code);
    assert_eq!(func, "os.getenv");
    assert_eq!(args[0], MontyObject::String("MISSING".to_owned()));
    assert_eq!(args[1], MontyObject::String("fallback".to_owned()));
}

#[test]
fn os_getenv_result_used() {
    let code = r"
import os
'HOME=' + os.getenv('HOME')
";
    let (func, _, result) = run_oscall_with_result(code, MontyObject::String("/home/user".to_owned()));
    assert_eq!(func, "os.getenv");
    assert_eq!(result, MontyObject::String("HOME=/home/user".to_owned()));
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
    let mock_env = MontyObject::Dict(
        vec![
            (
                MontyObject::String("HOME".to_owned()),
                MontyObject::String("/home/user".to_owned()),
            ),
            (
                MontyObject::String("PATH".to_owned()),
                MontyObject::String("/usr/bin".to_owned()),
            ),
        ]
        .into(),
    );
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::String("dict".to_owned()));
}

#[test]
fn os_environ_key_access() {
    let code = r"
import os
os.environ['HOME']
";
    let mock_env = MontyObject::Dict(
        vec![(
            MontyObject::String("HOME".to_owned()),
            MontyObject::String("/home/user".to_owned()),
        )]
        .into(),
    );
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::String("/home/user".to_owned()));
}

#[test]
fn os_environ_get_method() {
    let code = r"
import os
os.environ.get('MISSING', 'default')
";
    let mock_env = MontyObject::Dict(vec![].into());
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::String("default".to_owned()));
}

#[test]
fn os_environ_len() {
    let code = r"
import os
len(os.environ)
";
    let mock_env = MontyObject::Dict(
        vec![
            (MontyObject::String("A".to_owned()), MontyObject::String("1".to_owned())),
            (MontyObject::String("B".to_owned()), MontyObject::String("2".to_owned())),
            (MontyObject::String("C".to_owned()), MontyObject::String("3".to_owned())),
        ]
        .into(),
    );
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::Int(3));
}

#[test]
fn os_environ_in_check() {
    let code = r"
import os
'HOME' in os.environ
";
    let mock_env = MontyObject::Dict(
        vec![(
            MontyObject::String("HOME".to_owned()),
            MontyObject::String("/home/user".to_owned()),
        )]
        .into(),
    );
    let (func, _, result) = run_oscall_with_result(code, mock_env);
    assert_eq!(func, "os.environ");
    assert_eq!(result, MontyObject::Bool(true));
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
        MontyObject::List(vec![
            MontyObject::Path("/mnt/data/b.txt".to_owned()),
            MontyObject::Path("/mnt/data/a.txt".to_owned()),
            MontyObject::Path("/mnt/data/sub".to_owned()),
        ]),
    );
    // os.listdir reuses the Path.iterdir OS call — hosts see that name.
    assert_eq!(func, "Path.iterdir");
    assert_eq!(args, vec![MontyObject::Path("/mnt/data".to_owned())]);
    assert_eq!(
        result,
        MontyObject::List(vec![
            MontyObject::String("b.txt".to_owned()),
            MontyObject::String("a.txt".to_owned()),
            MontyObject::String("sub".to_owned()),
        ])
    );
}

#[test]
fn os_listdir_default_path_is_dot() {
    let (func, args, result) = run_oscall_with_result("import os\nos.listdir()", MontyObject::List(vec![]));
    assert_eq!(func, "Path.iterdir");
    assert_eq!(args, vec![MontyObject::Path(".".to_owned())]);
    assert_eq!(result, MontyObject::List(vec![]));
}

#[test]
fn os_listdir_accepts_host_strings() {
    // Hosts serving the `Path.iterdir` callback directly may return plain
    // strings instead of paths — names are stripped the same way.
    let (_, _, result) = run_oscall_with_result(
        "import os\nos.listdir('/mnt')",
        MontyObject::List(vec![
            MontyObject::String("/mnt/x.txt".to_owned()),
            MontyObject::String("plain".to_owned()),
        ]),
    );
    assert_eq!(
        result,
        MontyObject::List(vec![
            MontyObject::String("x.txt".to_owned()),
            MontyObject::String("plain".to_owned()),
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
        .resume(MontyObject::List(vec![MontyObject::Int(3)]), PrintWriter::Stdout)
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
    assert_eq!(args, vec![MontyObject::Path("/tmp/file.txt".to_owned())]);
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
        let _ = call.resume(MontyObject::None, PrintWriter::Stdout);
    }
}

#[test]
fn os_remove_and_unlink_yield_unlink_call() {
    let (func, args) = run_to_oscall("import os\nos.remove('/mnt/f.txt')");
    assert_eq!(func, "Path.unlink");
    assert_eq!(args, vec![MontyObject::Path("/mnt/f.txt".to_owned())]);

    let (func, args) = run_to_oscall("import os\nos.unlink('/mnt/g.txt')");
    assert_eq!(func, "Path.unlink");
    assert_eq!(args, vec![MontyObject::Path("/mnt/g.txt".to_owned())]);
}

#[test]
fn os_rmdir_yields_rmdir_call() {
    let (func, args) = run_to_oscall("import os\nos.rmdir('/mnt/d')");
    assert_eq!(func, "Path.rmdir");
    assert_eq!(args, vec![MontyObject::Path("/mnt/d".to_owned())]);
}

#[test]
fn os_rename_and_replace_yield_rename_call() {
    let (func, args) = run_to_oscall("import os\nos.rename('/mnt/a', '/mnt/b')");
    assert_eq!(func, "Path.rename");
    assert_eq!(
        args,
        vec![
            MontyObject::Path("/mnt/a".to_owned()),
            MontyObject::Path("/mnt/b".to_owned())
        ]
    );

    let (func, args) = run_to_oscall("import os\nos.replace('/mnt/a', '/mnt/b')");
    assert_eq!(func, "Path.rename");
    assert_eq!(
        args,
        vec![
            MontyObject::Path("/mnt/a".to_owned()),
            MontyObject::Path("/mnt/b".to_owned())
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
    let (func, _, result) = run_oscall_with_result(code, MontyObject::String("value".to_owned()));
    assert_eq!(func, "os.getenv");
    assert_eq!(result, MontyObject::String("value".to_owned()));
}
