//! Shared filesystem helpers used by both direct and overlay backends.
//!
//! These helpers keep low-level host filesystem behavior in one place so the
//! backend modules can focus on mount semantics rather than repeating the same
//! byte decoding, stat conversion, and quota bookkeeping logic.

use std::{fs, io::ErrorKind, path::Path, time::SystemTime};

use super::error::MountError;
use crate::{MontyObject, dir_stat, file_stat};

/// Per-call mount context shared by the filesystem backends.
///
/// The context carries immutable mount identity and mutable write accounting so
/// the backends do not need long parameter lists or ad hoc state threading.
pub(super) struct MountContext<'a> {
    /// Virtual mount prefix such as `"/mnt/data"`.
    pub mount_virtual: &'a str,
    /// Canonical host directory that backs the mount.
    pub mount_host: &'a Path,
    /// Cumulative bytes written through this mount.
    pub write_bytes_used: &'a mut u64,
    /// Optional cumulative write cap for the mount.
    pub write_bytes_limit: Option<u64>,
}

/// Reads a file as UTF-8 text, preserving `UnicodeDecodeError` semantics.
///
/// On Windows, `fs::read()` on a directory returns `PermissionDenied` instead of
/// `IsADirectory`, so we check explicitly before reading.
pub(super) fn read_text_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    reject_directory(path, vpath)?;
    let bytes = fs::read(path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    let content = bytes_to_utf8(bytes)?;
    Ok(MontyObject::String(content))
}

/// Reads a file as raw bytes.
///
/// On Windows, `fs::read()` on a directory returns `PermissionDenied` instead of
/// `IsADirectory`, so we check explicitly before reading.
pub(super) fn read_bytes_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    reject_directory(path, vpath)?;
    let content = fs::read(path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    Ok(MontyObject::Bytes(content))
}

/// Writes text to a file and returns the number of characters written.
///
/// On Windows, `fs::write()` on a directory returns `PermissionDenied` instead of
/// `IsADirectory`, so we check explicitly before writing.
pub(super) fn write_text_fs(path: &Path, content: &str, vpath: &str) -> Result<MontyObject, MountError> {
    reject_directory(path, vpath)?;
    fs::write(path, content).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    Ok(MontyObject::Int(
        i64::try_from(content.chars().count()).unwrap_or(i64::MAX),
    ))
}

/// Writes bytes to a file and returns the number of bytes written.
///
/// On Windows, `fs::write()` on a directory returns `PermissionDenied` instead of
/// `IsADirectory`, so we check explicitly before writing.
pub(super) fn write_bytes_fs(path: &Path, content: &[u8], vpath: &str) -> Result<MontyObject, MountError> {
    reject_directory(path, vpath)?;
    fs::write(path, content).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    Ok(MontyObject::Int(i64::try_from(content.len()).unwrap_or(i64::MAX)))
}

/// Creates a directory, handling `exist_ok=True` the same way as `pathlib`.
pub(super) fn mkdir_fs(path: &Path, parents: bool, exist_ok: bool, vpath: &str) -> Result<MontyObject, MountError> {
    let result = if parents {
        fs::create_dir_all(path)
    } else {
        fs::create_dir(path)
    };

    match result {
        Ok(()) => Ok(MontyObject::None),
        Err(err) if err.kind() == ErrorKind::AlreadyExists && exist_ok => Ok(MontyObject::None),
        Err(err) => Err(MountError::Io(err, vpath.to_owned())),
    }
}

/// Removes a file.
pub(super) fn unlink_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    fs::remove_file(path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    Ok(MontyObject::None)
}

/// Removes an empty directory.
pub(super) fn rmdir_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    fs::remove_dir(path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    Ok(MontyObject::None)
}

/// Returns a `stat_result`-shaped object for a file or directory.
pub(super) fn stat_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    let metadata = fs::metadata(path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    let mtime = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64());
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);

    if metadata.is_dir() {
        Ok(dir_stat(0o755, mtime))
    } else {
        Ok(file_stat(0o644, size, mtime))
    }
}

/// Lists visible directory entries from the real filesystem.
pub(super) fn iterdir_fs(host_path: &Path, vpath: &str, mount_host_path: &Path) -> Result<MontyObject, MountError> {
    let mut result = Vec::new();
    for name in list_visible_real_dir_entry_names(host_path, mount_host_path, vpath)? {
        result.push(MontyObject::Path(format_child_path(vpath, &name)));
    }
    Ok(MontyObject::List(result))
}

/// Validates that writing `bytes` would not exceed the mount's quota.
pub(super) fn check_write_limit(bytes: usize, ctx: &MountContext<'_>) -> Result<(), MountError> {
    if let Some(limit) = ctx.write_bytes_limit {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        if *ctx.write_bytes_used + bytes > limit {
            return Err(MountError::WriteLimitExceeded(limit));
        }
    }
    Ok(())
}

/// Records a successful write against the mount's cumulative quota counter.
pub(super) fn commit_write_bytes(bytes: usize, ctx: &mut MountContext<'_>) {
    if ctx.write_bytes_limit.is_some() {
        *ctx.write_bytes_used += u64::try_from(bytes).unwrap_or(u64::MAX);
    }
}

/// Returns visible real directory entry names for `iterdir()`.
///
/// Symlinks are only exposed when their canonical target remains within the
/// mount boundary so directory iteration does not leak the existence of
/// outbound or broken links.
pub(super) fn list_visible_real_dir_entry_names(
    host_path: &Path,
    mount_host_path: &Path,
    vpath: &str,
) -> Result<Vec<String>, MountError> {
    let read_dir = fs::read_dir(host_path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    let mut names = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|err| MountError::Io(err, vpath.to_owned()))?;
        let file_type = entry.file_type().map_err(|err| MountError::Io(err, vpath.to_owned()))?;

        if file_type.is_symlink() {
            match fs::canonicalize(entry.path()) {
                Ok(canonical) if !canonical.starts_with(mount_host_path) => continue,
                Err(_) => continue,
                _ => {}
            }
        }

        names.push(entry.file_name().to_string_lossy().to_string());
    }

    Ok(names)
}

/// Converts raw bytes to UTF-8 or returns the exact decode failure details.
pub(super) fn bytes_to_utf8(bytes: Vec<u8>) -> Result<String, MountError> {
    String::from_utf8(bytes).map_err(|err| {
        let position = err.utf8_error().valid_up_to();
        let invalid_byte = err.into_bytes()[position];
        MountError::InvalidUtf8 { position, invalid_byte }
    })
}

/// Returns the current Unix timestamp as seconds since the epoch.
pub(super) fn current_timestamp() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// Reads a directory modification time, falling back to `now` if needed.
pub(super) fn dir_mtime(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or_else(|_| SystemTime::now())
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// Returns an `IsADirectory` error if `path` is a directory.
///
/// On Windows, many `std::fs` operations on directories return
/// `ErrorKind::PermissionDenied` instead of `ErrorKind::IsADirectory`.
/// This helper normalises the behaviour across platforms so callers get
/// the correct Python exception regardless of host OS.
fn reject_directory(path: &Path, vpath: &str) -> Result<(), MountError> {
    if path.is_dir() {
        return Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath));
    }
    Ok(())
}

/// Formats a child virtual path without introducing duplicate separators.
pub(super) fn format_child_path(parent: &str, child: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{child}")
    } else {
        format!("{parent}/{child}")
    }
}
