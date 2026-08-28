//! Helpers shared across the `monty-fs` integration tests.
//!
//! Each `tests/*.rs` compiles as its own crate, so anything used by more than
//! one of them lives here rather than being copied.

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir as win_symlink_dir, symlink_file as win_symlink_file};
use std::{fs, io, path::Path, sync::OnceLock};

use tempfile::TempDir;

/// Cross-platform symlink to a file. Guard the test with [`symlinks_supported`].
pub fn symlink_file(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    try_symlink_file(original, link).expect("failed to create file symlink");
}

/// Cross-platform symlink to a directory. Guard the test with [`symlinks_supported`].
pub fn symlink_dir(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    try_symlink_dir(original, link).expect("failed to create directory symlink");
}

/// Whether this host can create symlinks at all, probed once per test binary.
///
/// Windows needs Developer Mode or elevation; without it there is nothing under
/// test, so symlink tests should skip rather than fail the whole suite. Keep the
/// panicking helpers for use *after* this returns true, where a failure is a
/// real bug rather than a missing privilege.
pub fn symlinks_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let Ok(dir) = TempDir::new() else {
            return false;
        };
        let supported = try_symlink_file("probe-target", dir.path().join("probe-link")).is_ok();
        if !supported {
            eprintln!(
                "symlink tests will skip: this host cannot create symlinks \
                 (Windows needs Developer Mode or elevation)"
            );
        }
        supported
    })
}

fn try_symlink_file(original: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    return symlink(original.as_ref(), link.as_ref());
    #[cfg(windows)]
    return win_symlink_file(original.as_ref(), link.as_ref());
}

fn try_symlink_dir(original: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    return symlink(original.as_ref(), link.as_ref());
    #[cfg(windows)]
    return win_symlink_dir(original.as_ref(), link.as_ref());
}

/// Renames a mount's host directory, reporting whether the OS allowed it.
///
/// Windows refuses with `ERROR_SHARING_VIOLATION` while a mount holds the
/// directory open: cap-std deliberately omits `FILE_SHARE_DELETE` for
/// directories, so a mounted directory cannot be renamed or deleted at all.
pub fn try_rename_mount_root(from: impl AsRef<Path>, to: impl AsRef<Path>) -> bool {
    match fs::rename(from.as_ref(), to.as_ref()) {
        Ok(()) => true,
        Err(err) if cfg!(windows) && err.raw_os_error() == Some(32) => false,
        Err(err) => panic!("unexpected rename failure: {err:?}"),
    }
}
