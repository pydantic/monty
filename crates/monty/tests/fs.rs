//! Integration tests for filesystem mount operations.
//!
//! Tests `MountTable::handle_os_call()` across all supported mount modes (ReadWrite,
//! ReadOnly, OverlayMemory) and all supported filesystem
//! operations. Uses real temporary directories to verify correct behavior.

use std::fs;

use monty::{
    MontyObject, OsFunction,
    fs::{MountError, MountMode, MountTable, OverlayState},
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
    mt.mount("/mnt", tmpdir.path(), mode).unwrap();
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

/// Shorthand: call and unwrap Option, expect Err.
fn call_err(mt: &mut MountTable, func: OsFunction, path: &str) -> MountError {
    call(mt, func, path).expect("expected Some").expect_err("expected Err")
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
    assert!(matches!(err, MountError::Io(_, _)));
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
fn rw_mkdir_already_exists_error() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_mkdir(&mut mt, "/mnt/subdir", false, false).unwrap().unwrap_err();
    assert!(matches!(err, MountError::Io(_, _)));
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
    assert!(matches!(err, MountError::Io(_, _)));
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
    .unwrap_err();
    assert!(matches!(err, MountError::ReadOnly(_)));
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
    .unwrap_err();
    assert!(matches!(err, MountError::ReadOnly(_)));
}

#[test]
fn ro_mkdir_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call_mkdir(&mut mt, "/mnt/newdir", false, false).unwrap().unwrap_err();
    assert!(matches!(err, MountError::ReadOnly(_)));
}

#[test]
fn ro_unlink_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call_err(&mut mt, OsFunction::Unlink, "/mnt/hello.txt");
    assert!(matches!(err, MountError::ReadOnly(_)));
}

#[test]
fn ro_rmdir_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call_err(&mut mt, OsFunction::Rmdir, "/mnt/subdir");
    assert!(matches!(err, MountError::ReadOnly(_)));
}

#[test]
fn ro_rename_blocked() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

    let err = call_rename(&mut mt, "/mnt/hello.txt", "/mnt/renamed.txt")
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, MountError::ReadOnly(_)));
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
    .unwrap_err();
    assert!(matches!(err, MountError::Io(_, _)));

    let err = call_write(
        &mut mt,
        OsFunction::WriteBytes,
        "/mnt/nonexistent/child.bin",
        MontyObject::Bytes(vec![0]),
    )
    .unwrap()
    .unwrap_err();
    assert!(matches!(err, MountError::Io(_, _)));
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
// Cross-cutting tests
// =============================================================================

#[test]
fn rename_cross_mount_error() {
    let dir1 = create_test_dir();
    let dir2 = create_test_dir();
    let mut mt = MountTable::new();
    mt.mount("/mnt1", dir1.path(), MountMode::ReadWrite).unwrap();
    mt.mount("/mnt2", dir2.path(), MountMode::ReadWrite).unwrap();

    let err = call_rename(&mut mt, "/mnt1/hello.txt", "/mnt2/hello.txt")
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, MountError::CrossMountRename { .. }));
}

#[test]
fn no_mount_point_error() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let err = call_err(&mut mt, OsFunction::Exists, "/unmounted/file.txt");
    assert!(matches!(err, MountError::NoMountPoint(_)));
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
    mt.mount("/a", dir.path(), MountMode::ReadWrite).unwrap();
    mt.mount("/b", dir.path(), MountMode::ReadOnly).unwrap();
    assert_eq!(mt.len(), 2);
    assert!(!mt.is_empty());
}

#[test]
fn mount_sorting_specific_wins() {
    let dir = create_test_dir();
    let subdir = TempDir::new().unwrap();
    fs::write(subdir.path().join("specific.txt"), "from specific mount").unwrap();

    let mut mt = MountTable::new();
    mt.mount("/data", dir.path(), MountMode::ReadWrite).unwrap();
    mt.mount("/data/sub", subdir.path(), MountMode::ReadWrite).unwrap();

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
    mt.mount("/data", dir.path(), MountMode::ReadWrite).unwrap();

    // /data2/file should NOT match /data mount.
    let err = call_err(&mut mt, OsFunction::Exists, "/data2/file.txt");
    assert!(matches!(err, MountError::NoMountPoint(_)));
}

#[test]
fn path_with_spaces() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("hello world.txt"), "spaces").unwrap();
    let mut mt = MountTable::new();
    mt.mount("/mnt", dir.path(), MountMode::ReadWrite).unwrap();

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
    mt.mount("/mnt", dir.path(), MountMode::ReadWrite).unwrap();

    assert_eq!(
        call_ok(&mut mt, OsFunction::ReadText, "/mnt/文件.txt"),
        MontyObject::String("unicode".to_owned())
    );
}
