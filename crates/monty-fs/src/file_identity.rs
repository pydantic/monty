//! Host file identity, used to bind a security check to the file it checked.
//!
//! Path resolution validates a *name*, and the operation that follows resolves
//! it again — a concurrent `os.rename` in between decides which file that is.
//! Capturing identity where the boundary check passes and re-checking it
//! against the opened handle turns that substitution into a `PathEscape`.
//!
//! `(device, inode)` on Unix, `(volume serial, file index)` on Windows. Both
//! are reused after a delete, so this detects substitution rather than proving
//! a file is the same one across a deletion.

#[cfg(unix)]
use std::{fs, os::unix::fs::MetadataExt};
use std::{fs::File, path::Path};

use super::error::MountError;

/// Identifies a specific file on the host, independent of the name used to
/// reach it.
///
/// Capture with [`from_path`](Self::from_path) where a boundary check passes,
/// then [`verify`](Self::verify) against the handle actually opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileIdentity {
    /// `st_dev` on Unix, volume serial number on Windows.
    volume: u64,
    /// `st_ino` on Unix, file index on Windows.
    index: u64,
}

impl FileIdentity {
    /// Captures the identity of whatever `path` currently names, following
    /// symlinks.
    ///
    /// `None` (unreadable file) means callers skip verification, so this is a
    /// second lock on the door — never the door itself.
    pub fn from_path(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        {
            let metadata = fs::metadata(path).ok()?;
            Some(Self {
                volume: metadata.dev(),
                index: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            // Windows fills these in only for handle-derived info. The handle
            // is dropped at once: holding it would block a later `remove_file`.

            let handle = winapi_util::Handle::from_path_any(path).ok()?;
            Self::from_handle(&handle)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            None
        }
    }

    /// Captures the identity of an already-open file — the file the operation
    /// will actually touch, rather than the one a name points at.
    pub fn from_file(file: &File) -> Option<Self> {
        #[cfg(unix)]
        {
            let metadata = file.metadata().ok()?;
            Some(Self {
                volume: metadata.dev(),
                index: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            Self::from_handle(file)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            None
        }
    }

    /// Reads `BY_HANDLE_FILE_INFORMATION` for an open Windows handle.
    #[cfg(windows)]
    fn from_handle(handle: impl winapi_util::AsHandleRef) -> Option<Self> {
        let info = winapi_util::file::information(handle).ok()?;
        Some(Self {
            volume: info.volume_serial_number(),
            index: info.file_index(),
        })
    }

    /// Rejects `file` unless it is the file `expected` described — a mismatch
    /// means the handle refers to a file that was never validated.
    ///
    /// `expected` of `None` skips the check, leaving the boundary check to
    /// stand alone; with a baseline it fails closed, rejecting a handle whose
    /// identity cannot be read.
    pub fn verify(expected: Option<Self>, file: &File, vpath: &str) -> Result<(), MountError> {
        match expected {
            Some(expected) if Self::from_file(file) != Some(expected) => Err(MountError::PathEscape {
                virtual_path: vpath.to_owned(),
            }),
            _ => Ok(()),
        }
    }
}

// Tests live here because the check is unreachable from outside the crate:
// triggering it needs a substitution inside the resolve→open window, and
// anything staged via the public API lands before resolution, where the
// boundary check rejects it first.
#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// Must match, or every legitimate read is rejected.
    #[test]
    fn path_and_handle_agree_for_the_same_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, "content").unwrap();

        let from_path = FileIdentity::from_path(&path).expect("path capture failed");
        let file = File::open(&path).unwrap();
        assert_eq!(Some(from_path), FileIdentity::from_file(&file));
        assert!(FileIdentity::verify(Some(from_path), &file, "/mnt/file.txt").is_ok());
    }

    /// Must differ, or substitution goes undetected.
    #[test]
    fn distinct_files_have_distinct_identities() {
        let dir = TempDir::new().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        assert_ne!(FileIdentity::from_path(&a), FileIdentity::from_path(&b));
    }

    /// Identity follows the file, not the name — the property the whole check
    /// rests on.
    #[test]
    fn identity_survives_rename() {
        let dir = TempDir::new().unwrap();
        let (before, after) = (dir.path().join("before.txt"), dir.path().join("after.txt"));
        fs::write(&before, "content").unwrap();

        let captured = FileIdentity::from_path(&before);
        fs::rename(&before, &after).unwrap();
        assert_eq!(captured, FileIdentity::from_path(&after));
    }

    /// The case the module exists for: the name now refers to a different file.
    #[test]
    fn verify_rejects_a_substituted_file() {
        let dir = TempDir::new().unwrap();
        let (target, decoy) = (dir.path().join("target.txt"), dir.path().join("decoy.txt"));
        fs::write(&target, "target").unwrap();
        fs::write(&decoy, "decoy").unwrap();

        let captured = FileIdentity::from_path(&target);
        // Stand in for a rename landing between capture and open.
        fs::rename(&decoy, &target).unwrap();

        let opened = File::open(&target).unwrap();
        let outcome = FileIdentity::verify(captured, &opened, "/mnt/target.txt");
        assert!(
            matches!(outcome, Err(MountError::PathEscape { .. })),
            "expected PathEscape, got {outcome:?}"
        );
    }

    /// Creation targets resolve with `None` and must not be rejected.
    #[test]
    fn verify_without_a_baseline_permits_the_open() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, "content").unwrap();

        let file = File::open(&path).unwrap();
        assert!(FileIdentity::verify(None, &file, "/mnt/file.txt").is_ok());
    }
}
