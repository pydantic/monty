//! Tests for OS function calls.
//!
//! Verifies that Path filesystem methods and os module functions yield
//! `RunProgress::OsCall` with the correct `OsFunction` variant and arguments,
//! and that return values are correctly used by Python code.

use monty::{MontyObject, MontyRun, NoLimitTracker, OsFunction, PrintWriter, RunProgress, file_stat};

/// Helper to run code and extract the OsCall progress.
///
/// Runs the provided Python code and asserts that it yields an `OsCall`.
/// Returns the `OsFunction` and positional arguments from the call.
/// The state is resumed with a mock result to properly clean up ref counts.
fn run_to_oscall(code: &str) -> (OsFunction, Vec<MontyObject>) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    match progress {
        RunProgress::OsCall {
            function, args, state, ..
        } => {
            // Resume with a mock result appropriate for the function type.
            let mock_result = match function {
                OsFunction::Exists | OsFunction::IsFile | OsFunction::IsDir | OsFunction::IsSymlink => {
                    MontyObject::Bool(true)
                }
                OsFunction::ReadText | OsFunction::Resolve | OsFunction::Absolute => {
                    MontyObject::String("mock".to_owned())
                }
                OsFunction::ReadBytes => MontyObject::Bytes(vec![]),
                OsFunction::Stat => MontyObject::None,
                OsFunction::Iterdir => MontyObject::List(vec![]),
                OsFunction::WriteText
                | OsFunction::WriteBytes
                | OsFunction::Mkdir
                | OsFunction::Unlink
                | OsFunction::Rmdir
                | OsFunction::Rename => MontyObject::None,
                OsFunction::Getenv => MontyObject::String("mock_env_value".to_owned()),
                OsFunction::GetEnviron => MontyObject::Dict(vec![].into()),
            };
            let _ = state.run(mock_result, &mut PrintWriter::Stdout);
            (function, args)
        }
        _ => panic!("expected OsCall, got {progress:?}"),
    }
}

/// Helper to run code, provide an OS call result, and get the final value.
fn run_oscall_with_result(code: &str, mock_result: MontyObject) -> (OsFunction, Vec<MontyObject>, MontyObject) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    match progress {
        RunProgress::OsCall {
            function, args, state, ..
        } => {
            let resumed = state.run(mock_result, &mut PrintWriter::Stdout).unwrap();
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
    assert_eq!(func, OsFunction::Exists);
    assert_eq!(args, vec![MontyObject::Path("/tmp/test.txt".to_owned())]);
}

#[test]
fn path_is_file() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/test.txt').is_file()");
    assert_eq!(func, OsFunction::IsFile);
    assert_eq!(args, vec![MontyObject::Path("/tmp/test.txt".to_owned())]);
}

#[test]
fn path_is_dir() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp').is_dir()");
    assert_eq!(func, OsFunction::IsDir);
    assert_eq!(args, vec![MontyObject::Path("/tmp".to_owned())]);
}

#[test]
fn path_is_symlink() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/link').is_symlink()");
    assert_eq!(func, OsFunction::IsSymlink);
    assert_eq!(args, vec![MontyObject::Path("/tmp/link".to_owned())]);
}

#[test]
fn path_read_text() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.txt').read_text()");
    assert_eq!(func, OsFunction::ReadText);
    assert_eq!(args, vec![MontyObject::Path("/tmp/file.txt".to_owned())]);
}

#[test]
fn path_read_bytes() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.bin').read_bytes()");
    assert_eq!(func, OsFunction::ReadBytes);
    assert_eq!(args, vec![MontyObject::Path("/tmp/file.bin".to_owned())]);
}

#[test]
fn path_stat() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp/file.txt').stat()");
    assert_eq!(func, OsFunction::Stat);
    assert_eq!(args, vec![MontyObject::Path("/tmp/file.txt".to_owned())]);
}

#[test]
fn path_iterdir() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/tmp').iterdir()");
    assert_eq!(func, OsFunction::Iterdir);
    assert_eq!(args, vec![MontyObject::Path("/tmp".to_owned())]);
}

#[test]
fn path_resolve() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('./relative').resolve()");
    assert_eq!(func, OsFunction::Resolve);
    assert_eq!(args, vec![MontyObject::Path("./relative".to_owned())]);
}

#[test]
fn path_absolute() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('./relative').absolute()");
    assert_eq!(func, OsFunction::Absolute);
    assert_eq!(args, vec![MontyObject::Path("./relative".to_owned())]);
}

// =============================================================================
// Path argument handling (spaces, unicode, concatenation)
// =============================================================================

#[test]
fn path_with_spaces() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/path/with spaces/file.txt').exists()");
    assert_eq!(func, OsFunction::Exists);
    assert_eq!(args[0], MontyObject::Path("/path/with spaces/file.txt".to_owned()));
}

#[test]
fn path_with_unicode() {
    let (func, args) = run_to_oscall("from pathlib import Path; Path('/путь/文件.txt').exists()");
    assert_eq!(func, OsFunction::Exists);
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
    assert_eq!(func, OsFunction::Exists);
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
    assert_eq!(func, OsFunction::Exists);
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
    assert_eq!(func, OsFunction::ReadText);
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
    assert_eq!(func, OsFunction::ReadBytes);
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

    assert_eq!(func, OsFunction::Iterdir);
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

    assert_eq!(func, OsFunction::Iterdir);
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

    assert_eq!(func, OsFunction::Stat);
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

    assert_eq!(func, OsFunction::Stat);
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

    assert_eq!(func, OsFunction::Stat);
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

    assert_eq!(func, OsFunction::Stat);
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
    assert_eq!(func, OsFunction::Getenv);
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
    assert_eq!(func, OsFunction::Getenv);
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
    assert_eq!(func, OsFunction::Getenv);
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
    assert_eq!(func, OsFunction::GetEnviron);
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
    assert_eq!(func, OsFunction::GetEnviron);
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
    assert_eq!(func, OsFunction::GetEnviron);
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
    assert_eq!(func, OsFunction::GetEnviron);
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
    assert_eq!(func, OsFunction::GetEnviron);
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
    assert_eq!(func, OsFunction::GetEnviron);
    assert_eq!(result, MontyObject::Bool(true));
}
