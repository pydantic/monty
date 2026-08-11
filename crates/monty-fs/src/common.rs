//! Shared filesystem helpers used by both direct and overlay backends.
//!
//! These helpers keep low-level host filesystem behavior in one place so the
//! backend modules can focus on mount semantics rather than repeating the same
//! byte decoding, stat conversion, and quota bookkeeping logic.

use std::{
    ffi::OsStr,
    io::{Error as IoError, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::{
    fs::{Dir, File, Metadata, OpenOptions},
    time::SystemTime as CapSystemTime,
};
use monty_types::{MontyObject, UnicodeErrorData, dir_stat, file_stat, utf8_error_reason};
#[cfg(unix)]
use rustix::fs::OFlags;

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
    /// Descriptor for the mounted directory — the sandbox boundary. Every
    /// operation resolves relative to this, so no path can leave the mount and
    /// no concurrent rename can redirect one.
    pub mount_dir: &'a Dir,
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
pub(super) fn host_read_text(
    dir: &Dir,
    rel: &str,
    vpath: &str,
    budget: MemoryBudget,
) -> Result<MontyObject, MountError> {
    let bytes = read_file_limited(dir, rel, vpath, budget)?;
    let content = bytes_to_utf8(bytes)?;
    Ok(MontyObject::String(content))
}

/// Reads a file as raw bytes.
///
/// Directory-read errors differ across platforms, so the target is checked
/// explicitly before reading.
pub(super) fn host_read_bytes(
    dir: &Dir,
    rel: &str,
    vpath: &str,
    budget: MemoryBudget,
) -> Result<MontyObject, MountError> {
    Ok(MontyObject::Bytes(read_file_limited(dir, rel, vpath, budget)?))
}

/// Reads at most `budget + 1` bytes so an oversized file is rejected before it
/// can create an unbounded host allocation. The extra byte distinguishes a
/// file exactly at the limit from one that is larger without trusting metadata.
///
/// Metadata only serves the fast path: rejecting an obviously oversized file
/// with one `stat`, and pre-sizing the buffer (capped by the budget) to avoid
/// `read_to_end`'s doubling reallocations. Enforcement is always the byte
/// count actually read, so lying or racing metadata cannot evade the limit.
fn read_file_limited(dir: &Dir, rel: &str, vpath: &str, budget: MemoryBudget) -> Result<Vec<u8>, MountError> {
    reject_non_regular(dir, rel, vpath)?;
    let file = open_regular(dir, rel, vpath, OpenOptions::new().read(true))?;
    let meta_len = file.metadata().map_err(|err| map_io(err, vpath))?.len();
    budget.check(meta_len)?;
    // The check above bounds `meta_len` by the budget, so this pre-allocation
    // can never exceed the limit being enforced.
    let mut content = Vec::with_capacity(usize::try_from(meta_len).unwrap_or(0));
    file.take(budget.available.saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|err| map_io(err, vpath))?;
    budget.check(as_u64(content.len()))?;
    Ok(content)
}

/// Writes text to a file and returns the number of characters written.
///
/// On Windows, `fs::write()` on a directory returns `PermissionDenied` instead of
/// `IsADirectory`, so we check explicitly before writing.
pub(super) fn host_write_text(dir: &Dir, rel: &str, content: &str, vpath: &str) -> Result<MontyObject, MountError> {
    write_bytes_to_file(dir, rel, content.as_bytes(), vpath)?;
    Ok(MontyObject::Int(
        i64::try_from(content.chars().count()).unwrap_or(i64::MAX),
    ))
}

/// Writes bytes to a file and returns the number of bytes written.
///
/// On Windows, `fs::write()` on a directory returns `PermissionDenied` instead of
/// `IsADirectory`, so we check explicitly before writing.
pub(super) fn host_write_bytes(dir: &Dir, rel: &str, content: &[u8], vpath: &str) -> Result<MontyObject, MountError> {
    write_bytes_to_file(dir, rel, content, vpath)?;
    Ok(MontyObject::Int(i64::try_from(content.len()).unwrap_or(i64::MAX)))
}

/// Truncates `rel` and writes `content` through the mount descriptor.
fn write_bytes_to_file(dir: &Dir, rel: &str, content: &[u8], vpath: &str) -> Result<(), MountError> {
    reject_non_regular(dir, rel, vpath)?;
    let mut file = open_regular(
        dir,
        rel,
        vpath,
        OpenOptions::new().write(true).create(true).truncate(true),
    )?;
    file.write_all(content).map_err(|err| map_io(err, vpath))
}

/// Appends text to a file and returns the number of characters written.
///
/// The host file is opened only for the duration of this call, preserving the
/// sandbox invariant that Monty never keeps native file handles alive.
pub(super) fn host_append_text(dir: &Dir, rel: &str, content: &str, vpath: &str) -> Result<MontyObject, MountError> {
    append_bytes_to_file(dir, rel, content.as_bytes(), vpath)?;
    Ok(MontyObject::Int(
        i64::try_from(content.chars().count()).unwrap_or(i64::MAX),
    ))
}

/// Appends bytes to a file and returns the number of bytes written.
///
/// This is the binary counterpart of [`host_append_text`].
pub(super) fn host_append_bytes(dir: &Dir, rel: &str, content: &[u8], vpath: &str) -> Result<MontyObject, MountError> {
    append_bytes_to_file(dir, rel, content, vpath)?;
    Ok(MontyObject::Int(i64::try_from(content.len()).unwrap_or(i64::MAX)))
}

/// Opens `rel` in append mode, writes all bytes, and closes it before returning.
fn append_bytes_to_file(dir: &Dir, rel: &str, content: &[u8], vpath: &str) -> Result<(), MountError> {
    reject_non_regular(dir, rel, vpath)?;
    let mut file = open_regular(dir, rel, vpath, OpenOptions::new().create(true).append(true))?;
    file.write_all(content).map_err(|err| map_io(err, vpath))
}

/// Opens `rel` and rejects it unless the *handle* is a regular file — the
/// authoritative special-file guard, since a handle is bound to one inode and
/// cannot be raced the way [`reject_non_regular`]'s path check can.
fn open_regular(dir: &Dir, rel: &str, vpath: &str, options: &mut OpenOptions) -> Result<File, MountError> {
    let file = dir
        .open_with(rel, non_blocking(options))
        .map_err(|err| map_io(err, vpath))?;
    let metadata = file.metadata().map_err(|err| map_io(err, vpath))?;
    if metadata.is_file() {
        Ok(file)
    } else if metadata.is_dir() {
        Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
    } else {
        Err(MountError::io_err(
            ErrorKind::PermissionDenied,
            "Permission denied",
            vpath,
        ))
    }
}

/// Adds `O_NONBLOCK` so opening a FIFO returns instead of waiting for a peer,
/// which is what lets the guard above run after the open. No-op on regular files.
#[cfg(unix)]
fn non_blocking(options: &mut OpenOptions) -> &mut OpenOptions {
    options.custom_flags(OFlags::NONBLOCK.bits().cast_signed())
}

/// No-op: Windows named pipes aren't reachable through a `Dir`, so no open here
/// can block on a peer.
#[cfg(not(unix))]
fn non_blocking(options: &mut OpenOptions) -> &mut OpenOptions {
    options
}

/// Creates a directory, matching CPython `pathlib.Path.mkdir()` semantics:
///
/// - `exist_ok=False`: always raises `FileExistsError` if the path already exists
///   (whether file or directory), even with `parents=True`.
/// - `exist_ok=True`: silently succeeds only if the path is an existing **directory**.
///   If the path is an existing **file**, raises `FileExistsError` regardless.
pub(super) fn host_mkdir(
    dir: &Dir,
    rel: &str,
    parents: bool,
    exist_ok: bool,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    let result = if parents {
        // `create_dir_all` silently returns `Ok(())` when the directory already exists,
        // so we must check for pre-existing paths ourselves. The lookup follows,
        // matching both CPython and the `parents=false` recovery arm below: a
        // symlink to a directory satisfies `exist_ok` either way.
        match dir.metadata(rel) {
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
        dir.create_dir_all(rel)
    } else {
        dir.create_dir(rel)
    };

    match result {
        Ok(()) => Ok(MontyObject::None),
        Err(err) if err.kind() == ErrorKind::AlreadyExists && exist_ok && host_is_dir(dir, rel) => {
            Ok(MontyObject::None)
        }
        Err(err) => Err(map_io(err, vpath)),
    }
}

/// Removes a file, or the symlink itself when `rel` names one.
pub(super) fn host_unlink(dir: &Dir, rel: &str, vpath: &str) -> Result<MontyObject, MountError> {
    dir.remove_file(rel).map_err(|err| map_io(err, vpath))?;
    Ok(MontyObject::None)
}

/// Removes an empty directory.
pub(super) fn host_rmdir(dir: &Dir, rel: &str, vpath: &str) -> Result<MontyObject, MountError> {
    dir.remove_dir(rel).map_err(|err| map_io(err, vpath))?;
    Ok(MontyObject::None)
}

/// Returns a `stat_result`-shaped object for a file or directory.
pub(super) fn host_stat(dir: &Dir, rel: &str, vpath: &str) -> Result<MontyObject, MountError> {
    let metadata = dir.metadata(rel).map_err(|err| map_io(err, vpath))?;
    let mtime = mtime_secs(&metadata);
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);

    if metadata.is_dir() {
        Ok(dir_stat(0o755, mtime))
    } else {
        Ok(file_stat(0o644, size, mtime))
    }
}

/// Lists visible directory entries within the mount memory budget.
pub(super) fn host_iterdir(dir: &Dir, rel: &str, vpath: &str, budget: MemoryBudget) -> Result<MontyObject, MountError> {
    let names = host_list_visible_dir_entry_names(dir, rel, vpath, budget.halved())?;
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
/// Symlinks are only exposed when they still resolve inside the mount, so
/// iteration does not leak the existence of outbound or broken links.
pub(super) fn host_list_visible_dir_entry_names(
    dir: &Dir,
    rel: &str,
    vpath: &str,
    budget: MemoryBudget,
) -> Result<Vec<String>, MountError> {
    // `read_dir` on a missing path reports ENOTDIR, but CPython raises
    // `FileNotFoundError`, so surface the lookup failure first.
    dir.metadata(rel).map_err(|err| map_io(err, vpath))?;
    let read_dir = dir.read_dir(rel).map_err(|err| map_io(err, vpath))?;
    let mut names = Vec::new();
    let mut memory_usage = 0_u64;

    for entry in read_dir {
        let entry = entry.map_err(|err| map_io(err, vpath))?;
        let file_type = entry.file_type().map_err(|err| map_io(err, vpath))?;

        if file_type.is_symlink() {
            // Join the raw name: a lossy `String` round-trip would look up a
            // different entry and silently drop this one. The lookup must stay
            // path-based so it runs through the descriptor's confinement check —
            // `entry.metadata()` resolves the link without one.
            let child = join_mount_relative_os(rel, &entry.file_name());
            if dir.metadata(&child).is_err() {
                continue;
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

/// Seconds since the Unix epoch for a file's mtime, or `0.0` if unavailable.
pub(super) fn mtime_secs(metadata: &Metadata) -> f64 {
    metadata
        .modified()
        .map_or(SystemTime::UNIX_EPOCH, CapSystemTime::into_std)
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// Reads a directory modification time, falling back to `now` if needed.
pub(super) fn host_dir_mtime(dir: &Dir, rel: &str) -> f64 {
    dir.metadata(rel)
        .and_then(|metadata| metadata.modified())
        .map_or_else(|_| SystemTime::now(), CapSystemTime::into_std)
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// [`join_mount_relative`] for a raw directory-entry name: the child stays an
/// `OsStr` so a non-UTF-8 name survives the join and the resulting lookup
/// names the entry itself — a lossy `String` round-trip would name a
/// different path.
pub(super) fn join_mount_relative_os(rel: &str, child: &OsStr) -> PathBuf {
    if rel.is_empty() || rel == "." {
        PathBuf::from(child)
    } else {
        Path::new(rel).join(child)
    }
}

/// Joins a mount-relative directory path with a child name, yielding the
/// child's own mount-relative path — the coordinate system `Dir` operations
/// and overlay keys share. `""` and `"."` both mean the mount root.
pub(super) fn join_mount_relative(rel: &str, child: &str) -> String {
    if rel.is_empty() || rel == "." {
        child.to_owned()
    } else {
        format!("{rel}/{child}")
    }
}

/// Whether `rel` resolves to a directory inside the mount.
pub(super) fn host_is_dir(dir: &Dir, rel: &str) -> bool {
    dir.metadata(rel).is_ok_and(|meta| meta.is_dir())
}

/// Whether `rel` resolves to a regular file inside the mount.
pub(super) fn host_is_file(dir: &Dir, rel: &str) -> bool {
    dir.metadata(rel).is_ok_and(|meta| meta.is_file())
}

/// Converts a `cap-std` error into a [`MountError`].
///
/// `cap-std` reports both an escape attempt and a genuine in-mount `EACCES` as
/// `PermissionDenied`; the errno tells them apart, since its escape error is
/// synthetic on every platform (Linux maps `EXDEV` through the same
/// constructor). Keeps [`MountError::PathEscape`] meaningful for diagnostics.
pub(super) fn map_io(err: IoError, vpath: &str) -> MountError {
    if err.kind() == ErrorKind::PermissionDenied && err.raw_os_error().is_none() {
        MountError::PathEscape {
            virtual_path: vpath.to_owned(),
        }
    } else {
        MountError::Io(err, vpath.to_owned())
    }
}

/// Rejects an existing `path` that is not a regular file: directories get an
/// `IsADirectory` error, and special files (FIFOs, sockets, devices) get
/// `PermissionDenied`. A missing path passes — write/append create it.
///
/// A path check, so raceable — [`open_regular`] is the guard that decides. This
/// only buys error quality: `IsADirectory` on every platform, where the
/// post-open error depends on what the host's `open` made of the directory.
pub(super) fn reject_non_regular(dir: &Dir, rel: &str, vpath: &str) -> Result<(), MountError> {
    match dir.metadata(rel) {
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
