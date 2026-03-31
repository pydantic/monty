//! Error types for filesystem mount operations.

use std::{
    borrow::Cow,
    error::Error,
    fmt,
    io::{self, ErrorKind},
};

use crate::{ExcType, MontyException};

/// Errors from mount configuration or filesystem operations.
#[derive(Debug)]
pub enum MountError {
    /// The virtual path does not fall under any configured mount point.
    NoMountPoint(String),

    /// Path traversal or symlink escape detected. The resolved host path is
    /// intentionally NOT included to avoid leaking host filesystem information.
    PathEscape {
        /// The virtual path that the sandbox code attempted to access.
        virtual_path: String,
    },

    /// A write operation was attempted on a read-only mount.
    ReadOnly(String),

    /// A rename was attempted across different mount points (EXDEV).
    CrossMountRename {
        /// The source virtual path.
        src: String,
        /// The destination virtual path.
        dst: String,
    },

    /// An I/O error from the host filesystem.
    Io(io::Error, String),

    /// A file contained bytes that could not be decoded as UTF-8.
    InvalidUtf8(String),

    /// Invalid mount configuration (e.g., host path doesn't exist or isn't a directory).
    InvalidMount(String),
}

impl MountError {
    /// Converts this error into a [`MontyException`] for returning to the sandbox.
    #[must_use]
    pub fn into_exception(self) -> MontyException {
        match self {
            Self::NoMountPoint(path) => MontyException::new(
                ExcType::PermissionError,
                Some(format!("[Errno 13] Permission denied: '{path}'")),
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
                let raw_msg = err.to_string();
                let msg: Cow<str> = match raw_msg.as_str() {
                    "No such file or directory (os error 2)" => "[Errno 2] No such file or directory".into(),
                    _ => raw_msg.into(),
                };
                MontyException::new(exc_type, Some(format!("{msg}: '{path}'")))
            }
            Self::InvalidUtf8(path) => MontyException::new(
                ExcType::UnicodeDecodeError,
                Some(format!(
                    "'utf-8' codec can't decode bytes in '{path}': invalid utf-8 sequence"
                )),
            ),
            Self::InvalidMount(msg) => MontyException::new(ExcType::ValueError, Some(msg)),
        }
    }

    /// Creates a `MountError::Io` with a constructed `io::Error`.
    pub(super) fn io_err(kind: ErrorKind, msg: &str, vpath: &str) -> Self {
        Self::Io(io::Error::new(kind, msg), vpath.to_owned())
    }

    /// Shorthand for a "not found" error.
    pub(super) fn not_found(vpath: &str) -> Self {
        Self::io_err(ErrorKind::NotFound, "No such file or directory", vpath)
    }
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMountPoint(path) => write!(f, "no mount point for path: {path}"),
            Self::PathEscape { virtual_path } => write!(f, "path escape detected: {virtual_path}"),
            Self::ReadOnly(path) => write!(f, "read-only mount: {path}"),
            Self::CrossMountRename { src, dst } => write!(f, "cross-mount rename: {src} -> {dst}"),
            Self::Io(err, path) => write!(f, "I/O error on {path}: {err}"),
            Self::InvalidUtf8(path) => write!(f, "invalid UTF-8 in {path}"),
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
