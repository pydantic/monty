//! Shared filesystem helpers used by both direct and overlay backends.
//!
//! These helpers keep low-level host filesystem behavior in one place so the
//! backend modules can focus on mount semantics rather than repeating the same
//! byte decoding, stat conversion, and quota bookkeeping logic.

use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::Path,
    time::SystemTime,
};

use monty::{MontyObject, UnicodeErrorData, dir_stat, file_stat, utf8_error_reason};

use super::error::MountError;

/// Conservative per-item charge for transient listing bookkeeping: string
/// headers, container slots, and dedup-set entries. Variable-size name and
/// path bytes are charged separately.
pub(super) const LISTING_ENTRY_MEMORY_USAGE: u64 = 128;

/// Saturating `usize` → `u64` conversion for memory bookkeeping arithmetic.
pub(super) fn as_u64(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

/// Memory still available to one operation, plus the configured per-mount
/// limit for error reporting.
///
/// `available` is what enforcement compares against (the limit minus any
/// already-retained overlay data); `limit` only feeds the `MemoryError`
/// message so users see the configured value, not the residual.
#[derive(Clone, Copy)]
pub(super) struct MemoryBudget {
    /// Bytes still available before the mount's limit is exceeded.
    pub available: u64,
    /// The configured per-mount limit, reported in errors.
    pub limit: u64,
}

impl MemoryBudget {
    /// Budget for a mount with nothing retained (direct read-only/read-write modes).
    pub fn full(limit: u64) -> Self {
        Self {
            available: limit,
            limit,
        }
    }

    /// Errors if `bytes` exceeds the available budget.
    pub fn check(self, bytes: u64) -> Result<(), MountError> {
        if bytes > self.available {
            Err(MountError::MemoryUsageLimitExceeded(self.limit))
        } else {
            Ok(())
        }
    }

    /// Returns the budget with `bytes` fewer available, erroring if `bytes`
    /// exceeds what is available.
    pub fn shrink(self, bytes: u64) -> Result<Self, MountError> {
        match self.available.checked_sub(bytes) {
            Some(available) => Ok(Self { available, ..self }),
            None => Err(MountError::MemoryUsageLimitExceeded(self.limit)),
        }
    }

    /// Halves the available budget, for a listing phase that must leave room
    /// for a similarly-sized result phase built from it.
    pub fn halved(self) -> Self {
        Self {
            available: self.available / 2,
            ..self
        }
    }
}

/// Per-call mount context shared by the filesystem backends.
///
/// The context carries mount identity and resource limits so the backends do
/// not need long parameter lists or ad hoc state threading.
pub(super) struct MountContext<'a> {
    /// Virtual mount prefix such as `"/mnt/data"`.
    pub mount_virtual: &'a str,
    /// Canonical host directory that backs the mount.
    pub mount_host: &'a Path,
    /// Cumulative bytes written through this mount.
    pub write_bytes_used: &'a mut u64,
    /// Optional cumulative write cap for the mount.
    pub write_bytes_limit: Option<u64>,
    /// Aggregate budget for retained overlay data and transient results.
    pub memory_usage_limit: u64,
}

/// Reads a file as UTF-8 text, preserving `UnicodeDecodeError` semantics.
///
/// Directory-read errors differ across platforms, so the target is checked
/// explicitly before reading.
pub(super) fn read_text_fs(path: &Path, vpath: &str, budget: MemoryBudget) -> Result<MontyObject, MountError> {
    let bytes = read_file_limited(path, vpath, budget)?;
    let content = bytes_to_utf8(bytes)?;
    Ok(MontyObject::String(content))
}

/// Reads a file as raw bytes.
///
/// Directory-read errors differ across platforms, so the target is checked
/// explicitly before reading.
pub(super) fn read_bytes_fs(path: &Path, vpath: &str, budget: MemoryBudget) -> Result<MontyObject, MountError> {
    Ok(MontyObject::Bytes(read_file_limited(path, vpath, budget)?))
}

/// Reads at most `budget + 1` bytes so an oversized file is rejected before it
/// can create an unbounded host allocation. The extra byte distinguishes a
/// file exactly at the limit from one that is larger without trusting metadata.
///
/// Metadata only serves the fast path: rejecting an obviously oversized file
/// with one `stat`, and pre-sizing the buffer (capped by the budget) to avoid
/// `read_to_end`'s doubling reallocations. Enforcement is always the byte
/// count actually read, so lying or racing metadata cannot evade the limit.
fn read_file_limited(path: &Path, vpath: &str, budget: MemoryBudget) -> Result<Vec<u8>, MountError> {
    reject_non_regular(path, vpath)?;
    let file = File::open(path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    let meta_len = file
        .metadata()
        .map_err(|err| MountError::Io(err, vpath.to_owned()))?
        .len();
    budget.check(meta_len)?;
    // The check above bounds `meta_len` by the budget, so this pre-allocation
    // can never exceed the limit being enforced.
    let mut content = Vec::with_capacity(usize::try_from(meta_len).unwrap_or(0));
    file.take(budget.available.saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    budget.check(as_u64(content.len()))?;
    Ok(content)
}

/// Writes text to a file and returns the number of characters written.
///
/// On Windows, `fs::write()` on a directory returns `PermissionDenied` instead of
/// `IsADirectory`, so we check explicitly before writing.
pub(super) fn write_text_fs(path: &Path, content: &str, vpath: &str) -> Result<MontyObject, MountError> {
    reject_non_regular(path, vpath)?;
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
    reject_non_regular(path, vpath)?;
    fs::write(path, content).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    Ok(MontyObject::Int(i64::try_from(content.len()).unwrap_or(i64::MAX)))
}

/// Appends text to a file and returns the number of characters written.
///
/// The host file is opened only for the duration of this call, preserving the
/// sandbox invariant that Monty never keeps native file handles alive.
pub(super) fn append_text_fs(path: &Path, content: &str, vpath: &str) -> Result<MontyObject, MountError> {
    append_bytes_to_file(path, content.as_bytes(), vpath)?;
    Ok(MontyObject::Int(
        i64::try_from(content.chars().count()).unwrap_or(i64::MAX),
    ))
}

/// Appends bytes to a file and returns the number of bytes written.
///
/// This is the binary counterpart of [`append_text_fs`].
pub(super) fn append_bytes_fs(path: &Path, content: &[u8], vpath: &str) -> Result<MontyObject, MountError> {
    append_bytes_to_file(path, content, vpath)?;
    Ok(MontyObject::Int(i64::try_from(content.len()).unwrap_or(i64::MAX)))
}

/// Opens `path` in append mode, writes all bytes, and closes it before returning.
fn append_bytes_to_file(path: &Path, content: &[u8], vpath: &str) -> Result<(), MountError> {
    reject_non_regular(path, vpath)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    file.write_all(content)
        .map_err(|err| MountError::Io(err, vpath.to_owned()))
}

/// Creates a directory, matching CPython `pathlib.Path.mkdir()` semantics:
///
/// - `exist_ok=False`: always raises `FileExistsError` if the path already exists
///   (whether file or directory), even with `parents=True`.
/// - `exist_ok=True`: silently succeeds only if the path is an existing **directory**.
///   If the path is an existing **file**, raises `FileExistsError` regardless.
pub(super) fn mkdir_fs(path: &Path, parents: bool, exist_ok: bool, vpath: &str) -> Result<MontyObject, MountError> {
    let result = if parents {
        // `create_dir_all` silently returns `Ok(())` when the directory already exists,
        // so we must check for pre-existing paths ourselves.
        match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() => {
                return if exist_ok {
                    Ok(MontyObject::None)
                } else {
                    Err(MountError::io_err(ErrorKind::AlreadyExists, "File exists", vpath))
                };
            }
            Ok(_) => {
                // Path exists but is a file — always an error.
                return Err(MountError::io_err(ErrorKind::AlreadyExists, "File exists", vpath));
            }
            Err(_) => {} // Path doesn't exist, proceed with creation.
        }
        fs::create_dir_all(path)
    } else {
        fs::create_dir(path)
    };

    match result {
        Ok(()) => Ok(MontyObject::None),
        Err(err) if err.kind() == ErrorKind::AlreadyExists && exist_ok && path.is_dir() => Ok(MontyObject::None),
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

/// Lists visible directory entries within the mount memory budget.
pub(super) fn iterdir_fs(
    host_path: &Path,
    vpath: &str,
    mount_host_path: &Path,
    budget: MemoryBudget,
) -> Result<MontyObject, MountError> {
    let names = list_visible_real_dir_entry_names(host_path, mount_host_path, vpath, budget.halved())?;
    let mut memory_usage = names.iter().fold(0_u64, |usage, name| {
        usage
            .saturating_add(as_u64(name.len()))
            .saturating_add(LISTING_ENTRY_MEMORY_USAGE)
    });
    let mut result = Vec::new();
    for name in names {
        let path = format_child_path(vpath, &name);
        memory_usage = memory_usage
            .saturating_add(as_u64(path.len()))
            .saturating_add(LISTING_ENTRY_MEMORY_USAGE);
        budget.check(memory_usage)?;
        result.push(MontyObject::Path(path));
    }
    Ok(MontyObject::List(result))
}

/// Validates that writing `bytes` would not exceed the mount's quota.
pub(super) fn check_write_limit(bytes: usize, ctx: &MountContext<'_>) -> Result<(), MountError> {
    if let Some(limit) = ctx.write_bytes_limit {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        if (*ctx.write_bytes_used).saturating_add(bytes) > limit {
            return Err(MountError::WriteLimitExceeded(limit));
        }
    }
    Ok(())
}

/// Records a successful write against the mount's cumulative quota counter.
pub(super) fn commit_write_bytes(bytes: usize, ctx: &mut MountContext<'_>) {
    if ctx.write_bytes_limit.is_some() {
        *ctx.write_bytes_used = (*ctx.write_bytes_used).saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
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
    budget: MemoryBudget,
) -> Result<Vec<String>, MountError> {
    let read_dir = fs::read_dir(host_path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;
    let mut names = Vec::new();
    let mut memory_usage = 0_u64;

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

        let name = entry.file_name().to_string_lossy().to_string();
        memory_usage = memory_usage
            .saturating_add(as_u64(name.len()))
            .saturating_add(LISTING_ENTRY_MEMORY_USAGE);
        budget.check(memory_usage)?;
        names.push(name);
    }

    Ok(names)
}

/// Converts raw bytes to UTF-8 or returns the exact decode failure details
/// (byte range, first bad byte, and CPython's reason wording) so the
/// resulting `UnicodeDecodeError` matches `bytes.decode('utf-8')`.
pub(super) fn bytes_to_utf8(bytes: Vec<u8>) -> Result<String, MountError> {
    String::from_utf8(bytes).map_err(|err| {
        let utf8_error = err.utf8_error();
        let start = utf8_error.valid_up_to();
        let end = utf8_error.error_len().map_or(err.as_bytes().len(), |len| start + len);
        let reason = utf8_error_reason(err.as_bytes()[start], utf8_error.error_len());
        MountError::InvalidUtf8 {
            start,
            end,
            first_byte: err.as_bytes()[start],
            reason,
            data: UnicodeErrorData::decode("utf-8", err.as_bytes(), start, end, reason),
        }
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

/// Rejects an existing `path` that is not a regular file: directories get an
/// `IsADirectory` error, and special files (FIFOs, sockets, devices) get
/// `PermissionDenied`. A missing path passes — write/append create it.
///
/// The directory check normalises Windows (where many `std::fs` operations on
/// directories return `PermissionDenied` instead of `IsADirectory`). The
/// special-file check is a hang guard: reading or writing a FIFO blocks until
/// a peer appears, and mount I/O runs on the *host* thread servicing the
/// sandbox, so it must never block on sandbox-reachable input.
///
/// TOCTOU caveat: this is a check-then-open, so another *host* process with
/// write access to the mounted directory can swap a regular file for a FIFO
/// between check and open and block the servicing thread. Sandbox code alone
/// cannot exploit this — no mount mode can create special files or symlinks —
/// so do not mount directories writable by untrusted local processes.
pub(super) fn reject_non_regular(path: &Path, vpath: &str) -> Result<(), MountError> {
    match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath)),
        Ok(meta) if !meta.is_file() => Err(MountError::io_err(
            ErrorKind::PermissionDenied,
            "Permission denied",
            vpath,
        )),
        _ => Ok(()),
    }
}

/// Formats a child virtual path without introducing duplicate separators.
pub(super) fn format_child_path(parent: &str, child: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{child}")
    } else {
        format!("{parent}/{child}")
    }
}
