//! Error types for filesystem mount operations.
//!
//! Each [`MountError`] variant maps to a specific Python exception type so that
//! sandbox code sees familiar Python errors (e.g., `PermissionError`, `FileNotFoundError`).

use std::{error::Error, fmt, io, io::ErrorKind};

use crate::{ExcType, MontyException};

/// Errors that can occur during mount configuration or filesystem operations.
///
/// These errors are converted to Python exceptions via [`MountError::into_exception`]
/// before being returned to the sandbox. The mapping follows Python's exception hierarchy
/// so that `try`/`except` blocks in sandbox code work as expected.
#[derive(Debug)]
pub enum MountError {
    /// The virtual path does not fall under any configured mount point.
    /// Maps to `FileNotFoundError` in Python.
    NoMountPoint(String),

    /// Path traversal or symlink escape detected — the resolved path falls outside
    /// the mounted directory boundary.
    ///
    /// Maps to `PermissionError` in Python. The resolved host path is intentionally
    /// NOT included in the error message to avoid leaking host filesystem information.
    PathEscape {
        /// The virtual path that the sandbox code attempted to access.
        virtual_path: String,
    },

    /// A write operation was attempted on a read-only mount.
    /// Maps to `PermissionError` in Python.
    ReadOnly(String),

    /// A rename was attempted across different mount points.
    /// Maps to `OSError` with errno 18 (EXDEV) in Python.
    CrossMountRename {
        /// The source virtual path.
        src: String,
        /// The destination virtual path.
        dst: String,
    },

    /// An I/O error from the host filesystem.
    /// Mapped to the appropriate Python exception based on [`ErrorKind`].
    Io(io::Error, String),

    /// The mount configuration is invalid (e.g., host path doesn't exist,
    /// virtual path is not absolute, or upper directory is not writable).
    InvalidMount(String),
}

impl MountError {
    /// Converts this mount error into a [`MontyException`] suitable for returning
    /// to the sandbox as a Python exception.
    ///
    /// The error messages follow Python's convention of including `[Errno N]` prefixes
    /// where appropriate, so that sandbox code can parse them if needed.
    #[must_use]
    pub fn into_exception(self) -> MontyException {
        match self {
            Self::NoMountPoint(path) => MontyException::new(
                ExcType::FileNotFoundError,
                Some(format!("[Errno 2] No such file or directory: '{path}'")),
            ),
            Self::PathEscape { virtual_path } => MontyException::new(
                ExcType::PermissionError,
                Some(format!("[Errno 13] Permission denied: '{virtual_path}'")),
            ),
            Self::ReadOnly(path) => MontyException::new(
                ExcType::PermissionError,
                Some(format!("[Errno 30] Read-only file system: '{path}'")),
            ),
            Self::CrossMountRename { src, dst } => MontyException::new(
                ExcType::OSError,
                Some(format!("[Errno 18] Invalid cross-device link: '{src}' -> '{dst}'")),
            ),
            Self::Io(err, path) => {
                let exc_type = match err.kind() {
                    ErrorKind::NotFound => ExcType::FileNotFoundError,
                    ErrorKind::AlreadyExists => ExcType::FileExistsError,
                    ErrorKind::PermissionDenied => ExcType::PermissionError,
                    _ => ExcType::OSError,
                };
                MontyException::new(exc_type, Some(format!("{err}: '{path}'")))
            }
            Self::InvalidMount(msg) => MontyException::new(ExcType::ValueError, Some(msg)),
        }
    }
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMountPoint(path) => write!(f, "no mount point for path: {path}"),
            Self::PathEscape { virtual_path } => {
                write!(f, "path escape detected: {virtual_path}")
            }
            Self::ReadOnly(path) => write!(f, "read-only mount: {path}"),
            Self::CrossMountRename { src, dst } => {
                write!(f, "cross-mount rename: {src} -> {dst}")
            }
            Self::Io(err, path) => write!(f, "I/O error on {path}: {err}"),
            Self::InvalidMount(msg) => write!(f, "invalid mount: {msg}"),
        }
    }
}

impl Error for MountError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err, _) => Some(err),
            _ => None,
        }
    }
}
