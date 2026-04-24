//! Integration tests for filesystem mount operations.
//!
//! Tests `MountTable::handle_os_call()` across all supported mount modes (ReadWrite,
//! ReadOnly, OverlayMemory) and all supported filesystem
//! operations. Uses real temporary directories to verify correct behavior.

#[cfg(unix)]
use std::os::unix::fs::symlink as unix_symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as win_symlink_file;
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use monty::{
    ExcType, MontyException, MontyObject, OsFunction,
    fs::{Mount, MountError, MountMode, MountTable, OverlayState},
};
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

/// Shorthand: call handle_os_call with a single path argument.
fn call(mt: &mut MountTable, func: OsFunction, path: &str) -> Option<Result<MontyObject, MountError>> {
    mt.handle_os_call(func, &[MontyObject::Path(path.to_owned())], &[])
}

/// Shorthand: call and unwrap both the Option and Result.
fn call_ok(mt: &mut MountTable, func: OsFunction, path: &str) -> MontyObject {
    call(mt, func, path).expect("expected Some").expect("expected Ok")
}

/// Shorthand: call and unwrap Option, expect Err, convert to exception.
fn call_err(mt: &mut MountTable, func: OsFunction, path: &str) -> MontyException {
    call(mt, func, path)
        .expect("expected Some")
        .expect_err("expected Err")
        .into_exception()
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

/// Shorthand for write operations that take path + content args.
fn call_write(
    mt: &mut MountTable,
    func: OsFunction,
    path: &str,
    content: MontyObject,
) -> Option<Result<MontyObject, MountError>> {
    mt.handle_os_call(func, &[MontyObject::Path(path.to_owned()), content], &[])
}

/// Shorthand for mkdir with kwargs.
fn call_mkdir(
    mt: &mut MountTable,
    path: &str,
    parents: bool,
    exist_ok: bool,
) -> Option<Result<MontyObject, MountError>> {
    mt.handle_os_call(
        OsFunction::Mkdir,
        &[MontyObject::Path(path.to_owned())],
        &[
            (MontyObject::String("parents".to_owned()), MontyObject::Bool(parents)),
            (MontyObject::String("exist_ok".to_owned()), MontyObject::Bool(exist_ok)),
        ],
    )
}

/// Shorthand for rename.
fn call_rename(mt: &mut MountTable, src: &str, dst: &str) -> Option<Result<MontyObject, MountError>> {
    mt.handle_os_call(
        OsFunction::Rename,
        &[MontyObject::Path(src.to_owned()), MontyObject::Path(dst.to_owned())],
        &[],
    )
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
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/subdir"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/nonexistent"),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_is_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, OsFunction::IsFile, "/mnt/hello.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsFile, "/mnt/subdir"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsFile, "/mnt/nonexistent"),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_is_dir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/subdir"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/subdir/deep"),
        MontyObject::Bool(true)
    );
}

#[test]
fn rw_is_symlink() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, OsFunction::IsSymlink, "/mnt/hello.txt"),
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
        call_ok(&mut mt, OsFunction::IsSymlink, "/mnt/link.txt"),
        MontyObject::Bool(true)
    );
    // Target file should NOT be detected as a symlink
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsSymlink, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
    // Nonexistent path should return false
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsSymlink, "/mnt/nope.txt"),
        MontyObject::Bool(false)
    );
}

#[test]
fn overlay_is_symlink_true_for_symlink() {
    let dir = create_test_dir();
    symlink_file(dir.path().join("hello.txt"), dir.path().join("link.txt"));
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    assert_eq!(
        call_ok(&mut mt, OsFunction::IsSymlink, "/mnt/link.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsSymlink, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_read_text() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/hello.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/empty.txt"),
        MontyObject::String(String::new())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/subdir/nested.txt"),
        MontyObject::String("nested content".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/subdir/deep/file.txt"),
        MontyObject::String("deep file".to_owned())
    );
}

#[test]
fn rw_read_text_not_found() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_err(&mut mt, OsFunction::ReadText, "/mnt/nonexistent.txt");
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
        call_ok(&mut mt, OsFunction::ReadBytes, "/mnt/data.bin"),
        MontyObject::Bytes(vec![0x00, 0x01, 0x02, 0x03])
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadBytes, "/mnt/empty.txt"),
        MontyObject::Bytes(vec![])
    );
}

#[test]
fn rw_write_text_and_read_back() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/new_file.txt",
        MontyObject::String("new content".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/new_file.txt"),
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

    call_write(
        &mut mt,
        OsFunction::WriteBytes,
        "/mnt/out.bin",
        MontyObject::Bytes(vec![0xff, 0xfe]),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadBytes, "/mnt/out.bin"),
        MontyObject::Bytes(vec![0xff, 0xfe])
    );
}

#[test]
fn rw_overwrite_existing() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/hello.txt",
        MontyObject::String("overwritten".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/hello.txt"),
        MontyObject::String("overwritten".to_owned())
    );
}

#[test]
fn rw_stat_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let stat = call_ok(&mut mt, OsFunction::Stat, "/mnt/hello.txt");
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

    let stat = call_ok(&mut mt, OsFunction::Stat, "/mnt/subdir");
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

    let result = call_ok(&mut mt, OsFunction::Iterdir, "/mnt");
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

    let result = call_ok(&mut mt, OsFunction::Iterdir, "/mnt/subdir");
    let names = sorted_names(&result);
    assert_eq!(names, vec!["deep", "nested.txt"]);
}

#[test]
fn rw_mkdir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call_mkdir(&mut mt, "/mnt/new_dir", false, false).unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/new_dir"),
        MontyObject::Bool(true)
    );
    assert!(dir.path().join("new_dir").is_dir());
}

#[test]
fn rw_mkdir_parents() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call_mkdir(&mut mt, "/mnt/a/b/c", true, false).unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/a/b/c"),
        MontyObject::Bool(true)
    );
}

#[test]
fn rw_mkdir_exist_ok() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call_mkdir(&mut mt, "/mnt/subdir", false, true).unwrap().unwrap();
}

#[test]
#[cfg(not(target_os = "windows"))]
fn rw_mkdir_already_exists_error() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_mkdir(&mut mt, "/mnt/subdir", false, false)
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
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(true)
    );
    call(&mut mt, OsFunction::Unlink, "/mnt/hello.txt").unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
    assert!(!dir.path().join("hello.txt").exists());
}

#[test]
fn rw_unlink_not_found() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_err(&mut mt, OsFunction::Unlink, "/mnt/nonexistent.txt");
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

    call_mkdir(&mut mt, "/mnt/empty_dir", false, false).unwrap().unwrap();
    call(&mut mt, OsFunction::Rmdir, "/mnt/empty_dir").unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/empty_dir"),
        MontyObject::Bool(false)
    );
}

#[test]
fn rw_rename() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    call_rename(&mut mt, "/mnt/hello.txt", "/mnt/renamed.txt")
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/renamed.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn rw_resolve() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, OsFunction::Resolve, "/mnt/subdir/../hello.txt"),
        MontyObject::Path("/mnt/hello.txt".to_owned())
    );
}

#[test]
fn rw_absolute() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call_ok(&mut mt, OsFunction::Absolute, "/mnt/./subdir"),
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
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsFile, "/mnt/hello.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/subdir"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/hello.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadBytes, "/mnt/data.bin"),
        MontyObject::Bytes(vec![0x00, 0x01, 0x02, 0x03])
    );

    // stat and iterdir should work
    let _stat = call_ok(&mut mt, OsFunction::Stat, "/mnt/hello.txt");
    let _entries = call_ok(&mut mt, OsFunction::Iterdir, "/mnt");
}

#[test]
fn ro_write_text_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/new.txt",
        MontyObject::String("blocked".to_owned()),
    )
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

    let err = call_write(
        &mut mt,
        OsFunction::WriteBytes,
        "/mnt/new.bin",
        MontyObject::Bytes(vec![0x00]),
    )
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

    let err = call_mkdir(&mut mt, "/mnt/newdir", false, false)
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

    let err = call_err(&mut mt, OsFunction::Unlink, "/mnt/hello.txt");
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

    let err = call_err(&mut mt, OsFunction::Rmdir, "/mnt/subdir");
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

    let err = call_rename(&mut mt, "/mnt/hello.txt", "/mnt/renamed.txt")
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
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/hello.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadBytes, "/mnt/data.bin"),
        MontyObject::Bytes(vec![0x00, 0x01, 0x02, 0x03])
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/subdir"),
        MontyObject::Bool(true)
    );
}

#[test]
fn ovl_mem_write_readable_back() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/new_overlay.txt",
        MontyObject::String("overlay content".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/new_overlay.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/new_overlay.txt"),
        MontyObject::String("overlay content".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsFile, "/mnt/new_overlay.txt"),
        MontyObject::Bool(true)
    );
}

#[test]
fn ovl_mem_write_does_not_modify_host() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/hello.txt",
        MontyObject::String("overlay overwrite".to_owned()),
    )
    .unwrap()
    .unwrap();

    // Overlay returns the new content.
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/hello.txt"),
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
    call(&mut mt, OsFunction::Unlink, "/mnt/hello.txt").unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/overlay_new.txt",
        MontyObject::String("new".to_owned()),
    )
    .unwrap()
    .unwrap();

    let result = call_ok(&mut mt, OsFunction::Iterdir, "/mnt");
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

    call(&mut mt, OsFunction::Unlink, "/mnt/hello.txt").unwrap().unwrap();

    let result = call_ok(&mut mt, OsFunction::Iterdir, "/mnt");
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

    let err = call_err(&mut mt, OsFunction::Iterdir, "/mnt/no_such_dir");
    assert_eq!(err.exc_type(), ExcType::FileNotFoundError);
}

#[test]
fn ovl_mem_iterdir_file_errors() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let err = call_err(&mut mt, OsFunction::Iterdir, "/mnt/hello.txt");
    assert_eq!(err.exc_type(), ExcType::NotADirectoryError);
}

#[test]
fn ovl_mem_path_component_too_long() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // 256-byte component exceeds NAME_MAX (255)
    let long_name = "a".repeat(256);
    let path = format!("/mnt/{long_name}");

    let err = call_err(&mut mt, OsFunction::Exists, &path);
    assert_exc(
        &err,
        ExcType::OSError,
        &format!("[Errno 36] File name too long: '{path}'"),
    );

    // 255-byte component is fine
    let ok_name = "b".repeat(255);
    let ok_path = format!("/mnt/{ok_name}");
    call(&mut mt, OsFunction::Exists, &ok_path)
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

    let err = call_err(&mut mt, OsFunction::Exists, &path);
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

    let err = call_err(&mut mt, OsFunction::Stat, &path);
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
    call(&mut mt, OsFunction::Unlink, "/mnt/subdir/nested.txt")
        .unwrap()
        .unwrap();
    call(&mut mt, OsFunction::Unlink, "/mnt/subdir/deep/file.txt")
        .unwrap()
        .unwrap();
    call(&mut mt, OsFunction::Rmdir, "/mnt/subdir/deep").unwrap().unwrap();
    call(&mut mt, OsFunction::Rmdir, "/mnt/subdir").unwrap().unwrap();

    // Recreate the directory in the overlay. The old real children must stay hidden.
    call_mkdir(&mut mt, "/mnt/subdir", false, false).unwrap().unwrap();

    let result = call_ok(&mut mt, OsFunction::Iterdir, "/mnt/subdir");
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

    call_mkdir(&mut mt, "/mnt/overlay_dir", false, false).unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::IsDir, "/mnt/overlay_dir"),
        MontyObject::Bool(true)
    );
    // Host should not have the directory.
    assert!(!dir.path().join("overlay_dir").exists());
}

#[test]
fn ovl_mem_stat_overlay_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/sized.txt",
        MontyObject::String("12345".to_owned()),
    )
    .unwrap()
    .unwrap();

    let stat = call_ok(&mut mt, OsFunction::Stat, "/mnt/sized.txt");
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

    call_mkdir(&mut mt, "/mnt/temp_dir", false, false).unwrap().unwrap();
    call(&mut mt, OsFunction::Rmdir, "/mnt/temp_dir").unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/temp_dir"),
        MontyObject::Bool(false)
    );
}

#[test]
fn ovl_mem_rename() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_rename(&mut mt, "/mnt/hello.txt", "/mnt/moved.txt")
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/moved.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
    // Host unchanged.
    assert!(dir.path().join("hello.txt").exists());
}

#[test]
fn ovl_mem_write_bytes() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_write(
        &mut mt,
        OsFunction::WriteBytes,
        "/mnt/bin_overlay.dat",
        MontyObject::Bytes(vec![0xAA, 0xBB]),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadBytes, "/mnt/bin_overlay.dat"),
        MontyObject::Bytes(vec![0xAA, 0xBB])
    );
}

#[test]
fn ovl_mem_resolve() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    assert_eq!(
        call_ok(&mut mt, OsFunction::Resolve, "/mnt/subdir/../hello.txt"),
        MontyObject::Path("/mnt/hello.txt".to_owned())
    );
}

#[test]
fn ovl_mem_rename_directory() {
    // Renaming a directory must also move its descendants.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    // Rename subdir -> renamed_dir
    call_rename(&mut mt, "/mnt/subdir", "/mnt/renamed_dir")
        .unwrap()
        .unwrap();

    // Old path should be gone.
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/subdir"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/subdir/nested.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/subdir/deep/file.txt"),
        MontyObject::Bool(false)
    );

    // New path should have all descendants.
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/renamed_dir"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/renamed_dir/nested.txt"),
        MontyObject::String("nested content".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/renamed_dir/deep/file.txt"),
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/subdir/overlay_file.txt",
        MontyObject::String("overlay content".to_owned()),
    )
    .unwrap()
    .unwrap();

    call_rename(&mut mt, "/mnt/subdir", "/mnt/moved").unwrap().unwrap();

    // Overlay-written file should appear under the new name.
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/moved/overlay_file.txt"),
        MontyObject::String("overlay content".to_owned())
    );
    // Real-FS file should also appear.
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/moved/nested.txt"),
        MontyObject::String("nested content".to_owned())
    );
}

#[test]
fn ovl_mem_write_missing_parent() {
    // write_text/write_bytes to a path with missing parent should fail,
    // matching CPython's FileNotFoundError behavior.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let err = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/nonexistent/child.txt",
        MontyObject::String("x".to_owned()),
    )
    .unwrap()
    .unwrap_err()
    .into_exception();
    assert_exc(
        &err,
        ExcType::FileNotFoundError,
        "[Errno 2] No such file or directory: '/mnt/nonexistent/child.txt'",
    );

    let err = call_write(
        &mut mt,
        OsFunction::WriteBytes,
        "/mnt/nonexistent/child.bin",
        MontyObject::Bytes(vec![0]),
    )
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

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/subdir/new_file.txt",
        MontyObject::String("new content".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/subdir/new_file.txt"),
        MontyObject::String("new content".to_owned())
    );
}

#[test]
fn ovl_mem_write_after_mkdir() {
    // Writing to a path whose parent was created via mkdir should work.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_mkdir(&mut mt, "/mnt/newdir", false, false).unwrap().unwrap();

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/newdir/file.txt",
        MontyObject::String("content".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/newdir/file.txt"),
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
    call_rename(&mut mt, "/mnt/hello.txt", "/mnt/empty.txt")
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/empty.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn ovl_mem_rename_overlay_file_overwrites_overlay_file() {
    // Overwrite between two overlay-only files.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("aaa".to_owned()),
    )
    .unwrap()
    .unwrap();
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/b.txt",
        MontyObject::String("bbb".to_owned()),
    )
    .unwrap()
    .unwrap();

    call_rename(&mut mt, "/mnt/a.txt", "/mnt/b.txt").unwrap().unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/a.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/b.txt"),
        MontyObject::String("aaa".to_owned())
    );
}

#[test]
fn ovl_mem_rename_to_same_path() {
    // Renaming a file to itself should be a no-op.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_rename(&mut mt, "/mnt/hello.txt", "/mnt/hello.txt")
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/hello.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn ovl_mem_rename_deleted_file_fails() {
    // Renaming a tombstoned file should fail with not-found.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call(&mut mt, OsFunction::Unlink, "/mnt/hello.txt").unwrap().unwrap();
    let err = call_rename(&mut mt, "/mnt/hello.txt", "/mnt/other.txt")
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

    let err = call_rename(&mut mt, "/mnt/no_such_file.txt", "/mnt/other.txt")
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

    let err = call_rename(&mut mt, "/mnt/hello.txt", "/mnt/no_such_dir/file.txt")
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
    call(&mut mt, OsFunction::Unlink, "/mnt/subdir/nested.txt")
        .unwrap()
        .unwrap();
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/subdir/nested.txt"),
        MontyObject::Bool(false)
    );

    // Rename the directory.
    call_rename(&mut mt, "/mnt/subdir", "/mnt/moved").unwrap().unwrap();

    // The tombstoned file should still be invisible under the new name.
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/moved/nested.txt"),
        MontyObject::Bool(false)
    );
    // Other descendants should still be present.
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/moved/deep/file.txt"),
        MontyObject::String("deep file".to_owned())
    );
}

#[test]
fn ovl_mem_rename_deeply_nested_overlay_dirs() {
    // Renaming a directory with multiple levels of overlay-only subdirectories.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_mkdir(&mut mt, "/mnt/a", false, false).unwrap().unwrap();
    call_mkdir(&mut mt, "/mnt/a/b", false, false).unwrap().unwrap();
    call_mkdir(&mut mt, "/mnt/a/b/c", false, false).unwrap().unwrap();
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a/b/c/leaf.txt",
        MontyObject::String("leaf".to_owned()),
    )
    .unwrap()
    .unwrap();

    call_rename(&mut mt, "/mnt/a", "/mnt/x").unwrap().unwrap();

    // Old paths gone.
    assert_eq!(call_ok(&mut mt, OsFunction::Exists, "/mnt/a"), MontyObject::Bool(false));
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/a/b/c/leaf.txt"),
        MontyObject::Bool(false)
    );

    // New paths present.
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/x/b/c"),
        MontyObject::Bool(true)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/x/b/c/leaf.txt"),
        MontyObject::String("leaf".to_owned())
    );
}

#[test]
fn ovl_mem_rename_then_rename_again() {
    // A file renamed once, then renamed again — both renames should work.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_rename(&mut mt, "/mnt/hello.txt", "/mnt/step1.txt")
        .unwrap()
        .unwrap();
    call_rename(&mut mt, "/mnt/step1.txt", "/mnt/step2.txt")
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/step1.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/step2.txt"),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn ovl_mem_rename_overlay_written_file() {
    // Rename a file that was created entirely in the overlay (never on real FS).
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/new_file.txt",
        MontyObject::String("overlay only".to_owned()),
    )
    .unwrap()
    .unwrap();

    call_rename(&mut mt, "/mnt/new_file.txt", "/mnt/renamed_new.txt")
        .unwrap()
        .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/new_file.txt"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/renamed_new.txt"),
        MontyObject::String("overlay only".to_owned())
    );
}

#[test]
fn ovl_mem_rename_dir_iterdir_consistent() {
    // After renaming a directory, iterdir on both old and new parent should be correct.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_rename(&mut mt, "/mnt/subdir", "/mnt/newdir").unwrap().unwrap();

    // Old name should not appear in root listing.
    let root_listing = call_ok(&mut mt, OsFunction::Iterdir, "/mnt");
    let root_names = sorted_names(&root_listing);
    assert!(!root_names.contains(&"subdir".to_owned()), "old name still in listing");
    assert!(
        root_names.contains(&"newdir".to_owned()),
        "new name missing from listing"
    );

    // New directory listing should contain the descendants.
    let new_listing = call_ok(&mut mt, OsFunction::Iterdir, "/mnt/newdir");
    let new_names = sorted_names(&new_listing);
    assert!(new_names.contains(&"nested.txt".to_owned()));
    assert!(new_names.contains(&"deep".to_owned()));
}

#[test]
fn ovl_mem_rename_dir_over_empty_overlay_dir() {
    // Renaming a directory onto an existing empty overlay directory should succeed.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    call_mkdir(&mut mt, "/mnt/target_dir", false, false).unwrap().unwrap();

    // Write a file into subdir overlay so we can verify it moves.
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/subdir/extra.txt",
        MontyObject::String("extra".to_owned()),
    )
    .unwrap()
    .unwrap();

    call_rename(&mut mt, "/mnt/subdir", "/mnt/target_dir").unwrap().unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::Exists, "/mnt/subdir"),
        MontyObject::Bool(false)
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/target_dir/extra.txt"),
        MontyObject::String("extra".to_owned())
    );
    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/target_dir/nested.txt"),
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

    let err = call_rename(&mut mt, "/mnt1/hello.txt", "/mnt2/hello.txt")
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

    let result = call(&mut mt, OsFunction::Exists, "/unmounted/file.txt");
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
        call_ok(&mut mt, OsFunction::ReadText, "/data/sub/specific.txt"),
        MontyObject::String("from specific mount".to_owned())
    );
}

#[test]
fn non_filesystem_ops_return_none() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = mt.handle_os_call(
        OsFunction::Getenv,
        &[MontyObject::String("PATH".to_owned()), MontyObject::None],
        &[],
    );
    assert!(result.is_none(), "non-filesystem ops should return None");
}

#[test]
fn mount_prefix_no_partial_match() {
    let dir = create_test_dir();
    let mut mt = MountTable::new();
    mt.mount("/data", dir.path(), MountMode::ReadWrite, None).unwrap();

    // /data2/file should NOT match /data mount.
    let result = call(&mut mt, OsFunction::Exists, "/data2/file.txt");
    assert!(result.is_none(), "expected None for path not matching any mount prefix");
}

#[test]
fn path_with_spaces() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("hello world.txt"), "spaces").unwrap();
    let mut mt = MountTable::new();
    mt.mount("/mnt", dir.path(), MountMode::ReadWrite, None).unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/hello world.txt"),
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
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/文件.txt"),
        MontyObject::String("unicode".to_owned())
    );
}

#[test]
fn windows_style_paths_do_not_match_mounts() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Sandbox virtual paths are always POSIX-style, even on Windows hosts.
    assert!(
        call(&mut mt, OsFunction::Exists, r"\mnt\hello.txt").is_none(),
        "backslash-only paths should not match a mount"
    );
    assert!(
        call(&mut mt, OsFunction::ReadText, r"C:\mnt\hello.txt").is_none(),
        "drive-letter paths should not match a mount"
    );
    assert!(
        call(&mut mt, OsFunction::Resolve, r"/mnt\hello.txt").is_none(),
        "mixed slash and backslash paths should not match a mount"
    );
}

#[test]
fn windows_style_write_paths_do_not_touch_host_mount() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call_write(
        &mut mt,
        OsFunction::WriteText,
        r"\mnt\created.txt",
        MontyObject::String("should not be written".to_owned()),
    );
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("hello".to_owned()),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/a.txt"),
        MontyObject::String("hello".to_owned())
    );
}

#[test]
fn rw_write_text_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 10);

    // 20 bytes exceeds the 10-byte limit.
    let exc = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("a]".repeat(10)),
    )
    .unwrap()
    .expect_err("expected write limit error")
    .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 10 bytes exceeded");
}

#[test]
fn rw_write_bytes_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 5);

    let exc = call_write(
        &mut mt,
        OsFunction::WriteBytes,
        "/mnt/a.bin",
        MontyObject::Bytes(vec![0u8; 10]),
    )
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("0123456789".to_owned()),
    )
    .unwrap()
    .unwrap();

    // Second write: 10 more bytes, cumulative 20 > 15 limit.
    let exc = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/b.txt",
        MontyObject::String("0123456789".to_owned()),
    )
    .unwrap()
    .expect_err("expected write limit error")
    .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 15 bytes exceeded");
}

#[test]
fn ovl_write_text_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 10);

    let exc = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("a]".repeat(10)),
    )
    .unwrap()
    .expect_err("expected write limit error")
    .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 10 bytes exceeded");
}

#[test]
fn ovl_write_bytes_exceeds_limit() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 5);

    let exc = call_write(
        &mut mt,
        OsFunction::WriteBytes,
        "/mnt/a.bin",
        MontyObject::Bytes(vec![0u8; 10]),
    )
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("0123456789".to_owned()),
    )
    .unwrap()
    .unwrap();

    // Second write: 10 more bytes, cumulative 20 > 15 limit.
    let exc = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/b.txt",
        MontyObject::String("0123456789".to_owned()),
    )
    .unwrap()
    .expect_err("expected write limit error")
    .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 15 bytes exceeded");
}

#[test]
fn write_limit_pretty_format_kb() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 5_000);

    let exc = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("x".repeat(5_001)),
    )
    .unwrap()
    .expect_err("expected write limit error")
    .into_exception();

    assert_exc(&exc, ExcType::OSError, "disk write limit of 5 KB exceeded");
}

#[test]
fn write_limit_pretty_format_mb() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 1_500_000);

    let exc = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("x".repeat(1_500_001)),
    )
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("0123456789".to_owned()),
    )
    .unwrap()
    .unwrap();
}

#[test]
fn write_one_over_limit_fails() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::ReadWrite, 10);

    // 11 bytes with a 10-byte limit should fail.
    let exc = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/a.txt",
        MontyObject::String("01234567890".to_owned()),
    )
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/big.txt",
        MontyObject::String("x".repeat(100_000)),
    )
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
    call_ok(&mut mt, OsFunction::Unlink, "/mnt/link.txt");

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
    call_rename(&mut mt, "/mnt/link.txt", "/mnt/moved_link.txt")
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
    let result = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/no_such_dir/file.txt",
        MontyObject::String("12345".to_owned()),
    );
    assert!(result.unwrap().is_err());

    // Now write exactly at the limit — should succeed since the failed write
    // didn't consume any quota.
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/quota_ok.txt",
        MontyObject::String("0123456789".to_owned()),
    )
    .unwrap()
    .unwrap();
}

/// Same quota-preservation test for overlay mode.
#[test]
fn ovl_failed_write_does_not_consume_quota() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt_with_limit(&dir, MountMode::OverlayMemory(OverlayState::new()), 10);

    // Write to a path whose parent doesn't exist — should fail.
    let result = call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/no_such_dir/file.txt",
        MontyObject::String("12345".to_owned()),
    );
    assert!(result.unwrap().is_err());

    // Valid write of exactly 10 bytes should succeed.
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/quota_ok.txt",
        MontyObject::String("0123456789".to_owned()),
    )
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

    call_rename(&mut mt, "/mnt/subdir", "/mnt/renamed_dir")
        .unwrap()
        .unwrap();

    // Descendants should be accessible under the new prefix.
    let result = call_ok(&mut mt, OsFunction::ReadText, "/mnt/renamed_dir/nested.txt");
    assert_eq!(result, MontyObject::String("nested content".to_owned()));

    let result = call_ok(&mut mt, OsFunction::ReadText, "/mnt/renamed_dir/deep/file.txt");
    assert_eq!(result, MontyObject::String("deep file".to_owned()));

    // Old paths should not exist.
    let result = call_ok(&mut mt, OsFunction::Exists, "/mnt/subdir/nested.txt");
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

    let result = call_rename(&mut mt, "/mnt/hello.txt", "/mnt/subdir");
    let exc = result.unwrap().unwrap_err().into_exception();
    assert_eq!(exc.exc_type(), ExcType::IsADirectoryError);
}

/// Renaming a directory onto an existing file should raise NotADirectoryError.
#[test]
fn ovl_mem_rename_directory_onto_file() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let result = call_rename(&mut mt, "/mnt/subdir", "/mnt/hello.txt");
    let exc = result.unwrap().unwrap_err().into_exception();
    assert_eq!(exc.exc_type(), ExcType::NotADirectoryError);
}

/// Renaming a directory into its own descendant should raise OSError.
#[test]
fn ovl_mem_rename_directory_into_own_subdir() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

    let result = call_rename(&mut mt, "/mnt/subdir", "/mnt/subdir/deep/moved");
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
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/src.txt",
        MontyObject::String("content".to_owned()),
    )
    .unwrap()
    .unwrap();
    call_mkdir(&mut mt, "/mnt/dst_dir", false, false).unwrap().unwrap();

    let result = call_rename(&mut mt, "/mnt/src.txt", "/mnt/dst_dir");
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
    let result = call_ok(&mut mt, OsFunction::IsSymlink, "/mnt/link.txt");
    assert_eq!(result, MontyObject::Bool(true));

    // Rename the symlink
    call_rename(&mut mt, "/mnt/link.txt", "/mnt/moved_link.txt")
        .unwrap()
        .unwrap();

    // After rename: the moved path should still be readable (via the stored host ref)
    let result = call_ok(&mut mt, OsFunction::ReadText, "/mnt/moved_link.txt");
    assert_eq!(result, MontyObject::String("hello world\n".to_owned()));

    // Original symlink path should be gone
    let result = call_ok(&mut mt, OsFunction::Exists, "/mnt/link.txt");
    assert_eq!(result, MontyObject::Bool(false));

    // Original target should still exist
    let result = call_ok(&mut mt, OsFunction::Exists, "/mnt/hello.txt");
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
    call(&mut mt, OsFunction::Unlink, "/mnt/subdir/nested.txt")
        .unwrap()
        .unwrap();
    call(&mut mt, OsFunction::Unlink, "/mnt/subdir/deep/file.txt")
        .unwrap()
        .unwrap();
    call(&mut mt, OsFunction::Rmdir, "/mnt/subdir/deep").unwrap().unwrap();

    // Add an overlay-only child
    call_write(
        &mut mt,
        OsFunction::WriteText,
        "/mnt/subdir/overlay_only.txt",
        MontyObject::String("overlay".to_owned()),
    )
    .unwrap()
    .unwrap();

    // rmdir should fail because of the overlay child
    let exc = call_err(&mut mt, OsFunction::Rmdir, "/mnt/subdir");
    assert_eq!(exc.exc_type(), ExcType::OSError);
    assert!(
        exc.message().unwrap_or("").contains("Directory not empty"),
        "expected 'Directory not empty', got: {:?}",
        exc.message()
    );

    // The overlay child should still be accessible
    let result = call_ok(&mut mt, OsFunction::ReadText, "/mnt/subdir/overlay_only.txt");
    assert_eq!(result, MontyObject::String("overlay".to_owned()));
}

// =============================================================================
// on_no_handler error message format
// =============================================================================

/// `on_no_handler` for filesystem ops should not include `Errno` prefix.
#[test]
fn on_no_handler_includes_errno() {
    let exc = OsFunction::Exists.on_no_handler(&[MontyObject::Path("/outside".to_owned())]);
    assert_eq!(exc.exc_type(), ExcType::PermissionError);
    assert_eq!(exc.message().unwrap_or(""), "Permission denied: '/outside'");
}

// =============================================================================
// take_shared_mounts rollback
// =============================================================================

/// Helper: wraps a mount in a shared slot.
fn shared_slot(tmpdir: &TempDir, vpath: &str) -> Arc<Mutex<Option<Mount>>> {
    let mount = Mount::new(vpath, tmpdir.path(), MountMode::ReadOnly, None).unwrap();
    Arc::new(Mutex::new(Some(mount)))
}

/// Happy path: take and put back two mounts.
#[test]
fn take_shared_mounts_success() {
    let dir1 = create_test_dir();
    let dir2 = create_test_dir();
    let slot1 = shared_slot(&dir1, "/a");
    let slot2 = shared_slot(&dir2, "/b");
    let slots = [Arc::clone(&slot1), Arc::clone(&slot2)];

    let table = MountTable::take_shared_mounts(&slots).unwrap();
    assert_eq!(table.len(), 2);
    // Slots should be empty while table holds the mounts.
    assert!(slot1.lock().unwrap().is_none());
    assert!(slot2.lock().unwrap().is_none());

    table.put_back_shared_mounts(&slots);
    assert!(slot1.lock().unwrap().is_some());
    assert!(slot2.lock().unwrap().is_some());
}

/// When a slot is already `None`, earlier successfully-taken mounts are restored.
#[test]
fn take_shared_mounts_rollback_on_none() {
    let dir1 = create_test_dir();
    let slot1 = shared_slot(&dir1, "/a");
    let slot2: Arc<Mutex<Option<Mount>>> = Arc::new(Mutex::new(None)); // already empty
    let slots = [Arc::clone(&slot1), Arc::clone(&slot2)];

    let err = MountTable::take_shared_mounts(&slots).unwrap_err();
    assert_eq!(err, "mount 1 is already in use by another run");

    // Slot 1 should be restored, not lost.
    assert!(
        slot1.lock().unwrap().is_some(),
        "slot 1 must be restored after rollback"
    );
}

/// Passing the same slot twice: second take sees `None` and rolls back the first.
#[test]
fn take_shared_mounts_duplicate_slot_rollback() {
    let dir = create_test_dir();
    let slot = shared_slot(&dir, "/mnt");
    let slots = [Arc::clone(&slot), Arc::clone(&slot)];

    let err = MountTable::take_shared_mounts(&slots).unwrap_err();
    assert_eq!(err, "mount 1 is already in use by another run");

    // The mount should be back in the slot, not permanently lost.
    assert!(
        slot.lock().unwrap().is_some(),
        "mount must be restored after duplicate-slot failure"
    );
}

/// Rollback with three slots where the third fails.
#[test]
fn take_shared_mounts_rollback_restores_all_prior() {
    let dir1 = create_test_dir();
    let dir2 = create_test_dir();
    let slot1 = shared_slot(&dir1, "/a");
    let slot2 = shared_slot(&dir2, "/b");
    let slot3: Arc<Mutex<Option<Mount>>> = Arc::new(Mutex::new(None));
    let slots = [Arc::clone(&slot1), Arc::clone(&slot2), Arc::clone(&slot3)];

    let err = MountTable::take_shared_mounts(&slots).unwrap_err();
    assert_eq!(err, "mount 2 is already in use by another run");

    // Both prior slots should be restored.
    assert!(slot1.lock().unwrap().is_some(), "slot 1 must be restored");
    assert!(slot2.lock().unwrap().is_some(), "slot 2 must be restored");
}
