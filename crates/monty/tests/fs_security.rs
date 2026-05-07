//! Security boundary tests for filesystem mounts.
//!
//! Exhaustively verifies that sandbox code cannot escape the mount boundary
//! via path traversal, null bytes, symlinks, or any other technique.
//! Tests cover all mount modes to ensure the security invariant holds everywhere.

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir as win_symlink_dir, symlink_file as win_symlink_file};
use std::{fs, path::Path};

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
    mt.mount("/mnt", tmpdir.path(), mode, None).unwrap();
    mt
}

/// Shorthand: call handle_os_call with a single path argument.
fn call(mt: &mut MountTable, func: OsFunction, path: &str) -> Option<Result<MontyObject, MountError>> {
    mt.handle_os_call(func, &[MontyObject::Path(path.to_owned())], &[])
}

/// Shorthand: call handle_os_call with kwargs.
fn call_with_kwargs(
    mt: &mut MountTable,
    func: OsFunction,
    path: &str,
    kwargs: &[(MontyObject, MontyObject)],
) -> Option<Result<MontyObject, MountError>> {
    mt.handle_os_call(func, &[MontyObject::Path(path.to_owned())], kwargs)
}

/// Returns kwargs for `mkdir(parents=True, exist_ok=True)`.
fn mkdir_parents_kwargs() -> Vec<(MontyObject, MontyObject)> {
    vec![
        (MontyObject::String("parents".to_owned()), MontyObject::Bool(true)),
        (MontyObject::String("exist_ok".to_owned()), MontyObject::Bool(true)),
    ]
}

/// Asserts that the operation is blocked: either an error (PathEscape, NoMountPoint, Io)
/// or `None` (no matching mount for the normalized path).
fn assert_blocked(mt: &mut MountTable, func: OsFunction, path: &str) {
    let result = call(mt, func, path);
    match result {
        Some(Err(MountError::PathEscape { .. } | MountError::NoMountPoint(_) | MountError::Io(_, _))) | None => {}
        Some(Ok(val)) => panic!("expected blocked, got Ok({val:?}) for path: {path}"),
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
        Some(Err(MountError::PathEscape { .. } | MountError::NoMountPoint(_) | MountError::Io(_, _))) | None => {}
        Some(Ok(val)) => panic!("expected write blocked, got Ok({val:?}) for path: {path}"),
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

/// Cross-platform symlink to a file.
///
/// On Unix uses `std::os::unix::fs::symlink`, on Windows uses
/// `std::os::windows::fs::symlink_file`.
fn symlink_file(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    symlink(original.as_ref(), link.as_ref()).unwrap();
    #[cfg(windows)]
    win_symlink_file(original.as_ref(), link.as_ref()).unwrap();
}

/// Cross-platform symlink to a directory.
///
/// On Unix uses `std::os::unix::fs::symlink`, on Windows uses
/// `std::os::windows::fs::symlink_dir`.
fn symlink_dir(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    symlink(original.as_ref(), link.as_ref()).unwrap();
    #[cfg(windows)]
    win_symlink_dir(original.as_ref(), link.as_ref()).unwrap();
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

mod symlink_tests {
    use super::*;

    #[test]
    fn symlink_to_outside_directory() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret data").unwrap();

        // Create symlink inside mount pointing outside.
        symlink_dir(outside.path(), dir.path().join("escape_link"));

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

        symlink_file(outside.path().join("secret.txt"), dir.path().join("link_to_file"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/link_to_file");
    }

    #[test]
    fn symlink_to_parent() {
        let dir = create_test_dir();
        let parent = dir.path().parent().unwrap();

        symlink_dir(parent, dir.path().join("parent_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::Iterdir, "/mnt/parent_link");
    }

    #[test]
    #[cfg(unix)] // Relative symlink targets are not supported on Windows
    fn relative_symlink_escape() {
        let dir = create_test_dir();

        // Create symlink that uses relative path to escape.
        symlink_dir("../../", dir.path().join("rel_escape"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::Iterdir, "/mnt/rel_escape");
    }

    #[test]
    fn symlink_escape_no_info_leak() {
        // Error messages should only contain virtual path, not host path.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        symlink_dir(outside.path(), dir.path().join("escape"));

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
        symlink_dir(outside.path(), dir.path().join("escape"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/escape/secret.txt");
        assert_blocked(&mut mt, OsFunction::Exists, "/mnt/escape/secret.txt");
    }

    #[test]
    fn symlink_within_mount_allowed() {
        // Symlinks that stay within the mount boundary should work.
        let dir = create_test_dir();
        symlink_file(dir.path().join("hello.txt"), dir.path().join("internal_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::ReadText, "/mnt/internal_link")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
    }

    #[test]
    fn symlink_to_directory_within_mount_allowed() {
        // Symlink to a subdirectory within the mount should work for all operations.
        let dir = create_test_dir();
        symlink_dir(dir.path().join("subdir"), dir.path().join("dir_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        // Reading a file through the symlinked directory should work.
        let result = call(&mut mt, OsFunction::ReadText, "/mnt/dir_link/nested.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::String("nested content".to_owned()));

        // Listing the symlinked directory should work.
        let result = call(&mut mt, OsFunction::Iterdir, "/mnt/dir_link");
        assert!(result.unwrap().is_ok());

        // Checking existence through the symlink should work.
        let result = call(&mut mt, OsFunction::Exists, "/mnt/dir_link/deep/file.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::Bool(true));
    }

    #[test]
    fn chained_symlinks_within_mount_allowed() {
        // A symlink pointing to another symlink, both within the mount, should work.
        let dir = create_test_dir();
        symlink_file(dir.path().join("hello.txt"), dir.path().join("link1"));
        symlink_file(dir.path().join("link1"), dir.path().join("link2"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::ReadText, "/mnt/link2").unwrap().unwrap();
        assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
    }

    #[test]
    fn chained_symlinks_escape_blocked() {
        // A symlink within mount pointing to another symlink that escapes should be blocked.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();

        // link1 -> outside (escapes), link2 -> link1 (chain escapes)
        symlink_dir(outside.path(), dir.path().join("link1"));
        symlink_dir(dir.path().join("link1"), dir.path().join("link2"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, OsFunction::ReadText, "/mnt/link2/secret.txt");
    }

    #[test]
    fn mkdir_parents_through_symlink_escape_blocked_readwrite() {
        // Regression test: mkdir(parents=True) through a symlinked ancestor must
        // not create directories outside the mount boundary in ReadWrite mode.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();

        // Create a symlink inside the mount that points outside.
        symlink_dir(outside.path(), dir.path().join("escape"));

        let kwargs = mkdir_parents_kwargs();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_with_kwargs(&mut mt, OsFunction::Mkdir, "/mnt/escape/pwned", &kwargs);
        match result {
            Some(Err(MountError::PathEscape { .. } | MountError::Io(_, _))) => {}
            Some(Ok(_)) => panic!("mkdir through symlink escape should be blocked"),
            other => panic!("unexpected result: {other:?}"),
        }

        // Verify nothing was created outside the mount.
        assert!(
            !outside.path().join("pwned").exists(),
            "directory was created outside the mount!"
        );
    }

    #[test]
    fn mkdir_parents_through_symlink_escape_blocked_readonly() {
        // ReadOnly mode should also block mkdir through symlink escape.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        symlink_dir(outside.path(), dir.path().join("escape"));

        let kwargs = mkdir_parents_kwargs();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

        let result = call_with_kwargs(&mut mt, OsFunction::Mkdir, "/mnt/escape/pwned", &kwargs);
        match result {
            Some(Err(MountError::PathEscape { .. } | MountError::Io(_, _) | MountError::ReadOnly(_))) => {}
            Some(Ok(_)) => panic!("mkdir through symlink escape should be blocked"),
            other => panic!("unexpected result: {other:?}"),
        }

        assert!(
            !outside.path().join("pwned").exists(),
            "directory was created outside the mount!"
        );
    }

    #[test]
    fn mkdir_parents_through_nested_symlink_escape_blocked() {
        // mkdir(parents=True) through a symlinked directory deeper in the tree.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();

        // subdir/link -> outside
        symlink_dir(outside.path(), dir.path().join("subdir").join("link"));

        let kwargs = mkdir_parents_kwargs();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_with_kwargs(&mut mt, OsFunction::Mkdir, "/mnt/subdir/link/deep/dir", &kwargs);
        match result {
            Some(Err(MountError::PathEscape { .. } | MountError::Io(_, _))) => {}
            Some(Ok(_)) => panic!("mkdir through nested symlink escape should be blocked"),
            other => panic!("unexpected result: {other:?}"),
        }

        assert!(
            !outside.path().join("deep").exists(),
            "directory was created outside the mount!"
        );
    }

    #[test]
    fn mkdir_parents_within_mount_allowed() {
        // mkdir(parents=True) for paths entirely within the mount should succeed.
        let dir = create_test_dir();
        let kwargs = mkdir_parents_kwargs();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_with_kwargs(&mut mt, OsFunction::Mkdir, "/mnt/new/nested/dir", &kwargs);
        assert!(result.unwrap().is_ok(), "mkdir within mount should succeed");
        assert!(dir.path().join("new/nested/dir").exists());
    }

    #[test]
    fn mkdir_parents_through_internal_symlink_allowed() {
        // mkdir(parents=True) through a symlink that stays within the mount is fine.
        let dir = create_test_dir();

        // Create a symlink within mount pointing to another dir within mount.
        symlink_dir(dir.path().join("subdir"), dir.path().join("internal_link"));

        let kwargs = mkdir_parents_kwargs();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_with_kwargs(&mut mt, OsFunction::Mkdir, "/mnt/internal_link/new_child", &kwargs);
        assert!(result.unwrap().is_ok(), "mkdir through internal symlink should succeed");
        assert!(dir.path().join("subdir/new_child").exists());
    }
}

// =============================================================================
// Hard link tests (`ln` without `-s`)
// =============================================================================
//
// Hard links are fundamentally different from symbolic links: a hard link is
// just another directory entry for the same inode, not a pointer to a path.
// `fs::canonicalize()` returns the path as-given (within the mount), so hard
// links always pass the boundary check regardless of where the original file
// lives.
//
// This is acceptable because sandboxed code cannot create hard links (no
// `os.link` is exposed), so hard links can only be placed in the mount by
// the host — an explicit choice to expose that content.

mod hard_link_tests {
    use super::*;

    #[test]
    fn hard_link_within_mount_allowed() {
        // A hard link to a file within the mount should work normally.
        let dir = create_test_dir();
        fs::hard_link(dir.path().join("hello.txt"), dir.path().join("hardlink.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::ReadText, "/mnt/hardlink.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
    }

    #[test]
    fn hard_link_from_outside_accessible() {
        // A hard link to a file outside the mount is indistinguishable from a
        // regular file at the path level — canonicalize returns the in-mount
        // path, so the boundary check passes. This is by design: only the host
        // can create hard links in the mounted directory, so this represents an
        // explicit choice to expose the content.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("external.txt");
        fs::write(&outside_file, "external content").unwrap();

        fs::hard_link(&outside_file, dir.path().join("hardlink_ext.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::ReadText, "/mnt/hardlink_ext.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::String("external content".to_owned()));
    }

    #[test]
    fn hard_link_is_not_detected_as_symlink() {
        // Hard links should report as regular files, not symlinks.
        let dir = create_test_dir();
        fs::hard_link(dir.path().join("hello.txt"), dir.path().join("hardlink.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::IsFile, "/mnt/hardlink.txt").unwrap().unwrap();
        assert_eq!(result, MontyObject::Bool(true));

        let result = call(&mut mt, OsFunction::IsSymlink, "/mnt/hardlink.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::Bool(false));
    }

    /// A broken symlink (target doesn't exist) inside the mount that points
    /// outside must not allow `write_text` / `write_bytes` to follow it.
    #[test]
    #[cfg(unix)]
    fn broken_symlink_write_escape_blocked() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        let escape_target = outside.path().join("pwned.txt");

        // Create symlink inside mount -> outside file that doesn't exist yet.
        symlink(&escape_target, dir.path().join("broken_link.txt")).unwrap();

        // Sanity: it's a broken symlink.
        assert!(!dir.path().join("broken_link.txt").exists());
        assert!(dir.path().join("broken_link.txt").symlink_metadata().is_ok());

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_write_blocked(&mut mt, OsFunction::WriteText, "/mnt/broken_link.txt");
        assert_write_blocked(&mut mt, OsFunction::WriteBytes, "/mnt/broken_link.txt");

        // The target file must NOT have been created.
        assert!(
            !escape_target.exists(),
            "broken symlink write escape: file was created outside the mount!"
        );
    }

    /// Overlay mode writes to in-memory storage, never the real filesystem,
    /// so a broken outbound symlink on the real FS is harmless — the overlay
    /// simply stores the content in memory under the virtual path.
    #[test]
    #[cfg(unix)]
    fn broken_symlink_overlay_writes_to_memory_not_real_fs() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        let escape_target = outside.path().join("pwned.txt");

        symlink(&escape_target, dir.path().join("broken_link.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

        // Write succeeds (goes to overlay memory).
        let result = mt
            .handle_os_call(
                OsFunction::WriteText,
                &[
                    MontyObject::Path("/mnt/broken_link.txt".to_owned()),
                    MontyObject::String("safe".to_owned()),
                ],
                &[],
            )
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::Int(4));

        // Real FS target was NOT created.
        assert!(!escape_target.exists());
    }

    /// Iterdir must filter out symlinks pointing outside the mount (including
    /// broken ones) while keeping regular files and inbound symlinks.
    #[test]
    #[cfg(unix)]
    fn iterdir_filters_outbound_symlinks_but_keeps_regular_and_inbound() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("external.txt"), "external").unwrap();

        // Outbound symlink (points outside mount) — should be filtered.
        symlink(outside.path().join("external.txt"), dir.path().join("escape_link")).unwrap();
        // Inbound symlink (points inside mount) — should be kept.
        symlink(dir.path().join("hello.txt"), dir.path().join("internal_link")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, OsFunction::Iterdir, "/mnt").unwrap().unwrap();

        if let MontyObject::List(entries) = &result {
            let names: Vec<String> = entries
                .iter()
                .filter_map(|e| {
                    if let MontyObject::Path(p) = e {
                        p.rsplit('/').next().map(ToOwned::to_owned)
                    } else {
                        None
                    }
                })
                .collect();
            assert!(
                !names.contains(&"escape_link".to_owned()),
                "outbound symlink should be filtered from iterdir"
            );
            assert!(
                names.contains(&"internal_link".to_owned()),
                "inbound symlink should be kept in iterdir"
            );
            assert!(
                names.contains(&"hello.txt".to_owned()),
                "regular files should be present"
            );
        } else {
            panic!("expected List from iterdir, got {result:?}");
        }
    }

    /// Overlay mode should expose the same visible real entries as direct mode:
    /// inbound symlinks stay visible, outbound and broken symlinks are filtered.
    #[test]
    #[cfg(unix)]
    fn overlay_iterdir_filters_symlinks_like_direct_mode() {
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("external.txt"), "external").unwrap();

        symlink(outside.path().join("external.txt"), dir.path().join("escape_link")).unwrap();
        symlink(outside.path().join("missing.txt"), dir.path().join("broken_link")).unwrap();
        symlink(dir.path().join("hello.txt"), dir.path().join("internal_link")).unwrap();

        let mut direct = mount_at_mnt(&dir, MountMode::ReadWrite);
        let direct_result = call(&mut direct, OsFunction::Iterdir, "/mnt").unwrap().unwrap();

        let mut overlay = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let overlay_result = call(&mut overlay, OsFunction::Iterdir, "/mnt").unwrap().unwrap();

        let direct_names = sorted_names_from_list(&direct_result);
        let overlay_names = sorted_names_from_list(&overlay_result);

        assert_eq!(overlay_names, direct_names);
        assert!(overlay_names.contains(&"internal_link".to_owned()));
        assert!(!overlay_names.contains(&"escape_link".to_owned()));
        assert!(!overlay_names.contains(&"broken_link".to_owned()));
    }
}

/// Extracts sorted entry basenames from an `iterdir()` result list.
fn sorted_names_from_list(obj: &MontyObject) -> Vec<String> {
    match obj {
        MontyObject::List(entries) => {
            let mut names: Vec<String> = entries
                .iter()
                .filter_map(|entry| match entry {
                    MontyObject::Path(path) => path.rsplit('/').next().map(ToOwned::to_owned),
                    _ => None,
                })
                .collect();
            names.sort();
            names
        }
        other => panic!("expected List from iterdir result, got {other:?}"),
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
    let err = mt
        .mount("relative/path", dir.path(), MountMode::ReadWrite, None)
        .unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

#[test]
fn mount_nonexistent_host_path() {
    let mut mt = MountTable::new();
    let err = mt
        .mount(
            "/mnt",
            "/nonexistent/path/that/does/not/exist",
            MountMode::ReadWrite,
            None,
        )
        .unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

#[test]
fn mount_file_as_host_path() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("not_a_dir.txt");
    fs::write(&file_path, "content").unwrap();

    let mut mt = MountTable::new();
    let err = mt.mount("/mnt", &file_path, MountMode::ReadWrite, None).unwrap_err();
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
fn no_mount_point_returns_none() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call(&mut mt, OsFunction::ReadText, "/outside/secret.txt");
    assert!(
        result.is_none(),
        "expected None for path outside all mounts, got {result:?}"
    );
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
fn empty_table_all_ops_unhandled() {
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
            result.is_none(),
            "empty table should return None for {func:?}, got {result:?}"
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

// =============================================================================
// Sandbox escape via rename of symlink pointing outside mount
// =============================================================================

/// Regression test for a critical vulnerability: renaming a symlink that points
/// outside the mount boundary, then reading the renamed path, must NOT leak
/// the contents of the symlink target.
///
/// Attack flow:
/// 1. Host dir contains a symlink `escape_link -> <outside_file>`
/// 2. Sandbox renames `/mnt/escape_link` to `/mnt/renamed`
/// 3. Sandbox reads `/mnt/renamed` — overlay serves the `RealFileRef` whose
///    `host_path` is the original symlink; `fs::read` follows it and returns
///    the outside file's contents, completely bypassing boundary checks.
#[test]
fn rename_symlink_escape_overlay_read_text() {
    // Create the mount directory and a file *outside* it.
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();
    let secret = "TOP SECRET CONTENT";
    fs::write(outside_dir.path().join("secret.txt"), secret).unwrap();

    // Place a symlink inside the mount that points outside the boundary.
    symlink_file(
        outside_dir.path().join("secret.txt"),
        mount_dir.path().join("escape_link"),
    );

    let mut mt = mount_at_mnt(&mount_dir, MountMode::OverlayMemory(OverlayState::new()));

    // Step 1: Rename the symlink within the mount.
    let rename_result = mt.handle_os_call(
        OsFunction::Rename,
        &[
            MontyObject::Path("/mnt/escape_link".to_owned()),
            MontyObject::Path("/mnt/renamed".to_owned()),
        ],
        &[],
    );
    // The rename itself may succeed or may be blocked — either is acceptable.
    // The critical invariant is that reading the renamed path must NEVER
    // return the outside file's contents.

    if matches!(rename_result, Some(Ok(_))) {
        // Rename succeeded — now try to read the renamed path.
        let read_result = call(&mut mt, OsFunction::ReadText, "/mnt/renamed");
        match read_result {
            Some(Ok(MontyObject::String(content))) => {
                assert_ne!(
                    content, secret,
                    "SECURITY: overlay read_text leaked file contents from outside the mount boundary \
                     via a renamed symlink"
                );
            }
            Some(Err(_)) => {
                // An error (e.g. PathEscape, NotFound) is a valid safe outcome.
            }
            None => {
                // No mount matched — also safe.
            }
            other => panic!("unexpected read result: {other:?}"),
        }
    }
}

/// Same as above but for `read_bytes`.
#[test]
fn rename_symlink_escape_overlay_read_bytes() {
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();
    let secret = b"TOP SECRET BYTES";
    fs::write(outside_dir.path().join("secret.bin"), secret.as_slice()).unwrap();

    symlink_file(
        outside_dir.path().join("secret.bin"),
        mount_dir.path().join("escape_link"),
    );

    let mut mt = mount_at_mnt(&mount_dir, MountMode::OverlayMemory(OverlayState::new()));

    let rename_result = mt.handle_os_call(
        OsFunction::Rename,
        &[
            MontyObject::Path("/mnt/escape_link".to_owned()),
            MontyObject::Path("/mnt/renamed".to_owned()),
        ],
        &[],
    );

    if matches!(rename_result, Some(Ok(_))) {
        let read_result = call(&mut mt, OsFunction::ReadBytes, "/mnt/renamed");
        match read_result {
            Some(Ok(MontyObject::Bytes(content))) => {
                assert_ne!(
                    content,
                    secret.as_slice(),
                    "SECURITY: overlay read_bytes leaked file contents from outside the mount boundary \
                     via a renamed symlink"
                );
            }
            Some(Err(_)) => {}
            None => {}
            other => panic!("unexpected read result: {other:?}"),
        }
    }
}
