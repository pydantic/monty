//! Helpers shared across the `monty-fs` integration tests.
//!
//! Each `tests/*.rs` file compiles as its own crate, so anything used by more
//! than one of them lives here rather than being copied. Symlink creation is
//! the main case: the std API differs by platform, and mount-boundary tests
//! need symlinks on every platform CI runs.

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir as win_symlink_dir, symlink_file as win_symlink_file};
use std::path::Path;

/// Cross-platform symlink to a file.
///
/// On Unix uses `std::os::unix::fs::symlink`, on Windows uses
/// `std::os::windows::fs::symlink_file`, which needs Developer Mode or an
/// elevated shell.
pub fn symlink_file(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    symlink(original.as_ref(), link.as_ref()).expect("failed to create file symlink");
    #[cfg(windows)]
    win_symlink_file(original.as_ref(), link.as_ref())
        .expect("failed to create file symlink (enable Windows Developer Mode or run elevated)");
}

/// Cross-platform symlink to a directory.
///
/// On Unix uses `std::os::unix::fs::symlink`, on Windows uses
/// `std::os::windows::fs::symlink_dir`, which needs Developer Mode or an
/// elevated shell.
pub fn symlink_dir(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    symlink(original.as_ref(), link.as_ref()).expect("failed to create directory symlink");
    #[cfg(windows)]
    win_symlink_dir(original.as_ref(), link.as_ref())
        .expect("failed to create directory symlink (enable Windows Developer Mode or run elevated)");
}
