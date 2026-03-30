//! Security boundary tests for filesystem mounts.
//!
//! Exhaustively verifies that sandbox code cannot escape the mount boundary
//! via path traversal, null bytes, symlinks, or any other technique.
//! Tests cover all mount modes to ensure the security invariant holds everywhere.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use monty::{
    MontyObject, OsFunction,
    fs::{MountError, MountMode, MountTable, OverlayState},
};
use tempfile::TempDir;

// =============================================================================
// Helpers
// =============================================================================

/// Creates the standard test directory.
fn create_test_dir() -> TempDir {
    let dir = TempDir::new().expect("failed to create temp dir");
    let p = dir.path();

    fs::write(p.join("hello.txt"), "hello world\n").unwrap();
    fs::create_dir_all(p.join("subdir/deep")).unwrap();
    fs::write(p.join("subdir/nested.txt"), "nested content").unwrap();
    fs::write(p.join("subdir/deep/file.txt"), "deep file").unwrap();

    dir
}

/// Creates a mount table with mount at `/mnt` in the given mode.
fn mount_at_mnt(tmpdir: &TempDir, mode: MountMode) -> MountTable {
    let mut mt = MountTable::new();
    mt.mount("/mnt", tmpdir.path(), mode).unwrap();
    mt
}

/// Shorthand: call handle_os_call with a single path argument.
fn call(mt: &mut MountTable, func: OsFunction, path: &str) -> Option<Result<MontyObject, MountError>> {
    mt.handle_os_call(func, &[MontyObject::Path(path.to_owned())], &[])
}

/// Asserts that the operation returns an error (PathEscape, NoMountPoint, or Io).
fn assert_blocked(mt: &mut MountTable, func: OsFunction, path: &str) {
    let result = call(mt, func, path);
    match result {
        Some(Err(MountError::PathEscape { .. } | MountError::NoMountPoint(_))) => {}
        // I/O errors (NotFound, etc.) are also acceptable — the operation didn't succeed.
        Some(Err(MountError::Io(_, _))) => {}
        Some(Ok(val)) => panic!("expected blocked, got Ok({val:?}) for path: {path}"),
        None => panic!("expected Some result for filesystem op on path: {path}"),
        Some(Err(other)) => panic!("unexpected error variant for {path}: {other}"),
    }
}

/// Asserts blocked for a write operation with content.
fn assert_write_blocked(mt: &mut MountTable, func: OsFunction, path: &str) {
    let content = match func {
        OsFunction::WriteText => MontyObject::String("attack".to_owned()),
        OsFunction::WriteBytes => MontyObject::Bytes(b"attack".to_vec()),
        _ => MontyObject::None,
    };
    let result = mt.handle_os_call(func, &[MontyObject::Path(path.to_owned()), content], &[]);
    match result {
        Some(Err(MountError::PathEscape { .. } | MountError::NoMountPoint(_) | MountError::Io(_, _))) => {}
        Some(Ok(val)) => panic!("expected write blocked, got Ok({val:?}) for path: {path}"),
        None => panic!("expected Some result for filesystem write op on path: {path}"),
        Some(Err(other)) => panic!("unexpected error variant for write to {path}: {other}"),
    }
}

/// All mount modes to test against.
fn all_modes() -> Vec<(&'static str, MountMode)> {
    vec![
        ("ReadWrite", MountMode::ReadWrite),
        ("ReadOnly", MountMode::ReadOnly),
        ("OverlayMemory", MountMode::OverlayMemory(OverlayState::new())),
    ]
}

// =============================================================================
// Path traversal attacks
// =============================================================================

#[test]
fn traversal_dotdot_from_root() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/../etc/passwd");
        assert_blocked(&mut mt, OsFunction::Exists, "/mnt/../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_from_subdir() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/subdir/../../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_many_dotdots() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/a/../../../../../../../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_write_text() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_write_blocked(&mut mt, OsFunction::WriteText, "/mnt/../escape.txt");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_write_bytes() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_write_blocked(&mut mt, OsFunction::WriteBytes, "/mnt/../escape.bin");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_mkdir() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, OsFunction::Mkdir, "/mnt/../escape_dir");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_unlink() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, OsFunction::Unlink, "/mnt/../some_file");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_stat() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, OsFunction::Stat, "/mnt/../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_iterdir() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, OsFunction::Iterdir, "/mnt/..");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn valid_dotdot_within_mount() {
    // /mnt/subdir/../hello.txt normalizes to /mnt/hello.txt which is valid.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call(&mut mt, OsFunction::ReadText, "/mnt/subdir/../hello.txt")
        .unwrap()
        .unwrap();
    assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
}

// =============================================================================
// Null byte injection
// =============================================================================

#[test]
fn null_byte_middle() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/hello\x00.txt");
}

#[test]
fn null_byte_start() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    assert_blocked(&mut mt, OsFunction::Exists, "/mnt/\x00hello.txt");
}

#[test]
fn null_byte_end() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    assert_blocked(&mut mt, OsFunction::Exists, "/mnt/hello.txt\x00");
}

#[test]
fn null_byte_in_directory() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/sub\x00dir/nested.txt");
}

#[test]
fn null_byte_write_ops() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    assert_write_blocked(&mut mt, OsFunction::WriteText, "/mnt/evil\x00.txt");
    assert_write_blocked(&mut mt, OsFunction::WriteBytes, "/mnt/evil\x00.bin");
}

#[test]
fn null_byte_overlay_memory() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
    assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/hello\x00.txt");
    assert_blocked(&mut mt, OsFunction::Exists, "/mnt/\x00evil");
}

// =============================================================================
// Symlink escape
// =============================================================================

#[cfg(unix)]
mod symlink_tests {
    use super::*;

    #[test]
    fn symlink_to_outside_directory() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret data").unwrap();

        // Create symlink inside mount pointing outside.
        symlink(outside.path(), dir.path().join("escape_link")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/escape_link/secret.txt");
        assert_blocked(&mut mt, OsFunction::Exists, "/mnt/escape_link/secret.txt");
        assert_blocked(&mut mt, OsFunction::Iterdir, "/mnt/escape_link");
    }

    #[test]
    fn symlink_to_outside_file() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();

        symlink(outside.path().join("secret.txt"), dir.path().join("link_to_file")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/link_to_file");
    }

    #[test]
    fn symlink_to_parent() {
        let dir = create_test_dir();
        let parent = dir.path().parent().unwrap();

        symlink(parent, dir.path().join("parent_link")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::Iterdir, "/mnt/parent_link");
    }

    #[test]
    fn relative_symlink_escape() {
        let dir = create_test_dir();

        // Create symlink that uses relative path to escape.
        symlink("../../", dir.path().join("rel_escape")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::Iterdir, "/mnt/rel_escape");
    }

    #[test]
    fn symlink_escape_no_info_leak() {
        // Error messages should only contain virtual path, not host path.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), dir.path().join("escape")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::ReadText, "/mnt/escape/secret");
        match result {
            Some(Err(ref err)) => {
                let msg = format!("{err}");
                let host_str = dir.path().to_string_lossy();
                assert!(
                    !msg.contains(host_str.as_ref()),
                    "error message should not contain host path: {msg}"
                );
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn symlink_escape_overlay_memory() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink(outside.path(), dir.path().join("escape")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/escape/secret.txt");
        assert_blocked(&mut mt, OsFunction::Exists, "/mnt/escape/secret.txt");
    }

    #[test]
    fn symlink_escape_overlay_directory() {
        let dir = create_test_dir();
        let upper = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink(outside.path(), dir.path().join("escape")).unwrap();

        let mut mt = mount_at_mnt(
            &dir,
            MountMode::OverlayDirectory {
                upper_dir: upper.path().to_path_buf(),
            },
        );
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/escape/secret.txt");
    }

    #[test]
    fn symlink_within_mount_allowed() {
        // Symlinks that stay within the mount boundary should work.
        let dir = create_test_dir();
        symlink(dir.path().join("hello.txt"), dir.path().join("internal_link")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::ReadText, "/mnt/internal_link")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
    }
}

// =============================================================================
// Virtual path normalization edge cases
// =============================================================================

#[test]
fn double_slashes() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Double slashes should be normalized.
    assert_eq!(
        call(&mut mt, OsFunction::ReadText, "/mnt//hello.txt").unwrap().unwrap(),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn dot_components() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call(&mut mt, OsFunction::ReadText, "/mnt/./hello.txt")
            .unwrap()
            .unwrap(),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call(&mut mt, OsFunction::ReadText, "/mnt/./subdir/./nested.txt")
            .unwrap()
            .unwrap(),
        MontyObject::String("nested content".to_owned())
    );
}

#[test]
fn triple_dots_literal_name() {
    // "..." is a valid filename, not a path traversal.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Trying to read a file named "..." that doesn't exist should give NotFound, not PathEscape.
    let result = call(&mut mt, OsFunction::Exists, "/mnt/...");
    match result {
        Some(Ok(MontyObject::Bool(false))) => {} // Good — just doesn't exist.
        Some(Err(MountError::Io(_, _))) => {}    // Also acceptable.
        other => panic!("expected false or Io error for /mnt/..., got {other:?}"),
    }
}

// =============================================================================
// Mount configuration validation
// =============================================================================

#[test]
fn mount_relative_virtual_path_rejected() {
    let dir = TempDir::new().unwrap();
    let mut mt = MountTable::new();
    let err = mt.mount("relative/path", dir.path(), MountMode::ReadWrite).unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

#[test]
fn mount_nonexistent_host_path() {
    let mut mt = MountTable::new();
    let err = mt
        .mount("/mnt", "/nonexistent/path/that/does/not/exist", MountMode::ReadWrite)
        .unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

#[test]
fn mount_file_as_host_path() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("not_a_dir.txt");
    fs::write(&file_path, "content").unwrap();

    let mut mt = MountTable::new();
    let err = mt.mount("/mnt", &file_path, MountMode::ReadWrite).unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

#[test]
fn mount_overlay_dir_nonexistent_upper() {
    let dir = TempDir::new().unwrap();
    let mut mt = MountTable::new();
    let err = mt
        .mount(
            "/mnt",
            dir.path(),
            MountMode::OverlayDirectory {
                upper_dir: "/nonexistent/upper/dir".into(),
            },
        )
        .unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

// =============================================================================
// Information leakage
// =============================================================================

#[test]
fn path_escape_error_only_contains_virtual_path() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Null byte should trigger PathEscape.
    let result = call(&mut mt, OsFunction::Exists, "/mnt/file\x00evil");
    match result {
        Some(Err(MountError::PathEscape { virtual_path })) => {
            assert_eq!(virtual_path, "/mnt/file\x00evil");
            // Verify host path is not in the error.
            let host_str = dir.path().to_string_lossy();
            assert!(
                !virtual_path.contains(host_str.as_ref()),
                "PathEscape should not contain host path"
            );
        }
        other => panic!("expected PathEscape, got {other:?}"),
    }
}

#[test]
fn no_mount_point_error_only_contains_virtual_path() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call(&mut mt, OsFunction::ReadText, "/outside/secret.txt");
    match result {
        Some(Err(MountError::NoMountPoint(path))) => {
            assert_eq!(path, "/outside/secret.txt");
        }
        other => panic!("expected NoMountPoint, got {other:?}"),
    }
}

#[test]
fn error_into_exception_preserves_virtual_path() {
    // Verify that into_exception() doesn't leak host paths.
    let err = MountError::PathEscape {
        virtual_path: "/mnt/evil".to_owned(),
    };
    let exc = err.into_exception();
    let msg = exc.message().expect("exception should have message");
    assert!(msg.contains("/mnt/evil"));
    assert!(!msg.contains("/tmp/"), "should not contain tmp host paths");
    assert!(!msg.contains("/var/"), "should not contain var host paths");
}

// =============================================================================
// Operations on empty/unconfigured mount table
// =============================================================================

#[test]
fn empty_table_all_ops_error() {
    let mut mt = MountTable::new();

    for func in [
        OsFunction::Exists,
        OsFunction::IsFile,
        OsFunction::IsDir,
        OsFunction::ReadText,
        OsFunction::Stat,
        OsFunction::Iterdir,
    ] {
        let result = call(&mut mt, func, "/any/path");
        assert!(
            matches!(result, Some(Err(MountError::NoMountPoint(_)))),
            "empty table should return NoMountPoint for {func:?}"
        );
    }
}

// =============================================================================
// Traversal via rename
// =============================================================================

#[test]
fn rename_traversal_src() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = mt.handle_os_call(
        OsFunction::Rename,
        &[
            MontyObject::Path("/mnt/../etc/passwd".to_owned()),
            MontyObject::Path("/mnt/stolen.txt".to_owned()),
        ],
        &[],
    );
    match result {
        Some(Err(MountError::PathEscape { .. } | MountError::NoMountPoint(_) | MountError::Io(_, _))) => {}
        // If src doesn't match any mount, handle_rename returns None and normal dispatch
        // handles it — that will also fail.
        None => {}
        other => panic!("expected rename src traversal blocked, got {other:?}"),
    }
}

#[test]
fn rename_traversal_dst() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = mt.handle_os_call(
        OsFunction::Rename,
        &[
            MontyObject::Path("/mnt/hello.txt".to_owned()),
            MontyObject::Path("/mnt/../escape.txt".to_owned()),
        ],
        &[],
    );
    match result {
        Some(Err(
            MountError::PathEscape { .. }
            | MountError::NoMountPoint(_)
            | MountError::Io(_, _)
            | MountError::CrossMountRename { .. },
        )) => {}
        None => {} // Also acceptable — dst doesn't match any mount.
        other => panic!("expected rename dst traversal blocked, got {other:?}"),
    }
}
