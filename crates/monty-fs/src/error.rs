//! Error types for filesystem mount operations.

use std::{
    error::Error,
    fmt,
    io::{self, ErrorKind},
};

use monty::{ExcData, ExcType, MontyException, StringRepr, unicode_decode_error_msg};

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

    /// A file contained bytes that could not be decoded as UTF-8. Carries the
    /// details needed to reproduce CPython's `UnicodeDecodeError` wording
    /// exactly (see [`monty::unicode_decode_error_msg`]).
    InvalidUtf8 {
        /// Byte offset of the first invalid byte.
        start: usize,
        /// End of the invalid byte range (exclusive); `start + 1` for a
        /// single bad byte, further for truncated multi-byte sequences.
        end: usize,
        /// The first invalid byte value, shown in the single-byte message form.
        first_byte: u8,
        /// CPython's reason wording, from [`monty::utf8_error_reason`].
        reason: &'static str,
        /// Structured exception fields including the undecodable file bytes
        /// (omitted for files above `UnicodeErrorData::MAX_OBJECT_LEN`), so
        /// hosts can build a real `UnicodeDecodeError`.
        data: ExcData,
    },

    /// Invalid mount configuration (e.g., host path doesn't exist or isn't a directory).
    InvalidMount(String),

    /// Cumulative write bytes exceeded the configured per-mount limit.
    /// The configured byte limit that was exceeded.
    WriteLimitExceeded(u64),

    /// An operation would exceed the mount's aggregate memory budget.
    /// The configured byte limit that was exceeded.
    MemoryUsageLimitExceeded(u64),
}

impl MountError {
    /// Converts this error into a [`MontyException`] for returning to the sandbox.
    #[must_use]
    pub fn into_exception(self) -> MontyException {
        match self {
            Self::NoMountPoint(path) => MontyException::new(
                ExcType::PermissionError,
                Some(format!("[Errno 13] Permission denied: {}", StringRepr(&path))),
            ),
            Self::PathEscape { virtual_path } => MontyException::new(
                ExcType::PermissionError,
                Some(format!("[Errno 13] Permission denied: {}", StringRepr(&virtual_path))),
            ),
            Self::ReadOnly(path) => MontyException::new(
                ExcType::PermissionError,
                Some(format!("[Errno 30] Read-only file system: {}", StringRepr(&path))),
            ),
            Self::CrossMountRename { src, dst } => MontyException::new(
                ExcType::OSError,
                Some(format!(
                    "[Errno 18] Invalid cross-device link: {} -> {}",
                    StringRepr(&src),
                    StringRepr(&dst)
                )),
            ),
            // Use hardcoded POSIX errno values rather than `raw_os_error()` so
            // sandboxed code sees consistent error codes regardless of host OS.
            // Windows uses different native codes (e.g. 3 for ERROR_PATH_NOT_FOUND
            // vs POSIX 2 for ENOENT).
            Self::Io(err, path) => match err.kind() {
                ErrorKind::NotFound => MontyException::new(
                    ExcType::FileNotFoundError,
                    Some(format!("[Errno 2] No such file or directory: {}", StringRepr(&path))),
                ),
                ErrorKind::AlreadyExists => MontyException::new(
                    ExcType::FileExistsError,
                    Some(format!("[Errno 17] File exists: {}", StringRepr(&path))),
                ),
                ErrorKind::PermissionDenied => MontyException::new(
                    ExcType::PermissionError,
                    Some(format!("[Errno 13] Permission denied: {}", StringRepr(&path))),
                ),
                ErrorKind::IsADirectory => MontyException::new(
                    ExcType::IsADirectoryError,
                    Some(format!("[Errno 21] Is a directory: {}", StringRepr(&path))),
                ),
                ErrorKind::NotADirectory => MontyException::new(
                    ExcType::NotADirectoryError,
                    Some(format!("[Errno 20] Not a directory: {}", StringRepr(&path))),
                ),
                ErrorKind::DirectoryNotEmpty => MontyException::new(
                    ExcType::OSError,
                    Some(format!("[Errno 39] Directory not empty: {}", StringRepr(&path))),
                ),
                ErrorKind::InvalidFilename => MontyException::new(
                    ExcType::OSError,
                    Some(format!("[Errno 36] File name too long: {}", StringRepr(&path))),
                ),
                _ => MontyException::new(ExcType::OSError, Some(format!("{err}: {}", StringRepr(&path)))),
            },
            Self::InvalidUtf8 {
                start,
                end,
                first_byte,
                reason,
                data,
            } => MontyException::new(
                ExcType::UnicodeDecodeError,
                Some(unicode_decode_error_msg("utf-8", first_byte, start, end, reason)),
            )
            .with_data(data),
            Self::InvalidMount(msg) => MontyException::new(ExcType::TypeError, Some(msg)),
            Self::WriteLimitExceeded(limit) => MontyException::new(
                ExcType::OSError,
                Some(format!("disk write limit of {} exceeded", format_bytes_pretty(limit))),
            ),
            Self::MemoryUsageLimitExceeded(limit) => MontyException::new(
                ExcType::MemoryError,
                Some(format!(
                    "mount memory usage limit of {} exceeded",
                    format_bytes_pretty(limit)
                )),
            ),
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
            Self::InvalidUtf8 { start, first_byte, .. } => {
                write!(f, "invalid UTF-8 byte 0x{first_byte:02x} at position {start}")
            }
            Self::InvalidMount(msg) => write!(f, "invalid mount: {msg}"),
            Self::WriteLimitExceeded(limit) => {
                write!(f, "disk write limit of {} exceeded", format_bytes_pretty(*limit))
            }
            Self::MemoryUsageLimitExceeded(limit) => {
                write!(
                    f,
                    "mount memory usage limit of {} exceeded",
                    format_bytes_pretty(*limit)
                )
            }
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

/// Formats a byte count as a human-readable string using decimal SI units.
///
/// Uses KB (1,000), MB (1,000,000), GB (1,000,000,000) to match common disk
/// size conventions. Values below 1 KB are displayed as whole bytes. Larger
/// values use one decimal place (e.g. `"1.5 MB"`), dropping the decimal when
/// it would be `.0`.
fn format_bytes_pretty(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    const TB: u64 = 1_000_000_000_000;

    if bytes < KB {
        return format!("{bytes} bytes");
    }

    let (value, unit) = if bytes < MB {
        (bytes as f64 / KB as f64, "KB")
    } else if bytes < GB {
        (bytes as f64 / MB as f64, "MB")
    } else if bytes < TB {
        (bytes as f64 / GB as f64, "GB")
    } else {
        (bytes as f64 / TB as f64, "TB")
    };

    // Drop the decimal place when it rounds to `.0` for cleaner display.
    let tenths = (value * 10.0).round() % 10.0;
    if tenths < f64::EPSILON {
        format!("{value:.0} {unit}")
    } else {
        format!("{value:.1} {unit}")
    }
}
