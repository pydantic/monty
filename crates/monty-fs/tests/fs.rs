//! Integration tests for filesystem mount operations.
//!
//! Tests `MountTable::handle_os_call()` across all supported mount modes (ReadWrite,
//! ReadOnly, OverlayMemory) and all supported filesystem
//! operations. Uses real temporary directories to verify correct behavior.

#[cfg(unix)]
use std::os::unix::fs::symlink as unix_symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as win_symlink_file;
use std::{fs, path::Path};

use monty::{
    ExcType, MkdirCallArgs, MontyException, MontyObject, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs,
    RenameCallArgs, UnicodeErrorData, UnicodeErrorObject,
};
use monty_fs::{DEFAULT_MEMORY_USAGE_LIMIT, Mount, MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
use tempfile::TempDir;

// =============================================================================
// Helpers
// =============================================================================

/// Creates the standard test directory structure used across all tests.
///
/// ```text
/// tmpdir/
///   hello.txt          -> "hello world\n"
///   empty.txt          -> ""
///   data.bin           -> b"\x00\x01\x02\x03"
///   subdir/
///     nested.txt       -> "nested content"
///     deep/
///       file.txt       -> "deep file"
///   readonly.txt       -> "readonly content"
/// ```
fn create_test_dir() -> TempDir {
    let dir = TempDir::new().expect("failed to create temp dir");
    let p = dir.path();

    fs::write(p.join("hello.txt"), "hello world\n").unwrap();
    fs::write(p.join("empty.txt"), "").unwrap();
    fs::write(p.join("data.bin"), b"\x00\x01\x02\x03").unwrap();
    fs::create_dir_all(p.join("subdir/deep")).unwrap();
    fs::write(p.join("subdir/nested.txt"), "nested content").unwrap();
    fs::write(p.join("subdir/deep/file.txt"), "deep file").unwrap();
    fs::write(p.join("readonly.txt"), "readonly content").unwrap();

    dir
}

/// Creates a `MountTable` with a single mount at `/mnt`.
fn mount_at_mnt(tmpdir: &TempDir, mode: MountMode) -> MountTable {
    let mut mt = MountTable::new();
    mt.mount("/mnt", tmpdir.path(), mode, None).unwrap();
    mt
}

/// Shorthand: dispatch an `OsFunctionCall` through the mount table, adapting
/// the owning `handle_os_call` API back to the `Option` shape assertions use.
/// Clones the call — fine here, test payloads are tiny.
fn call(mt: &mut MountTable, c: &OsFunctionCall) -> Option<Result<MontyObject, MountError>> {
    match mt.handle_os_call(c.clone()) {
        MountCallOutcome::Handled(result) => Some(result),
        MountCallOutcome::NotHandled(_) => None,
    }
}

/// Shorthand: call and unwrap both the Option and Result.
fn call_ok(mt: &mut MountTable, c: &OsFunctionCall) -> MontyObject {
    call(mt, c).expect("expected Some").expect("expected Ok")
}

/// Shorthand: call and unwrap Option, expect Err, convert to exception.
fn call_err(mt: &mut MountTable, c: &OsFunctionCall) -> MontyException {
    call(mt, c)
        .expect("expected Some")
        .expect_err("expected Err")
        .into_exception()
}

/// Build a `WriteText` call.
fn write_text(path: &str, data: impl Into<String>) -> OsFunctionCall {
    OsFunctionCall::WriteText(PathStringDataArgs {
        path: path.into(),
        data: data.into(),
    })
}

/// Build a `WriteBytes` call.
fn write_bytes(path: &str, data: Vec<u8>) -> OsFunctionCall {
    OsFunctionCall::WriteBytes(PathBytesDataArgs {
        path: path.into(),
        data,
    })
}

/// Build an `AppendBytes` call.
fn append_bytes(path: &str, data: Vec<u8>) -> OsFunctionCall {
    OsFunctionCall::AppendBytes(PathBytesDataArgs {
        path: path.into(),
        data,
    })
}

/// Build a `Mkdir` call with kwargs.
fn mkdir(path: &str, parents: bool, exist_ok: bool) -> OsFunctionCall {
    OsFunctionCall::Mkdir(MkdirCallArgs {
        path: path.into(),
        parents,
        exist_ok,
    })
}

/// Build a `Rename` call.
fn rename(src: &str, dst: &str) -> OsFunctionCall {
    OsFunctionCall::Rename(RenameCallArgs {
        src: src.into(),
        dst: dst.into(),
    })
}

/// Creates a file symlink, handling platform differences.
///
/// On Unix, uses `std::os::unix::fs::symlink`. On Windows, uses
/// `std::os::windows::fs::symlink_file`.
fn symlink_file(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    unix_symlink(original.as_ref(), link.as_ref()).unwrap();

    #[cfg(windows)]
    win_symlink_file(original.as_ref(), link.as_ref()).unwrap();
}

/// Asserts an exception has the expected type and message.
#[track_caller]
fn assert_exc(exc: &MontyException, expected_type: ExcType, expected_msg: &str) {
    assert_eq!(exc.exc_type(), expected_type, "wrong exception type");
    assert_eq!(exc.message().unwrap_or(""), expected_msg, "wrong exception message");
}

/// Extracts entry names from an iterdir result list, sorted for deterministic comparison.
fn sorted_names(obj: &MontyObject) -> Vec<String> {
    match obj {
        MontyObject::List(items) => {
            let mut names: Vec<String> = items
                .iter()
                .map(|item| match item {
                    MontyObject::Path(p) => p.rsplit('/').next().unwrap().to_owned(),
                    other => panic!("expected Path in iterdir result, got {other:?}"),
                })
                .collect();
            names.sort();
            names
        }
        other => panic!("expected List from iterdir, got {other:?}"),
    }
}

// =============================================================================
// ReadWrite mode
// =============================================================================

#[test]
fn rw_exists() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/nonexistent".into())),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_is_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsFile("/mnt/hello.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsFile("/mnt/subdir".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsFile("/mnt/nonexistent".into())),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_is_dir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/subdir".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/subdir/deep".into())),
        MontyObject::Bool(true)
    );
}

#[test]
fn rw_is_symlink() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsSymlink("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_is_symlink_true_for_symlink() {
    let dir = create_test_dir();
    symlink_file(dir.path().join("hello.txt"), dir.path().join("link.txt"));
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Symlink should be detected as a symlink
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsSymlink("/mnt/link.txt".into())),
        MontyObject::Bool(true)
    );
    // Target file should NOT be detected as a symlink
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsSymlink("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    // Nonexistent path should return false
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsSymlink("/mnt/nope.txt".into())),
        MontyObject::Bool(false)
    );
}

#[test]
fn overlay_is_symlink_true_for_symlink() {
    let dir = create_test_dir();
    symlink_file(dir.path().join("hello.txt"), dir.path().join("link.txt"));
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsSymlink("/mnt/link.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsSymlink("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_read_text() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/hello.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/empty.txt".into())),
        MontyObject::String(String::new())
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/subdir/nested.txt".into())),
        MontyObject::String("nested content".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/subdir/deep/file.txt".into())),
        MontyObject::String("deep file".to_owned())
    );
}

/// Text-mode reads of invalid UTF-8 must raise `UnicodeDecodeError` with the
/// same reason wording CPython uses for `bytes.decode('utf-8')` — including
/// the distinction between an invalid continuation byte mid-file and a
/// sequence truncated at end-of-file (previously both reported the sometimes
/// wrong `invalid start byte`).
#[test]
fn rw_read_text_invalid_utf8() {
    let dir = create_test_dir();
    fs::write(dir.path().join("bad_start.txt"), b"a\xffb").unwrap();
    fs::write(dir.path().join("bad_continuation.txt"), b"a\xe2(b").unwrap();
    fs::write(dir.path().join("truncated.txt"), b"ab\xe2\x82").unwrap();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_err(&mut mt, &OsFunctionCall::ReadText("/mnt/bad_start.txt".into()));
    assert_exc(
        &err,
        ExcType::UnicodeDecodeError,
        "'utf-8' codec can't decode byte 0xff in position 1: invalid start byte",
    );
    // file-read decode errors carry the structured constructor fields
    // (including the file's bytes) just like `bytes.decode()`
    assert_eq!(
        err.unicode_data(),
        Some(&UnicodeErrorData {
            encoding: "utf-8".to_owned(),
            object: UnicodeErrorObject::Bytes(b"a\xffb".to_vec()),
            start: 1,
            end: 2,
            reason: "invalid start byte".to_owned(),
        })
    );
    let err = call_err(&mut mt, &OsFunctionCall::ReadText("/mnt/bad_continuation.txt".into()));
    assert_exc(
        &err,
        ExcType::UnicodeDecodeError,
        "'utf-8' codec can't decode byte 0xe2 in position 1: invalid continuation byte",
    );
    let err = call_err(&mut mt, &OsFunctionCall::ReadText("/mnt/truncated.txt".into()));
    assert_exc(
        &err,
        ExcType::UnicodeDecodeError,
        "'utf-8' codec can't decode bytes in position 2-3: unexpected end of data",
    );
}

#[test]
fn rw_read_text_not_found() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_err(&mut mt, &OsFunctionCall::ReadText("/mnt/nonexistent.txt".into()));
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'",
    );
}

#[test]
fn rw_read_bytes() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/data.bin".into())),
        MontyObject::Bytes(vec![0x00, 0x01, 0x02, 0x03])
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/empty.txt".into())),
        MontyObject::Bytes(vec![])
    );
}

#[test]
fn rw_write_text_and_read_back() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &write_text("/mnt/new_file.txt", "new content".to_owned()))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/new_file.txt".into())),
        MontyObject::String("new content".to_owned())
    );
    // Verify host file was actually written (ReadWrite mode).
    assert_eq!(
        fs::read_to_string(dir.path().join("new_file.txt")).unwrap(),
        "new content"
    );
}

#[test]
fn rw_write_bytes_and_read_back() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &write_bytes("/mnt/out.bin", vec![0xff, 0xfe]))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/out.bin".into())),
        MontyObject::Bytes(vec![0xff, 0xfe])
    );
}

#[test]
fn rw_overwrite_existing() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &write_text("/mnt/hello.txt", "overwritten".to_owned()))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/hello.txt".into())),
        MontyObject::String("overwritten".to_owned())
    );
}

#[test]
fn rw_stat_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let stat = call_ok(&mut mt, &OsFunctionCall::Stat("/mnt/hello.txt".into()));
    // stat returns a NamedTuple; check st_size at index 6
    match &stat {
        MontyObject::NamedTuple { values, .. } => {
            assert_eq!(values[6], MontyObject::Int(12), "st_size should be 12");
        }
        other => panic!("expected NamedTuple from stat, got {other:?}"),
    }
}

#[test]
fn rw_stat_dir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let stat = call_ok(&mut mt, &OsFunctionCall::Stat("/mnt/subdir".into()));
    match &stat {
        MontyObject::NamedTuple { values, .. } => {
            // st_mode should have directory type bits (0o040_000)
            if let MontyObject::Int(mode) = values[0] {
                assert_eq!(mode & 0o170_000, 0o040_000, "should be directory type");
            } else {
                panic!("st_mode should be Int");
            }
        }
        other => panic!("expected NamedTuple from stat, got {other:?}"),
    }
}

#[test]
fn rw_iterdir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt".into()));
    let names = sorted_names(&result);
    assert_eq!(
        names,
        vec!["data.bin", "empty.txt", "hello.txt", "readonly.txt", "subdir"]
    );
}

#[test]
fn rw_iterdir_nested() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt/subdir".into()));
    let names = sorted_names(&result);
    assert_eq!(names, vec!["deep", "nested.txt"]);
}

#[test]
fn rw_mkdir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &mkdir("/mnt/new_dir", false, false)).unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/new_dir".into())),
        MontyObject::Bool(true)
    );
    assert!(dir.path().join("new_dir").is_dir());
}

#[test]
fn rw_mkdir_parents() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &mkdir("/mnt/a/b/c", true, false)).unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/a/b/c".into())),
        MontyObject::Bool(true)
    );
}

#[test]
fn rw_mkdir_exist_ok() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &mkdir("/mnt/subdir", false, true)).unwrap().unwrap();
}

#[test]
#[cfg(not(target_os = "windows"))]
fn rw_mkdir_already_exists_error() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call(&mut mt, &mkdir("/mnt/subdir", false, false))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(&err, ExcType::FileExistsError, "[Errno 17] File exists: '/mnt/subdir'");
}

#[test]
fn rw_unlink() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(true)
    );
    call(&mut mt, &OsFunctionCall::Unlink("/mnt/hello.txt".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    assert!(!dir.path().join("hello.txt").exists());
}

#[test]
fn rw_unlink_not_found() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_err(&mut mt, &OsFunctionCall::Unlink("/mnt/nonexistent.txt".into()));
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'",
    );
}

#[test]
fn rw_rmdir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &mkdir("/mnt/empty_dir", false, false)).unwrap().unwrap();
    call(&mut mt, &OsFunctionCall::Rmdir("/mnt/empty_dir".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/empty_dir".into())),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_rename() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call(&mut mt, &rename("/mnt/hello.txt", "/mnt/renamed.txt"))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/renamed.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn rw_resolve() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Resolve("/mnt/subdir/../hello.txt".into())),
        MontyObject::Path("/mnt/hello.txt".to_owned())
    );
}

#[test]
fn rw_absolute() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Absolute("/mnt/./subdir".into())),
        MontyObject::Path("/mnt/subdir".to_owned())
    );
}

// =============================================================================
// ReadOnly mode
// =============================================================================

#[test]
fn ro_reads_work() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsFile("/mnt/hello.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/subdir".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/hello.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/data.bin".into())),
        MontyObject::Bytes(vec![0x00, 0x01, 0x02, 0x03])
    );

    // stat and iterdir should work
    let _stat = call_ok(&mut mt, &OsFunctionCall::Stat("/mnt/hello.txt".into()));
    let _entries = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt".into()));
}

#[test]
fn ro_write_text_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call(&mut mt, &write_text("/mnt/new.txt", "blocked".to_owned()))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::PermissionError,
        "[Errno 30] Read-only file system: '/mnt/new.txt'",
    );
}

#[test]
fn ro_write_bytes_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call(&mut mt, &write_bytes("/mnt/new.bin", vec![0x00]))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::PermissionError,
        "[Errno 30] Read-only file system: '/mnt/new.bin'",
    );
}

#[test]
fn ro_mkdir_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call(&mut mt, &mkdir("/mnt/newdir", false, false))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::PermissionError,
        "[Errno 30] Read-only file system: '/mnt/newdir'",
    );
}

#[test]
fn ro_unlink_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call_err(&mut mt, &OsFunctionCall::Unlink("/mnt/hello.txt".into()));
    assert_exc(
        &err,
        ExcType::PermissionError,
        "[Errno 30] Read-only file system: '/mnt/hello.txt'",
    );
}

#[test]
fn ro_rmdir_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call_err(&mut mt, &OsFunctionCall::Rmdir("/mnt/subdir".into()));
    assert_exc(
        &err,
        ExcType::PermissionError,
        "[Errno 30] Read-only file system: '/mnt/subdir'",
    );
}

#[test]
fn ro_rename_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call(&mut mt, &rename("/mnt/hello.txt", "/mnt/renamed.txt"))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::PermissionError,
        "[Errno 30] Read-only file system: '/mnt/hello.txt'",
    );
}

// =============================================================================
// OverlayMemory mode
// =============================================================================

#[test]
fn ovl_mem_reads_fall_through() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/hello.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/data.bin".into())),
        MontyObject::Bytes(vec![0x00, 0x01, 0x02, 0x03])
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/subdir".into())),
        MontyObject::Bool(true)
    );
}

#[test]
fn ovl_mem_write_readable_back() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(
        &mut mt,
        &write_text("/mnt/new_overlay.txt", "overlay content".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/new_overlay.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/new_overlay.txt".into())),
        MontyObject::String("overlay content".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsFile("/mnt/new_overlay.txt".into())),
        MontyObject::Bool(true)
    );
}

#[test]
fn ovl_mem_write_does_not_modify_host() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &write_text("/mnt/hello.txt", "overlay overwrite".to_owned()))
        .unwrap()
        .unwrap();

    // Overlay returns the new content.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/hello.txt".into())),
        MontyObject::String("overlay overwrite".to_owned())
    );
    // Host file remains unchanged.
    assert_eq!(
        fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
        "hello world\n"
    );
}

#[test]
fn ovl_mem_tombstone() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Delete a real file.
    call(&mut mt, &OsFunctionCall::Unlink("/mnt/hello.txt".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    // Host file still exists.
    assert!(dir.path().join("hello.txt").exists());
}

#[test]
fn ovl_mem_iterdir_merges() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Write a new overlay file.
    call(&mut mt, &write_text("/mnt/overlay_new.txt", "new".to_owned()))
        .unwrap()
        .unwrap();

    let result = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt".into()));
    let names = sorted_names(&result);
    assert!(names.contains(&"hello.txt".to_owned()), "should contain real files");
    assert!(
        names.contains(&"overlay_new.txt".to_owned()),
        "should contain overlay files"
    );
}

#[test]
fn ovl_mem_iterdir_respects_tombstones() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &OsFunctionCall::Unlink("/mnt/hello.txt".into()))
        .unwrap()
        .unwrap();

    let result = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt".into()));
    let names = sorted_names(&result);
    assert!(
        !names.contains(&"hello.txt".to_owned()),
        "tombstoned file should be hidden"
    );
}

#[test]
fn ovl_mem_iterdir_missing_directory_errors() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let err = call_err(&mut mt, &OsFunctionCall::Iterdir("/mnt/no_such_dir".into()));
    assert_eq!(err.exc_type(), ExcType::FileNotFoundError);
}

#[test]
fn ovl_mem_iterdir_file_errors() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let err = call_err(&mut mt, &OsFunctionCall::Iterdir("/mnt/hello.txt".into()));
    assert_eq!(err.exc_type(), ExcType::NotADirectoryError);
}

#[test]
fn ovl_mem_path_component_too_long() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // 256-byte component exceeds NAME_MAX (255)
    let long_name = "a".repeat(256);
    let path = format!("/mnt/{long_name}");

    let err = call_err(&mut mt, &OsFunctionCall::Exists(path.as_str().into()));
    assert_exc(
        &err,
        ExcType::OSError,
        &format!("[Errno 36] File name too long: '{path}'"),
    );

    // 255-byte component is fine
    let ok_name = "b".repeat(255);
    let ok_path = format!("/mnt/{ok_name}");
    call(&mut mt, &OsFunctionCall::Exists(ok_path.into()))
        .expect("expected Some")
        .expect("expected Ok");
}

#[test]
fn ovl_mem_path_total_too_long() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Path exceeding 4096 bytes total
    let segment = "x".repeat(200);
    let segments: Vec<&str> = (0..21).map(|_| segment.as_str()).collect();
    let path = format!("/mnt/{}", segments.join("/"));
    assert!(path.len() > 4096);

    let err = call_err(&mut mt, &OsFunctionCall::Exists(path.as_str().into()));
    assert_exc(
        &err,
        ExcType::OSError,
        &format!("[Errno 36] File name too long: '{path}'"),
    );
}

#[test]
fn rw_path_component_too_long() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let long_name = "a".repeat(256);
    let path = format!("/mnt/{long_name}");

    let err = call_err(&mut mt, &OsFunctionCall::Stat(path.as_str().into()));
    assert_exc(
        &err,
        ExcType::OSError,
        &format!("[Errno 36] File name too long: '{path}'"),
    );
}

#[test]
fn ovl_mem_recreated_directory_shadows_old_real_children() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Tombstone every visible child under the real directory so it can be removed.
    call(&mut mt, &OsFunctionCall::Unlink("/mnt/subdir/nested.txt".into()))
        .unwrap()
        .unwrap();
    call(&mut mt, &OsFunctionCall::Unlink("/mnt/subdir/deep/file.txt".into()))
        .unwrap()
        .unwrap();
    call(&mut mt, &OsFunctionCall::Rmdir("/mnt/subdir/deep".into()))
        .unwrap()
        .unwrap();
    call(&mut mt, &OsFunctionCall::Rmdir("/mnt/subdir".into()))
        .unwrap()
        .unwrap();

    // Recreate the directory in the overlay. The old real children must stay hidden.
    call(&mut mt, &mkdir("/mnt/subdir", false, false)).unwrap().unwrap();

    let result = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt/subdir".into()));
    let names = sorted_names(&result);
    assert!(
        names.is_empty(),
        "recreated overlay dir should shadow old real children"
    );
}

#[test]
fn ovl_mem_mkdir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &mkdir("/mnt/overlay_dir", false, false))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::IsDir("/mnt/overlay_dir".into())),
        MontyObject::Bool(true)
    );
    // Host should not have the directory.
    assert!(!dir.path().join("overlay_dir").exists());
}

#[test]
fn ovl_mem_stat_overlay_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &write_text("/mnt/sized.txt", "12345".to_owned()))
        .unwrap()
        .unwrap();

    let stat = call_ok(&mut mt, &OsFunctionCall::Stat("/mnt/sized.txt".into()));
    match &stat {
        MontyObject::NamedTuple { values, .. } => {
            assert_eq!(values[6], MontyObject::Int(5), "st_size should be 5");
        }
        other => panic!("expected NamedTuple, got {other:?}"),
    }
}

#[test]
fn ovl_mem_rmdir_overlay() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &mkdir("/mnt/temp_dir", false, false)).unwrap().unwrap();
    call(&mut mt, &OsFunctionCall::Rmdir("/mnt/temp_dir".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/temp_dir".into())),
        MontyObject::Bool(false)
    );
}

#[test]
fn ovl_mem_rename() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &rename("/mnt/hello.txt", "/mnt/moved.txt"))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/moved.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
    // Host unchanged.
    assert!(dir.path().join("hello.txt").exists());
}

#[test]
fn ovl_mem_write_bytes() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &write_bytes("/mnt/bin_overlay.dat", vec![0xAA, 0xBB]))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/bin_overlay.dat".into())),
        MontyObject::Bytes(vec![0xAA, 0xBB])
    );
}

#[test]
fn ovl_mem_resolve() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Resolve("/mnt/subdir/../hello.txt".into())),
        MontyObject::Path("/mnt/hello.txt".to_owned())
    );
}

#[test]
fn ovl_mem_rename_directory() {
    // Renaming a directory must also move its descendants.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Rename subdir -> renamed_dir
    call(&mut mt, &rename("/mnt/subdir", "/mnt/renamed_dir"))
        .unwrap()
        .unwrap();

    // Old path should be gone.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir/nested.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir/deep/file.txt".into())),
        MontyObject::Bool(false)
    );

    // New path should have all descendants.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/renamed_dir".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/renamed_dir/nested.txt".into())),
        MontyObject::String("nested content".to_owned())
    );
    assert_eq!(
        call_ok(
            &mut mt,
            &OsFunctionCall::ReadText("/mnt/renamed_dir/deep/file.txt".into())
        ),
        MontyObject::String("deep file".to_owned())
    );

    // Host unchanged.
    assert!(dir.path().join("subdir/nested.txt").exists());
}

#[test]
fn ovl_mem_rename_directory_with_overlay_children() {
    // Directory rename must also move overlay-only children.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Add a new file in the overlay under subdir.
    call(
        &mut mt,
        &write_text("/mnt/subdir/overlay_file.txt", "overlay content".to_owned()),
    )
    .unwrap()
    .unwrap();

    call(&mut mt, &rename("/mnt/subdir", "/mnt/moved")).unwrap().unwrap();

    // Overlay-written file should appear under the new name.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/moved/overlay_file.txt".into())),
        MontyObject::String("overlay content".to_owned())
    );
    // Real-FS file should also appear.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/moved/nested.txt".into())),
        MontyObject::String("nested content".to_owned())
    );
}

#[test]
fn ovl_mem_write_missing_parent() {
    // write_text/write_bytes to a path with missing parent should fail,
    // matching CPython's FileNotFoundError behavior.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let err = call(&mut mt, &write_text("/mnt/nonexistent/child.txt", "x".to_owned()))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/nonexistent/child.txt'",
    );

    let err = call(&mut mt, &write_bytes("/mnt/nonexistent/child.bin", vec![0]))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/nonexistent/child.bin'",
    );
}

#[test]
fn ovl_mem_write_existing_parent() {
    // Writing to a path whose parent exists in the real FS should still work.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(
        &mut mt,
        &write_text("/mnt/subdir/new_file.txt", "new content".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/subdir/new_file.txt".into())),
        MontyObject::String("new content".to_owned())
    );
}

#[test]
fn ovl_mem_write_after_mkdir() {
    // Writing to a path whose parent was created via mkdir should work.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &mkdir("/mnt/newdir", false, false)).unwrap().unwrap();

    call(&mut mt, &write_text("/mnt/newdir/file.txt", "content".to_owned()))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/newdir/file.txt".into())),
        MontyObject::String("content".to_owned())
    );
}

// =============================================================================
// Overlay rename — exhaustive tests
// =============================================================================

#[test]
fn ovl_mem_rename_file_overwrites_existing_file() {
    // Renaming a file onto an existing file should overwrite the destination,
    // matching POSIX rename semantics.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Both are real FS files.
    call(&mut mt, &rename("/mnt/hello.txt", "/mnt/empty.txt"))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/empty.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn ovl_mem_rename_overlay_file_overwrites_overlay_file() {
    // Overwrite between two overlay-only files.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &write_text("/mnt/a.txt", "aaa".to_owned()))
        .unwrap()
        .unwrap();
    call(&mut mt, &write_text("/mnt/b.txt", "bbb".to_owned()))
        .unwrap()
        .unwrap();

    call(&mut mt, &rename("/mnt/a.txt", "/mnt/b.txt")).unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/a.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/b.txt".into())),
        MontyObject::String("aaa".to_owned())
    );
}

#[test]
fn ovl_mem_rename_to_same_path() {
    // Renaming a file to itself should be a no-op.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &rename("/mnt/hello.txt", "/mnt/hello.txt"))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/hello.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn ovl_mem_rename_deleted_file_fails() {
    // Renaming a tombstoned file should fail with not-found.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &OsFunctionCall::Unlink("/mnt/hello.txt".into()))
        .unwrap()
        .unwrap();
    let err = call(&mut mt, &rename("/mnt/hello.txt", "/mnt/other.txt"))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/hello.txt'",
    );
}

#[test]
fn ovl_mem_rename_nonexistent_file_fails() {
    // Renaming a file that never existed should fail.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let err = call(&mut mt, &rename("/mnt/no_such_file.txt", "/mnt/other.txt"))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/no_such_file.txt'",
    );
}

#[test]
fn ovl_mem_rename_into_nonexistent_parent_fails() {
    // Renaming into a path whose parent directory doesn't exist should fail.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let err = call(&mut mt, &rename("/mnt/hello.txt", "/mnt/no_such_dir/file.txt"))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/no_such_dir/file.txt'",
    );
}

#[test]
fn ovl_mem_rename_dir_with_tombstoned_entries() {
    // Renaming a directory that contains tombstoned entries should carry tombstones.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Delete a file inside subdir.
    call(&mut mt, &OsFunctionCall::Unlink("/mnt/subdir/nested.txt".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir/nested.txt".into())),
        MontyObject::Bool(false)
    );

    // Rename the directory.
    call(&mut mt, &rename("/mnt/subdir", "/mnt/moved")).unwrap().unwrap();

    // The tombstoned file should still be invisible under the new name.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/moved/nested.txt".into())),
        MontyObject::Bool(false)
    );
    // Other descendants should still be present.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/moved/deep/file.txt".into())),
        MontyObject::String("deep file".to_owned())
    );
}

#[test]
fn ovl_mem_rename_deeply_nested_overlay_dirs() {
    // Renaming a directory with multiple levels of overlay-only subdirectories.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &mkdir("/mnt/a", false, false)).unwrap().unwrap();
    call(&mut mt, &mkdir("/mnt/a/b", false, false)).unwrap().unwrap();
    call(&mut mt, &mkdir("/mnt/a/b/c", false, false)).unwrap().unwrap();
    call(&mut mt, &write_text("/mnt/a/b/c/leaf.txt", "leaf".to_owned()))
        .unwrap()
        .unwrap();

    call(&mut mt, &rename("/mnt/a", "/mnt/x")).unwrap().unwrap();

    // Old paths gone.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/a".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/a/b/c/leaf.txt".into())),
        MontyObject::Bool(false)
    );

    // New paths present.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/x/b/c".into())),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/x/b/c/leaf.txt".into())),
        MontyObject::String("leaf".to_owned())
    );
}

#[test]
fn ovl_mem_rename_then_rename_again() {
    // A file renamed once, then renamed again — both renames should work.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &rename("/mnt/hello.txt", "/mnt/step1.txt"))
        .unwrap()
        .unwrap();
    call(&mut mt, &rename("/mnt/step1.txt", "/mnt/step2.txt"))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/step1.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/step2.txt".into())),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn ovl_mem_rename_overlay_written_file() {
    // Rename a file that was created entirely in the overlay (never on real FS).
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &write_text("/mnt/new_file.txt", "overlay only".to_owned()))
        .unwrap()
        .unwrap();

    call(&mut mt, &rename("/mnt/new_file.txt", "/mnt/renamed_new.txt"))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/new_file.txt".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/renamed_new.txt".into())),
        MontyObject::String("overlay only".to_owned())
    );
}

#[test]
fn ovl_mem_rename_dir_iterdir_consistent() {
    // After renaming a directory, iterdir on both old and new parent should be correct.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &rename("/mnt/subdir", "/mnt/newdir")).unwrap().unwrap();

    // Old name should not appear in root listing.
    let root_listing = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt".into()));
    let root_names = sorted_names(&root_listing);
    assert!(!root_names.contains(&"subdir".to_owned()), "old name still in listing");
    assert!(
        root_names.contains(&"newdir".to_owned()),
        "new name missing from listing"
    );

    // New directory listing should contain the descendants.
    let new_listing = call_ok(&mut mt, &OsFunctionCall::Iterdir("/mnt/newdir".into()));
    let new_names = sorted_names(&new_listing);
    assert!(new_names.contains(&"nested.txt".to_owned()));
    assert!(new_names.contains(&"deep".to_owned()));
}

#[test]
fn ovl_mem_rename_dir_over_empty_overlay_dir() {
    // Renaming a directory onto an existing empty overlay directory should succeed.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &mkdir("/mnt/target_dir", false, false)).unwrap().unwrap();

    // Write a file into subdir overlay so we can verify it moves.
    call(&mut mt, &write_text("/mnt/subdir/extra.txt", "extra".to_owned()))
        .unwrap()
        .unwrap();

    call(&mut mt, &rename("/mnt/subdir", "/mnt/target_dir"))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/target_dir/extra.txt".into())),
        MontyObject::String("extra".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/target_dir/nested.txt".into())),
        MontyObject::String("nested content".to_owned())
    );
}

// =============================================================================
// Cross-cutting tests
// =============================================================================

#[test]
fn rename_cross_mount_error() {
    let dir1 = create_test_dir();
    let dir2 = create_test_dir();
    let mut mt = MountTable::new();
    mt.mount("/mnt1", dir1.path(), MountMode::ReadWrite, None).unwrap();
    mt.mount("/mnt2", dir2.path(), MountMode::ReadWrite, None).unwrap();

    let err = call(&mut mt, &rename("/mnt1/hello.txt", "/mnt2/hello.txt"))
        .unwrap()
        .unwrap_err()
        .into_exception();
    assert_exc(
        &err,
        ExcType::OSError,
        "[Errno 18] Invalid cross-device link: '/mnt1/hello.txt' -> '/mnt2/hello.txt'",
    );
}

#[test]
fn no_mount_point_returns_none() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call(&mut mt, &OsFunctionCall::Exists("/unmounted/file.txt".into()));
    assert!(result.is_none(), "expected None for path outside all mounts");
}

#[test]
fn empty_mount_table() {
    let mt = MountTable::new();
    assert!(mt.is_empty());
    assert_eq!(mt.len(), 0);
}

#[test]
fn mount_table_len() {
    let dir = create_test_dir();
    let mut mt = MountTable::new();
    mt.mount("/a", dir.path(), MountMode::ReadWrite, None).unwrap();
    mt.mount("/b", dir.path(), MountMode::ReadOnly, None).unwrap();
    assert_eq!(mt.len(), 2);
    assert!(!mt.is_empty());
}

#[test]
fn mount_sorting_specific_wins() {
    let dir = create_test_dir();
    let subdir = TempDir::new().unwrap();
    fs::write(subdir.path().join("specific.txt"), "from specific mount").unwrap();

    let mut mt = MountTable::new();
    mt.mount("/data", dir.path(), MountMode::ReadWrite, None).unwrap();
    mt.mount("/data/sub", subdir.path(), MountMode::ReadWrite, None)
        .unwrap();

    // /data/sub/specific.txt should come from the more specific mount.
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/data/sub/specific.txt".into())),
        MontyObject::String("from specific mount".to_owned())
    );
}

#[test]
fn non_filesystem_ops_not_handled() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = mt.handle_os_call(OsFunctionCall::Getenv(monty::GetenvArgs {
        key: "PATH".to_owned(),
        default: MontyObject::None,
    }));
    assert!(
        matches!(result, MountCallOutcome::NotHandled(OsFunctionCall::Getenv(_))),
        "non-filesystem ops should hand the call back"
    );
}

#[test]
fn mount_prefix_no_partial_match() {
    let dir = create_test_dir();
    let mut mt = MountTable::new();
    mt.mount("/data", dir.path(), MountMode::ReadWrite, None).unwrap();

    // /data2/file should NOT match /data mount.
    let result = call(&mut mt, &OsFunctionCall::Exists("/data2/file.txt".into()));
    assert!(result.is_none(), "expected None for path not matching any mount prefix");
}

#[test]
fn path_with_spaces() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("hello world.txt"), "spaces").unwrap();
    let mut mt = MountTable::new();
    mt.mount("/mnt", dir.path(), MountMode::ReadWrite, None).unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/hello world.txt".into())),
        MontyObject::String("spaces".to_owned())
    );
}

#[test]
fn path_with_unicode() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("文件.txt"), "unicode").unwrap();
    let mut mt = MountTable::new();
    mt.mount("/mnt", dir.path(), MountMode::ReadWrite, None).unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/文件.txt".into())),
        MontyObject::String("unicode".to_owned())
    );
}

#[test]
fn windows_style_paths_do_not_match_mounts() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Sandbox virtual paths are always POSIX-style, even on Windows hosts.
    assert!(
        call(&mut mt, &OsFunctionCall::Exists(r"\mnt\hello.txt".into())).is_none(),
        "backslash-only paths should not match a mount"
    );
    assert!(
        call(&mut mt, &OsFunctionCall::ReadText(r"C:\mnt\hello.txt".into())).is_none(),
        "drive-letter paths should not match a mount"
    );
    assert!(
        call(&mut mt, &OsFunctionCall::Resolve(r"/mnt\hello.txt".into())).is_none(),
        "mixed slash and backslash paths should not match a mount"
    );
}

#[test]
fn windows_style_write_paths_do_not_touch_host_mount() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call(&mut mt, &write_text(r"\mnt\created.txt", "should not be written"));
    assert!(
        result.is_none(),
        "windows-style write paths should be left unhandled by the mount table"
    );
    assert!(
        !dir.path().join("created.txt").exists(),
        "the host mount should not be modified for an unhandled windows-style path"
    );
}

// =============================================================================
// Mount memory usage limit
// =============================================================================

/// Creates a mount table with a small memory budget so tests can hit the
/// limit without large allocations.
fn mount_at_mnt_with_memory_limit(tmpdir: &TempDir, mode: MountMode, limit: u64) -> MountTable {
    let mount = Mount::new("/mnt", tmpdir.path(), mode, None)
        .unwrap()
        .with_memory_usage_limit(limit);
    let mut table = MountTable::new();
    table.push_mount(mount);
    table
}

#[test]
fn mount_memory_usage_limit_defaults_to_100_mb() {
    let dir = create_test_dir();
    let mount = Mount::new("/mnt", dir.path(), MountMode::ReadOnly, None).unwrap();

    assert_eq!(DEFAULT_MEMORY_USAGE_LIMIT, 100_000_000);
    assert_eq!(mount.memory_usage_limit(), DEFAULT_MEMORY_USAGE_LIMIT);
    assert_eq!(mount.memory_usage(), 0);
}

#[test]
fn direct_reads_accept_exact_limit_and_reject_one_byte_over() {
    let dir = create_test_dir();
    fs::write(dir.path().join("exact.txt"), b"12345").unwrap();
    fs::write(dir.path().join("large.txt"), b"123456").unwrap();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::ReadOnly, 5);

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/exact.txt".into())),
        MontyObject::Bytes(b"12345".to_vec())
    );
    let exc = call_err(&mut mt, &OsFunctionCall::ReadBytes("/mnt/large.txt".into()));
    assert_exc(
        &exc,
        ExcType::MemoryError,
        "mount memory usage limit of 5 bytes exceeded",
    );
    let exc = call_err(&mut mt, &OsFunctionCall::ReadText("/mnt/large.txt".into()));
    assert_exc(
        &exc,
        ExcType::MemoryError,
        "mount memory usage limit of 5 bytes exceeded",
    );
}

#[test]
fn directory_results_obey_mount_memory_budget() {
    let dir = create_test_dir();
    fs::write(dir.path().join("one"), b"").unwrap();
    fs::write(dir.path().join("two"), b"").unwrap();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::ReadOnly, 100);

    let exc = call_err(&mut mt, &OsFunctionCall::Iterdir("/mnt".into()));
    assert_exc(
        &exc,
        ExcType::MemoryError,
        "mount memory usage limit of 100 bytes exceeded",
    );
}

#[test]
fn overlay_retained_data_and_reads_share_one_memory_budget() {
    let dir = create_test_dir();
    fs::write(dir.path().join("large.bin"), vec![b'x'; 800]).unwrap();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_000);

    call(&mut mt, &write_bytes("/mnt/overlay.bin", vec![b'a'; 500]))
        .unwrap()
        .unwrap();
    let exc = call_err(&mut mt, &OsFunctionCall::ReadBytes("/mnt/overlay.bin".into()));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 1 KB exceeded");

    let exc = call_err(&mut mt, &append_bytes("/mnt/large.bin", b"7".to_vec()));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 1 KB exceeded");
    assert_eq!(fs::read(dir.path().join("large.bin")).unwrap(), vec![b'x'; 800]);
}

#[test]
fn separate_overlay_files_share_memory_budget() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_000);

    call_ok(&mut mt, &write_bytes("/mnt/one.bin", vec![b'a'; 300]));
    let exc = call_err(&mut mt, &write_bytes("/mnt/two.bin", vec![b'b'; 300]));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 1 KB exceeded");
}

/// Replacing an overlay file must release the old entry's usage first: a
/// same-path replacement larger than the original fits, while a *new* file of
/// the replacement's size does not.
#[test]
fn overwriting_an_overlay_file_reuses_its_budget() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 2_000);

    call_ok(&mut mt, &write_bytes("/mnt/f.bin", vec![b'a'; 1_200]));
    let exc = call_err(&mut mt, &write_bytes("/mnt/g.bin", vec![b'b'; 1_200]));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 2 KB exceeded");

    // the replacement releases the 1200 retained bytes, so 1400 fits...
    // (write_text so the text path's replacement accounting is covered too)
    call_ok(&mut mt, &write_text("/mnt/f.bin", "a".repeat(1_400)));
    // ...while a second 1400-byte file alongside it does not
    let exc = call_err(&mut mt, &write_bytes("/mnt/h.bin", vec![b'c'; 1_400]));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 2 KB exceeded");
}

/// In-place appends to an existing overlay file are charged against the
/// budget, and a rejected append leaves the content untouched.
#[test]
fn in_place_append_obeys_memory_budget() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 3_000);

    call_ok(&mut mt, &write_bytes("/mnt/a.bin", vec![b'a'; 400]));
    call_ok(&mut mt, &append_bytes("/mnt/a.bin", vec![b'b'; 200]));
    let mut expected = vec![b'a'; 400];
    expected.extend(vec![b'b'; 200]);
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/a.bin".into())),
        MontyObject::Bytes(expected.clone())
    );

    let exc = call_err(&mut mt, &append_bytes("/mnt/a.bin", vec![b'c'; 3_000]));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 3 KB exceeded");
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/a.bin".into())),
        MontyObject::Bytes(expected)
    );
}

/// Deleting an overlay file swaps it for a small tombstone, freeing budget
/// for later writes.
#[test]
fn deleting_an_overlay_file_frees_budget() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_600);

    call_ok(&mut mt, &write_bytes("/mnt/big.bin", vec![b'a'; 600]));
    let exc = call_err(&mut mt, &write_bytes("/mnt/b2.bin", vec![b'b'; 600]));
    assert_exc(
        &exc,
        ExcType::MemoryError,
        "mount memory usage limit of 1.6 KB exceeded",
    );

    call_ok(&mut mt, &OsFunctionCall::Unlink("/mnt/big.bin".into()));
    call_ok(&mut mt, &write_bytes("/mnt/b2.bin", vec![b'b'; 600]));
}

/// Deleting a *real* mounted file records an in-memory tombstone, so even
/// `unlink` raises `MemoryError` once the budget is exhausted.
#[test]
fn tombstones_are_charged_to_the_memory_budget() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_000);

    call_ok(&mut mt, &write_bytes("/mnt/x.bin", vec![b'a'; 600]));
    let exc = call_err(&mut mt, &OsFunctionCall::Unlink("/mnt/hello.txt".into()));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 1 KB exceeded");
}

/// Overlay `iterdir` results are transient allocations charged against the
/// budget remaining after the retained entries themselves.
#[test]
fn overlay_iterdir_obeys_memory_budget() {
    let dir = TempDir::new().unwrap();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 4_500);

    for i in 0..10 {
        call_ok(&mut mt, &write_bytes(&format!("/mnt/f{i}"), vec![b'x']));
    }
    let exc = call_err(&mut mt, &OsFunctionCall::Iterdir("/mnt".into()));
    assert_exc(
        &exc,
        ExcType::MemoryError,
        "mount memory usage limit of 4.5 KB exceeded",
    );
}

/// A fall-through read of a real mounted file only gets the budget left over
/// after retained overlay data.
#[test]
fn fall_through_reads_share_budget_with_retained_data() {
    let dir = create_test_dir();
    fs::write(dir.path().join("large.bin"), vec![b'x'; 800]).unwrap();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_000);

    // with nothing retained the 800-byte host file fits...
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/large.bin".into())),
        MontyObject::Bytes(vec![b'x'; 800])
    );
    // ...but after retaining overlay data it no longer does
    call_ok(&mut mt, &write_bytes("/mnt/keep.bin", vec![b'k'; 100]));
    let exc = call_err(&mut mt, &OsFunctionCall::ReadBytes("/mnt/large.bin".into()));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 1 KB exceeded");
}

/// Renaming an overlay directory tombstones every source entry, and the whole
/// batch is checked atomically: it fits within a loose budget and is rejected
/// under a tight one before any entry moves.
#[test]
fn overlay_directory_rename_obeys_memory_budget() {
    // loose budget: the rename succeeds and the tree is intact at the new path
    let dir = TempDir::new().unwrap();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 10_000);
    call_ok(&mut mt, &mkdir("/mnt/d", false, false));
    for i in 0..8 {
        call_ok(&mut mt, &write_bytes(&format!("/mnt/d/f{i}"), vec![b'x']));
    }
    call_ok(&mut mt, &rename("/mnt/d", "/mnt/e"));
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/d".into())),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/e/f0".into())),
        MontyObject::Bytes(vec![b'x'])
    );

    // tight budget: the added tombstones and destination entries do not fit,
    // and the atomic preflight leaves the source untouched
    let dir = TempDir::new().unwrap();
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 5_000);
    call_ok(&mut mt, &mkdir("/mnt/d", false, false));
    for i in 0..8 {
        call_ok(&mut mt, &write_bytes(&format!("/mnt/d/f{i}"), vec![b'x']));
    }
    let exc = call_err(&mut mt, &rename("/mnt/d", "/mnt/e"));
    assert_exc(&exc, ExcType::MemoryError, "mount memory usage limit of 5 KB exceeded");
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/d/f0".into())),
        MontyObject::Bytes(vec![b'x'])
    );
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/e".into())),
        MontyObject::Bool(false)
    );
}

/// Renaming a real mounted directory captures its descendants into the
/// overlay; that capture is bounded by the memory budget.
#[test]
fn real_directory_rename_capture_obeys_memory_budget() {
    let dir = create_test_dir();
    // 1500 is below the fixed per-entry capture charge for subdir's three
    // descendants alone, so the capture must be rejected regardless of
    // host path lengths
    let mut mt = mount_at_mnt_with_memory_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_500);

    let exc = call_err(&mut mt, &rename("/mnt/subdir", "/mnt/moved"));
    assert_exc(
        &exc,
        ExcType::MemoryError,
        "mount memory usage limit of 1.5 KB exceeded",
    );
    // the source is untouched in the overlay and on the host
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir/nested.txt".into())),
        MontyObject::Bool(true)
    );
    assert!(dir.path().join("subdir/nested.txt").is_file());
}

// =============================================================================
// Write bytes limit
// =============================================================================

/// Helper: creates a mount table with a write bytes limit.
fn mount_at_mnt_with_limit(tmpdir: &TempDir, mode: MountMode, limit: u64) -> MountTable {
    let mut mt = MountTable::new();
    mt.mount("/mnt", tmpdir.path(), mode, Some(limit)).unwrap();
    mt
}

#[test]
fn rw_write_text_within_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 100);

    // "hello" is 5 bytes, well within the 100-byte limit.
    call(&mut mt, &write_text("/mnt/a.txt", "hello".to_owned()))
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/a.txt".into())),
        MontyObject::String("hello".to_owned())
    );
}

#[test]
fn rw_write_text_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 10);

    // 20 bytes exceeds the 10-byte limit.
    let exc = call(&mut mt, &write_text("/mnt/a.txt", "a]".repeat(10)))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 10 bytes exceeded");
}

#[test]
fn rw_write_bytes_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 5);

    let exc = call(&mut mt, &write_bytes("/mnt/a.bin", vec![0u8; 10]))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 5 bytes exceeded");
}

#[test]
fn rw_cumulative_writes_exceed_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 15);

    // First write: 10 bytes, within limit.
    call(&mut mt, &write_text("/mnt/a.txt", "0123456789".to_owned()))
        .unwrap()
        .unwrap();

    // Second write: 10 more bytes, cumulative 20 > 15 limit.
    let exc = call(&mut mt, &write_text("/mnt/b.txt", "0123456789".to_owned()))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 15 bytes exceeded");
}

#[test]
fn ovl_write_text_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 10);

    let exc = call(&mut mt, &write_text("/mnt/a.txt", "a]".repeat(10)))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 10 bytes exceeded");
}

#[test]
fn ovl_write_bytes_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 5);

    let exc = call(&mut mt, &write_bytes("/mnt/a.bin", vec![0u8; 10]))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 5 bytes exceeded");
}

#[test]
fn ovl_cumulative_writes_exceed_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 15);

    // First write: 10 bytes, within limit.
    call(&mut mt, &write_text("/mnt/a.txt", "0123456789".to_owned()))
        .unwrap()
        .unwrap();

    // Second write: 10 more bytes, cumulative 20 > 15 limit.
    let exc = call(&mut mt, &write_text("/mnt/b.txt", "0123456789".to_owned()))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 15 bytes exceeded");
}

#[test]
fn ovl_append_existing_real_file_counts_existing_bytes_toward_limit() {
    let dir = create_test_dir();
    fs::write(dir.path().join("large.bin"), vec![0u8; 10]).unwrap();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 5);

    let exc = call(&mut mt, &append_bytes("/mnt/large.bin", vec![1u8]))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 5 bytes exceeded");
    assert_eq!(
        call_ok(&mut mt, &OsFunctionCall::ReadBytes("/mnt/large.bin".into())),
        MontyObject::Bytes(vec![0u8; 10]),
        "failed append should leave the real backing file visible and unchanged"
    );

    call(&mut mt, &write_bytes("/mnt/quota_ok.bin", vec![1u8; 5]))
        .unwrap()
        .unwrap();
}

#[test]
fn write_limit_pretty_format_kb() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 5_000);

    let exc = call(&mut mt, &write_text("/mnt/a.txt", "x".repeat(5_001)))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 5 KB exceeded");
}

#[test]
fn write_limit_pretty_format_mb() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_500_000);

    let exc = call(&mut mt, &write_text("/mnt/a.txt", "x".repeat(1_500_001)))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 1.5 MB exceeded");
}

#[test]
fn write_exactly_at_limit_succeeds() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 10);

    // Exactly 10 bytes with a 10-byte limit should succeed.
    call(&mut mt, &write_text("/mnt/a.txt", "0123456789".to_owned()))
        .unwrap()
        .unwrap();
}

#[test]
fn write_one_over_limit_fails() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 10);

    // 11 bytes with a 10-byte limit should fail.
    let exc = call(&mut mt, &write_text("/mnt/a.txt", "01234567890".to_owned()))
        .unwrap()
        .expect_err("expected write limit error")
        .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 10 bytes exceeded");
}

#[test]
fn no_limit_allows_large_writes() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Without a limit, large writes should succeed.
    call(&mut mt, &write_text("/mnt/big.txt", "x".repeat(100_000)))
        .unwrap()
        .unwrap();
}

// =============================================================================
// Unlink and rename operate on symlink entries, not targets (Issue #3)
// =============================================================================

/// `unlink()` on a symlink should remove the symlink entry, not the target.
#[test]
#[cfg(unix)]
fn rw_unlink_symlink_removes_link_not_target() {
    let dir = create_test_dir();
    symlink_file(dir.path().join("hello.txt"), dir.path().join("link.txt"));

    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    call_ok(&mut mt, &OsFunctionCall::Unlink("/mnt/link.txt".into()));

    // The symlink should be gone.
    assert!(!dir.path().join("link.txt").exists());
    assert!(dir.path().join("link.txt").symlink_metadata().is_err());
    // The target should still exist.
    assert!(dir.path().join("hello.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
        "hello world\n"
    );
}

/// `rename()` on a symlink should move the symlink entry, not the target.
#[test]
#[cfg(unix)]
fn rw_rename_symlink_renames_link_not_target() {
    let dir = create_test_dir();
    symlink_file(dir.path().join("hello.txt"), dir.path().join("link.txt"));

    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    call(&mut mt, &rename("/mnt/link.txt", "/mnt/moved_link.txt"))
        .unwrap()
        .unwrap();

    // The old symlink should be gone, the new one should exist as a symlink.
    assert!(dir.path().join("link.txt").symlink_metadata().is_err());
    assert!(
        dir.path()
            .join("moved_link.txt")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // The original target should still exist and be unchanged.
    assert!(dir.path().join("hello.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
        "hello world\n"
    );
}

// =============================================================================
// Failed writes should not consume write quota (Issue #4)
// =============================================================================

/// A failed write (e.g. parent doesn't exist) must not burn quota.
#[test]
fn rw_failed_write_does_not_consume_quota() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 10);

    // Write to a nonexistent parent — this should fail.
    let result = call(&mut mt, &write_text("/mnt/no_such_dir/file.txt", "12345".to_owned()));
    assert!(result.unwrap().is_err());

    // Now write exactly at the limit — should succeed since the failed write
    // didn't consume any quota.
    call(&mut mt, &write_text("/mnt/quota_ok.txt", "0123456789".to_owned()))
        .unwrap()
        .unwrap();
}

/// Same quota-preservation test for overlay mode.
#[test]
fn ovl_failed_write_does_not_consume_quota() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 10);

    // Write to a path whose parent doesn't exist — should fail.
    let result = call(&mut mt, &write_text("/mnt/no_such_dir/file.txt", "12345".to_owned()));
    assert!(result.unwrap().is_err());

    // Valid write of exactly 10 bytes should succeed.
    call(&mut mt, &write_text("/mnt/quota_ok.txt", "0123456789".to_owned()))
        .unwrap()
        .unwrap();
}

// =============================================================================
// Overlay rename preserves access to descendants (Issue #7)
// =============================================================================

/// Renaming a directory in overlay mode must make all descendants accessible
/// under the new prefix.
#[test]
fn ovl_rename_directory_preserves_descendants() {
    let dir = create_test_dir();
    // test_dir has subdir/nested.txt and subdir/deep/file.txt

    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, &rename("/mnt/subdir", "/mnt/renamed_dir"))
        .unwrap()
        .unwrap();

    // Descendants should be accessible under the new prefix.
    let result = call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/renamed_dir/nested.txt".into()));
    assert_eq!(result, MontyObject::String("nested content".to_owned()));

    let result = call_ok(
        &mut mt,
        &OsFunctionCall::ReadText("/mnt/renamed_dir/deep/file.txt".into()),
    );
    assert_eq!(result, MontyObject::String("deep file".to_owned()));

    // Old paths should not exist.
    let result = call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/subdir/nested.txt".into()));
    assert_eq!(result, MontyObject::Bool(false));
}

// =============================================================================
// Overlay rename: destination type validation
// =============================================================================

/// Renaming a file onto an existing directory should raise IsADirectoryError.
#[test]
fn ovl_mem_rename_file_onto_directory() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let result = call(&mut mt, &rename("/mnt/hello.txt", "/mnt/subdir"));
    let exc = result.unwrap().unwrap_err().into_exception();
    assert_eq!(exc.exc_type(), ExcType::IsADirectoryError);
}

/// Renaming a directory onto an existing file should raise NotADirectoryError.
#[test]
fn ovl_mem_rename_directory_onto_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let result = call(&mut mt, &rename("/mnt/subdir", "/mnt/hello.txt"));
    let exc = result.unwrap().unwrap_err().into_exception();
    assert_eq!(exc.exc_type(), ExcType::NotADirectoryError);
}

/// Renaming a directory into its own descendant should raise OSError.
#[test]
fn ovl_mem_rename_directory_into_own_subdir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let result = call(&mut mt, &rename("/mnt/subdir", "/mnt/subdir/deep/moved"));
    let exc = result.unwrap().unwrap_err().into_exception();
    assert_eq!(exc.exc_type(), ExcType::OSError);
    assert!(
        exc.message().unwrap_or("").contains("Invalid argument"),
        "expected 'Invalid argument', got: {:?}",
        exc.message()
    );
}

/// Renaming an overlay file onto an overlay directory should raise IsADirectoryError.
#[test]
fn ovl_mem_rename_overlay_file_onto_overlay_dir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Create an overlay file and directory
    call(&mut mt, &write_text("/mnt/src.txt", "content".to_owned()))
        .unwrap()
        .unwrap();
    call(&mut mt, &mkdir("/mnt/dst_dir", false, false)).unwrap().unwrap();

    let result = call(&mut mt, &rename("/mnt/src.txt", "/mnt/dst_dir"));
    let exc = result.unwrap().unwrap_err().into_exception();
    assert_eq!(exc.exc_type(), ExcType::IsADirectoryError);
}

// =============================================================================
// Overlay rename: symlink preservation
// =============================================================================

/// Renaming a real symlink in overlay mode should preserve its symlink identity.
#[test]
#[cfg(unix)]
fn ovl_mem_rename_symlink_preserves_symlink() {
    let dir = create_test_dir();
    symlink_file(dir.path().join("hello.txt"), dir.path().join("link.txt"));

    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Before rename: link should be a symlink
    let result = call_ok(&mut mt, &OsFunctionCall::IsSymlink("/mnt/link.txt".into()));
    assert_eq!(result, MontyObject::Bool(true));

    // Rename the symlink
    call(&mut mt, &rename("/mnt/link.txt", "/mnt/moved_link.txt"))
        .unwrap()
        .unwrap();

    // After rename: the moved path should still be readable (via the stored host ref)
    let result = call_ok(&mut mt, &OsFunctionCall::ReadText("/mnt/moved_link.txt".into()));
    assert_eq!(result, MontyObject::String("hello world\n".to_owned()));

    // Original symlink path should be gone
    let result = call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/link.txt".into()));
    assert_eq!(result, MontyObject::Bool(false));

    // Original target should still exist
    let result = call_ok(&mut mt, &OsFunctionCall::Exists("/mnt/hello.txt".into()));
    assert_eq!(result, MontyObject::Bool(true));
}

// =============================================================================
// Overlay rmdir: must check overlay children on real directories
// =============================================================================

/// rmdir on a real directory must fail if it has overlay-only children.
#[test]
fn ovl_mem_rmdir_real_dir_with_overlay_children() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Delete the real child via tombstone
    call(&mut mt, &OsFunctionCall::Unlink("/mnt/subdir/nested.txt".into()))
        .unwrap()
        .unwrap();
    call(&mut mt, &OsFunctionCall::Unlink("/mnt/subdir/deep/file.txt".into()))
        .unwrap()
        .unwrap();
    call(&mut mt, &OsFunctionCall::Rmdir("/mnt/subdir/deep".into()))
        .unwrap()
        .unwrap();

    // Add an overlay-only child
    call(
        &mut mt,
        &write_text("/mnt/subdir/overlay_only.txt", "overlay".to_owned()),
    )
    .unwrap()
    .unwrap();

    // rmdir should fail because of the overlay child
    let exc = call_err(&mut mt, &OsFunctionCall::Rmdir("/mnt/subdir".into()));
    assert_eq!(exc.exc_type(), ExcType::OSError);
    assert!(
        exc.message().unwrap_or("").contains("Directory not empty"),
        "expected 'Directory not empty', got: {:?}",
        exc.message()
    );

    // The overlay child should still be accessible
    let result = call_ok(
        &mut mt,
        &OsFunctionCall::ReadText("/mnt/subdir/overlay_only.txt".into()),
    );
    assert_eq!(result, MontyObject::String("overlay".to_owned()));
}

// =============================================================================
// on_no_handler error message format
// =============================================================================

/// `on_no_handler` for filesystem ops should not include `Errno` prefix.
#[test]
fn on_no_handler_includes_errno() {
    let exc = OsFunctionCall::Exists("/outside".into()).on_no_handler();
    assert_eq!(exc.exc_type(), ExcType::PermissionError);
    assert_eq!(exc.message().unwrap_or(""), "Permission denied: '/outside'");
}
