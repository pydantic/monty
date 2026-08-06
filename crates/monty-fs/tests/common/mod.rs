//! Helpers shared across the `monty-fs` integration tests.
//!
//! Each `tests/*.rs` compiles as its own crate, so anything used by more than
//! one of them lives here rather than being copied.

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir as win_symlink_dir, symlink_file as win_symlink_file};
use std::{fs, path::Path};

/// Cross-platform symlink to a file. Windows needs Developer Mode or elevation.
pub fn symlink_file(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    symlink(original.as_ref(), link.as_ref()).expect("failed to create file symlink");
    #[cfg(windows)]
    win_symlink_file(original.as_ref(), link.as_ref())
        .expect("failed to create file symlink (enable Windows Developer Mode or run elevated)");
}

/// Cross-platform symlink to a directory. Windows needs Developer Mode or elevation.
pub fn symlink_dir(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    symlink(original.as_ref(), link.as_ref()).expect("failed to create directory symlink");
    #[cfg(windows)]
    win_symlink_dir(original.as_ref(), link.as_ref())
        .expect("failed to create directory symlink (enable Windows Developer Mode or run elevated)");
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
