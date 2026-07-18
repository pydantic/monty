//! Tests for OS function calls.
//!
//! Verifies that Path filesystem methods and os module functions yield
//! `RunProgress::OsCall` with the correct `OsFunction` variant and arguments,
//! and that return values are correctly used by Python code.

use monty::{
    CompileOptions, FileMode, MkdirCallArgs, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyPath,
    MontyRun, NoLimitTracker, OpenCallArgs, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs, PrintWriter,
    RenameCallArgs, RunProgress, file_stat,
};

/// Helper to run code and extract the OsCall progress.
///
/// Runs the provided Python code and asserts that it yields an `OsCall`.
/// Returns the OS function name (stable `OsFunctionCall::name` string) and
/// positional args projected via `to_args`. State is resumed with a mock
/// result to properly clean up ref counts.
fn run_to_oscall(code: &str) -> (&'static str, Vec<MontyObject>) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

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
fn mock_oscall_result(call: &monty::OsFunctionCall) -> MontyObject {
    match call {
        monty::OsFunctionCall::Exists(_)
        | monty::OsFunctionCall::IsFile(_)
        | monty::OsFunctionCall::IsDir(_)
        | monty::OsFunctionCall::IsSymlink(_) => MontyObject::Bool(true),
        monty::OsFunctionCall::ReadText(_) | monty::OsFunctionCall::Resolve(_) | monty::OsFunctionCall::Absolute(_) => {
            MontyObject::String("mock".to_owned())
        }
        monty::OsFunctionCall::ReadBytes(_) => MontyObject::Bytes(vec![]),
        monty::OsFunctionCall::Stat(_) => MontyObject::None,
        monty::OsFunctionCall::Iterdir(_) => MontyObject::List(vec![]),
        monty::OsFunctionCall::WriteText(_)
        | monty::OsFunctionCall::WriteBytes(_)
        | monty::OsFunctionCall::AppendText(_)
        | monty::OsFunctionCall::AppendBytes(_)
        | monty::OsFunctionCall::Mkdir(_)
        | monty::OsFunctionCall::Unlink(_)
        | monty::OsFunctionCall::Rmdir(_)
        | monty::OsFunctionCall::Rename(_) => MontyObject::None,
        monty::OsFunctionCall::Open(_) => MontyObject::FileHandle(MontyFileHandle {
            path: "mock".to_owned(),
            mode: "r".parse::<FileMode>().unwrap(),
            position: 0,
        }),
        monty::OsFunctionCall::Getenv(_) => MontyObject::String("mock_env_value".to_owned()),
        monty::OsFunctionCall::GetEnviron => MontyObject::Dict(vec![].into()),
        monty::OsFunctionCall::DateToday => MontyObject::Date(MontyDate {
            year: 2023,
            month: 11,
            day: 14,
        }),
        monty::OsFunctionCall::DateTimeNow(_) => MontyObject::DateTime(MontyDateTime {
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
        monty::OsFunctionCall::Used => unreachable!("OsFunctionCall::Used in mock_oscall_result"),
    }
}

/// Helper to run code, provide an OS call result, and get the final value.
fn run_oscall_with_result(code: &str, mock_result: MontyObject) -> (&'static str, Vec<MontyObject>, MontyObject) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

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
// to_args / from_wire_args round-trip — keeps the parent-side inverse in sync.
// =============================================================================

/// Asserts `call` survives the `to_args` → `from_wire_args` round trip.
/// Compared via `Debug` since `OsFunctionCall` has no `PartialEq`.
fn assert_round_trip(call: OsFunctionCall) {
    let expected = format!("{call:?}");
    let name = call.name();
    let (args, kwargs) = call.to_args();
    let reconstructed = OsFunctionCall::from_wire_args(name, &args, &kwargs)
        .unwrap_or_else(|err| panic!("from_wire_args errored for {name}: {err}"))
        .unwrap_or_else(|| panic!("from_wire_args returned Ok(None) for {name}"));
    assert_eq!(format!("{reconstructed:?}"), expected);
}

#[test]
fn from_wire_args_round_trips_all_fs_variants() {
    let p = || MontyPath::new("/mnt/data/f.txt".to_owned());
    for call in [
        OsFunctionCall::Exists(p()),
        OsFunctionCall::IsFile(p()),
        OsFunctionCall::IsDir(p()),
        OsFunctionCall::IsSymlink(p()),
        OsFunctionCall::ReadText(p()),
        OsFunctionCall::ReadBytes(p()),
        OsFunctionCall::Stat(p()),
        OsFunctionCall::Iterdir(p()),
        OsFunctionCall::Resolve(p()),
        OsFunctionCall::Absolute(p()),
        OsFunctionCall::Unlink(p()),
        OsFunctionCall::Rmdir(p()),
        OsFunctionCall::WriteText(PathStringDataArgs {
            path: p(),
            data: "hello".to_owned(),
        }),
        OsFunctionCall::AppendText(PathStringDataArgs {
            path: p(),
            data: String::new(),
        }),
        OsFunctionCall::WriteBytes(PathBytesDataArgs {
            path: p(),
            data: vec![1, 2, 3],
        }),
        OsFunctionCall::AppendBytes(PathBytesDataArgs {
            path: p(),
            data: vec![],
        }),
        OsFunctionCall::Mkdir(MkdirCallArgs {
            path: p(),
            parents: true,
            exist_ok: false,
        }),
        OsFunctionCall::Rename(RenameCallArgs {
            src: p(),
            dst: MontyPath::new("/mnt/data/g.txt".to_owned()),
        }),
    ] {
        assert_round_trip(call);
    }
}

#[test]
fn from_wire_args_round_trips_open_all_modes() {
    // Only the modes `FromStr` produces — the `+` update modes are reserved
    // and unreachable from user input.
    for mode in ["r", "rb", "w", "wb", "a", "ab"] {
        assert_round_trip(OsFunctionCall::Open(OpenCallArgs {
            path: MontyPath::new("/mnt/data/f.txt".to_owned()),
            mode: mode.parse().unwrap(),
        }));
    }
}

#[test]
fn from_wire_args_classifies_malformed_input() {
    let path = || MontyObject::Path("/mnt/data/f.txt".to_owned());
    let s = || MontyObject::String("x".to_owned());
    let no_kw: Vec<(MontyObject, MontyObject)> = vec![];
    // Non-fs and unknown names are not errors — they surface to the caller.
    // The empty name is how an OsCall re-announced after a snapshot restore
    // arrives (its payload was consumed at first announcement).
    for name in ["os.getenv", "os.environ", "date.today", "datetime.now", "nonsense", ""] {
        assert!(matches!(
            OsFunctionCall::from_wire_args(name, &[path()], &no_kw),
            Ok(None)
        ));
    }
    // A recognized fs name with a malformed shape is a protocol error.
    let err = OsFunctionCall::from_wire_args("Path.read_text", &[], &no_kw).unwrap_err();
    assert_eq!(err, "invalid arguments for OS call 'Path.read_text'");
    // Wrong arity.
    assert!(OsFunctionCall::from_wire_args("Path.read_text", &[path(), path()], &no_kw).is_err());
    // Wrong types: str where a path is required, path where data is required.
    assert!(OsFunctionCall::from_wire_args("Path.read_text", &[s()], &no_kw).is_err());
    assert!(OsFunctionCall::from_wire_args("Path.write_text", &[path(), path()], &no_kw).is_err());
    // Unexpected kwargs on a positional-only call.
    assert!(OsFunctionCall::from_wire_args("Path.read_text", &[path()], &[(s(), s())]).is_err());
    // Bogus open mode.
    let bad_mode = MontyObject::String("q".to_owned());
    assert!(OsFunctionCall::from_wire_args("Open", &[path(), bad_mode], &no_kw).is_err());
    // Mkdir kwargs must be exactly parents/exist_ok bools.
    let kw = |k: &str, v: MontyObject| (MontyObject::String(k.to_owned()), v);
    assert!(OsFunctionCall::from_wire_args("Path.mkdir", &[path()], &no_kw).is_err());
    assert!(
        OsFunctionCall::from_wire_args(
            "Path.mkdir",
            &[path()],
            &[kw("parents", MontyObject::Bool(true)), kw("mode", MontyObject::Int(7))],
        )
        .is_err()
    );
    assert!(
        OsFunctionCall::from_wire_args(
            "Path.mkdir",
            &[path()],
            &[
                kw("parents", MontyObject::String("yes".to_owned())),
                kw("exist_ok", MontyObject::Bool(false)),
            ],
        )
        .is_err()
    );
}
