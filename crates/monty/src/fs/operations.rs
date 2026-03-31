//! Filesystem operation handlers for each mount mode.
//!
//! Each [`OsFunction`] filesystem variant is dispatched here based on the [`MountMode`]:
//!
//! - **`ReadWrite`**: direct pass-through to [`std::fs`] on the resolved host path
//! - **`ReadOnly`**: read ops pass through; write ops return [`MountError::ReadOnly`]
//! - **`OverlayMemory`**: reads check overlay first then fall through; writes go to overlay

use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    time::SystemTime,
};

use ahash::AHashSet;

use super::{
    error::MountError,
    mount_mode::{MountMode, OverlayEntry, OverlayFile, OverlayFileRef, OverlayState},
    path_security::{
        normalize_virtual_path, resolve_path, resolve_path_for_lstat, resolve_path_mkdir_parents, strip_mount_prefix,
    },
};
use crate::{MontyObject, dir_stat, file_stat, os::OsFunction};

/// Mount-specific context passed through the operation call chain.
///
/// Carries immutable mount identity (paths) and mutable write-tracking
/// state so that write operations can enforce byte limits without needing
/// extra function parameters at every call site.
pub(super) struct MountContext<'a> {
    /// The virtual path prefix of the mount (e.g., `/data`).
    pub mount_virtual: &'a str,
    /// The canonical host directory backing the mount.
    pub mount_host: &'a Path,
    /// Cumulative bytes written through this mount (monotonically increasing).
    pub write_bytes_used: &'a mut u64,
    /// Optional cap on total bytes written. Writes exceeding this raise `OSError`.
    pub write_bytes_limit: Option<u64>,
}

/// Executes a filesystem operation against a mount.
///
/// This is the main dispatch function called by
/// [`MountTable::handle_os_call`](super::MountTable::handle_os_call).
/// It resolves the virtual path, checks access permissions based on the mount
/// mode, and executes the operation.
pub fn execute(
    function: OsFunction,
    virtual_path: &str,
    extra_args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    ctx: &mut MountContext<'_>,
    mode: &mut MountMode,
) -> Result<MontyObject, MountError> {
    // For write operations, check if the mode allows writes.
    if function.is_write() && matches!(mode, MountMode::ReadOnly) {
        return Err(MountError::ReadOnly(virtual_path.to_owned()));
    }

    match mode {
        MountMode::ReadWrite | MountMode::ReadOnly => execute_direct(function, virtual_path, extra_args, kwargs, ctx),
        MountMode::OverlayMemory(state) => {
            execute_overlay_memory(function, virtual_path, extra_args, kwargs, ctx, state)
        }
    }
}

// =============================================================================
// Direct filesystem operations (ReadWrite / ReadOnly)
// =============================================================================

/// Executes a filesystem operation directly against the host filesystem.
fn execute_direct(
    function: OsFunction,
    vpath: &str,
    extra_args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    // Resolve/Absolute are pure virtual-path operations — no host I/O needed.
    if matches!(function, OsFunction::Resolve | OsFunction::Absolute) {
        return Ok(MontyObject::Path(normalize_virtual_path(vpath)));
    }

    let host = resolve_host_path(function, vpath, kwargs, ctx)?;

    match function {
        OsFunction::Exists => Ok(MontyObject::Bool(host.exists())),
        OsFunction::IsFile => Ok(MontyObject::Bool(host.is_file())),
        OsFunction::IsDir => Ok(MontyObject::Bool(host.is_dir())),
        OsFunction::IsSymlink => Ok(MontyObject::Bool(host.is_symlink())),
        OsFunction::ReadText => read_text_fs(&host, vpath),
        OsFunction::ReadBytes => read_bytes_fs(&host, vpath),
        OsFunction::WriteText => {
            let content = extract_string_arg(extra_args, "write_text")?;
            check_write_limit(content.len(), ctx)?;
            write_text_fs(&host, content, vpath)
        }
        OsFunction::WriteBytes => {
            let content = extract_bytes_arg(extra_args, "write_bytes")?;
            check_write_limit(content.len(), ctx)?;
            write_bytes_fs(&host, content, vpath)
        }
        OsFunction::Mkdir => {
            let (parents, exist_ok) = extract_mkdir_kwargs(kwargs);
            mkdir_fs(&host, parents, exist_ok, vpath)
        }
        OsFunction::Unlink => unlink_fs(&host, vpath),
        OsFunction::Rmdir => rmdir_fs(&host, vpath),
        OsFunction::Stat => stat_fs(&host, vpath),
        OsFunction::Iterdir => iterdir_fs(&host, vpath, ctx.mount_host),
        OsFunction::Rename => rename_fs(vpath, &extract_path_arg(extra_args, "rename")?, ctx),
        _ => unreachable!("all filesystem operations are handled above"),
    }
}

/// Resolves a virtual path to a validated host path, handling per-operation quirks.
///
/// Three categories of operations need different resolution strategies:
/// - **Existence checks** (`exists`, `is_file`, etc.): return a sentinel path on
///   `NotFound` so the caller gets `false` instead of an error, matching CPython.
/// - **`mkdir` with `parents`**: intermediate directories may not exist yet, so we
///   construct the path from the mount root without canonicalizing ancestors.
/// - **Everything else**: standard `resolve_path` with `for_creation` for writes.
fn resolve_host_path(
    function: OsFunction,
    vpath: &str,
    kwargs: &[(MontyObject, MontyObject)],
    ctx: &MountContext<'_>,
) -> Result<PathBuf, MountError> {
    // is_symlink needs special resolution: canonicalize the parent but NOT the
    // final component, so symlink identity is preserved for the metadata check.
    if matches!(function, OsFunction::IsSymlink) {
        match resolve_path_for_lstat(vpath, ctx.mount_virtual, ctx.mount_host) {
            Ok(r) => return Ok(r.host_path),
            Err(MountError::Io(_, _)) => return Ok(PathBuf::from("/nonexistent")),
            Err(e) => return Err(e),
        }
    }

    // Existence checks return false for nonexistent paths rather than erroring.
    if function.is_existence_check() {
        match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
            Ok(r) => Ok(r.host_path),
            // Path doesn't exist on the host — return a non-existent path so the
            // caller's `.exists()` / `.is_file()` / etc. naturally returns false.
            Err(MountError::Io(_, _)) => Ok(PathBuf::from("/nonexistent")),
            Err(e) => Err(e),
        }
    } else if matches!(function, OsFunction::Mkdir) && extract_mkdir_kwargs(kwargs).0 {
        // `mkdir -p` can't use resolve_path because intermediate parents may not exist.
        // Use resolve_path_mkdir_parents which walks existing ancestors, canonicalizing
        // each to detect symlinks that escape the mount boundary.
        Ok(resolve_path_mkdir_parents(vpath, ctx.mount_virtual, ctx.mount_host)?.host_path)
    } else {
        let for_creation =
            function.is_write() && !matches!(function, OsFunction::Unlink | OsFunction::Rmdir | OsFunction::Rename);
        Ok(resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, for_creation)?.host_path)
    }
}

// =============================================================================
// Overlay memory operations
// =============================================================================

/// Executes a filesystem operation with in-memory overlay semantics.
fn execute_overlay_memory(
    function: OsFunction,
    vpath: &str,
    extra_args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    ctx: &mut MountContext<'_>,
    state: &mut OverlayState,
) -> Result<MontyObject, MountError> {
    let normalized = normalize_virtual_path(vpath);
    let relative = strip_mount_prefix(&normalized, ctx.mount_virtual)
        .ok_or_else(|| MountError::NoMountPoint(vpath.to_owned()))?
        .to_owned();

    match function {
        OsFunction::Exists => overlay_exists(state, &relative, ctx, vpath),
        OsFunction::IsFile => overlay_is_file(state, &relative, ctx, vpath),
        OsFunction::IsDir => overlay_is_dir(state, &relative, ctx, vpath),
        OsFunction::IsSymlink => overlay_is_symlink(state, &relative, ctx, vpath),
        OsFunction::ReadText => overlay_read_text(state, &relative, ctx, vpath),
        OsFunction::ReadBytes => overlay_read_bytes(state, &relative, ctx, vpath),
        OsFunction::WriteText => {
            let content = extract_string_arg(extra_args, "write_text")?;
            check_write_limit(content.len(), ctx)?;
            overlay_write_text(state, relative, content, ctx, vpath)
        }
        OsFunction::WriteBytes => {
            let content = extract_bytes_arg(extra_args, "write_bytes")?;
            check_write_limit(content.len(), ctx)?;
            overlay_write_bytes(state, relative, content, ctx, vpath)
        }
        OsFunction::Mkdir => {
            let (parents, exist_ok) = extract_mkdir_kwargs(kwargs);
            overlay_mkdir(state, &relative, parents, exist_ok, ctx, vpath)
        }
        OsFunction::Unlink => overlay_unlink(state, &relative, ctx, vpath),
        OsFunction::Rmdir => overlay_rmdir(state, &relative, ctx, vpath),
        OsFunction::Stat => overlay_stat(state, &relative, ctx, vpath),
        OsFunction::Iterdir => overlay_iterdir(state, &relative, ctx, vpath),
        OsFunction::Rename => {
            let target = extract_path_arg(extra_args, "rename")?;
            overlay_rename(state, vpath, &target, ctx)
        }
        OsFunction::Resolve | OsFunction::Absolute => Ok(MontyObject::Path(normalize_virtual_path(vpath))),
        _ => unreachable!("all filesystem operations are handled above"),
    }
}

// --- Overlay read operations ---

/// Checks whether a path exists in the overlay or real filesystem.
fn overlay_exists(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_) | OverlayEntry::Directory { .. }) => {
            Ok(MontyObject::Bool(true))
        }
        Some(OverlayEntry::Deleted) => Ok(MontyObject::Bool(false)),
        None => match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
            Ok(r) => Ok(MontyObject::Bool(r.host_path.exists())),
            Err(MountError::Io(_, _)) => Ok(MontyObject::Bool(false)),
            Err(e) => Err(e),
        },
    }
}

/// Checks whether a path is a file in the overlay or real filesystem.
fn overlay_is_file(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => Ok(MontyObject::Bool(true)),
        Some(OverlayEntry::Directory { .. } | OverlayEntry::Deleted) => Ok(MontyObject::Bool(false)),
        None => match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
            Ok(r) => Ok(MontyObject::Bool(r.host_path.is_file())),
            Err(MountError::Io(_, _)) => Ok(MontyObject::Bool(false)),
            Err(e) => Err(e),
        },
    }
}

/// Checks whether a path is a directory in the overlay or real filesystem.
fn overlay_is_dir(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => Ok(MontyObject::Bool(true)),
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_) | OverlayEntry::Deleted) => {
            Ok(MontyObject::Bool(false))
        }
        None => match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
            Ok(r) => Ok(MontyObject::Bool(r.host_path.is_dir())),
            Err(MountError::Io(_, _)) => Ok(MontyObject::Bool(false)),
            Err(e) => Err(e),
        },
    }
}

/// Checks whether a path is a symlink. Overlay entries are never symlinks.
fn overlay_is_symlink(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(_) => Ok(MontyObject::Bool(false)),
        None => match resolve_path_for_lstat(vpath, ctx.mount_virtual, ctx.mount_host) {
            Ok(r) => Ok(MontyObject::Bool(r.host_path.is_symlink())),
            Err(MountError::Io(_, _)) => Ok(MontyObject::Bool(false)),
            Err(e) => Err(e),
        },
    }
}

/// Reads a file as text from the overlay or real filesystem.
fn overlay_read_text(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(f)) => {
            let text = bytes_to_utf8(f.content.clone())?;
            Ok(MontyObject::String(text))
        }
        Some(OverlayEntry::RealFileRef(r)) => read_text_fs(&r.host_path, vpath),
        Some(OverlayEntry::Directory { .. }) => Err(MountError::io_err(ErrorKind::Other, "Is a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            read_text_fs(&r.host_path, vpath)
        }
    }
}

/// Reads a file as bytes from the overlay or real filesystem.
fn overlay_read_bytes(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(f)) => Ok(MontyObject::Bytes(f.content.clone())),
        Some(OverlayEntry::RealFileRef(r)) => read_bytes_fs(&r.host_path, vpath),
        Some(OverlayEntry::Directory { .. }) => Err(MountError::io_err(ErrorKind::Other, "Is a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            read_bytes_fs(&r.host_path, vpath)
        }
    }
}

// --- Overlay write operations ---

/// Writes text content to the overlay, returning character count (not byte length).
fn overlay_write_text(
    state: &mut OverlayState,
    relative: String,
    content: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    overlay_check_parent_exists(state, &relative, ctx, vpath)?;
    let len = i64::try_from(content.chars().count()).unwrap_or(i64::MAX);
    state.insert(
        relative,
        OverlayEntry::File(OverlayFile {
            content: content.as_bytes().to_vec(),
            mtime: current_timestamp(),
        }),
    );
    Ok(MontyObject::Int(len))
}

/// Writes bytes content to the overlay.
fn overlay_write_bytes(
    state: &mut OverlayState,
    relative: String,
    content: &[u8],
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    overlay_check_parent_exists(state, &relative, ctx, vpath)?;
    let len = i64::try_from(content.len()).unwrap_or(i64::MAX);
    state.insert(
        relative,
        OverlayEntry::File(OverlayFile {
            content: content.to_vec(),
            mtime: current_timestamp(),
        }),
    );
    Ok(MontyObject::Int(len))
}

/// Checks that the parent directory of `relative` exists in the overlay or
/// real filesystem, matching CPython's `FileNotFoundError` behavior.
fn overlay_check_parent_exists(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<(), MountError> {
    if let Some(slash_pos) = relative.rfind('/') {
        let parent_rel = &relative[..slash_pos];
        let parent_exists = match state.get(parent_rel) {
            Some(OverlayEntry::Directory { .. }) => true,
            Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_) | OverlayEntry::Deleted) => false,
            None => {
                let parent_vpath = format!("{}/{parent_rel}", ctx.mount_virtual);
                resolve_path(&parent_vpath, ctx.mount_virtual, ctx.mount_host, false)
                    .is_ok_and(|r| r.host_path.is_dir())
            }
        };
        if !parent_exists {
            return Err(MountError::not_found(vpath));
        }
    }
    Ok(())
}

/// Creates a directory in the overlay.
fn overlay_mkdir(
    state: &mut OverlayState,
    relative: &str,
    parents: bool,
    exist_ok: bool,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => {
            return if exist_ok {
                Ok(MontyObject::None)
            } else {
                Err(MountError::io_err(ErrorKind::AlreadyExists, "File exists", vpath))
            };
        }
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
            return Err(MountError::io_err(ErrorKind::AlreadyExists, "File exists", vpath));
        }
        Some(OverlayEntry::Deleted) => { /* path was deleted, we can re-create */ }
        None => {
            // Check real FS.
            if let Ok(r) = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)
                && r.host_path.exists()
            {
                return if exist_ok {
                    Ok(MontyObject::None)
                } else {
                    Err(MountError::io_err(ErrorKind::AlreadyExists, "File exists", vpath))
                };
            }
        }
    }

    if parents {
        let mut current = String::new();
        for part in relative.split('/') {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(part);
            match state.get(&current) {
                Some(OverlayEntry::Directory { .. }) => {
                    // Already a directory, nothing to do.
                }
                Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
                    // A file blocks directory creation — NotADirectoryError.
                    let current_vpath = format!("{}/{current}", ctx.mount_virtual);
                    return Err(MountError::io_err(ErrorKind::Other, "Not a directory", &current_vpath));
                }
                Some(OverlayEntry::Deleted) => {
                    // Tombstoned — re-create as a directory (matches POSIX mkdir -p).
                    state.insert(
                        current.clone(),
                        OverlayEntry::Directory {
                            mtime: current_timestamp(),
                        },
                    );
                }
                None => {
                    // Check real FS — a real file blocks creation.
                    let check_vpath = format!("{}/{current}", ctx.mount_virtual);
                    if let Ok(r) = resolve_path(&check_vpath, ctx.mount_virtual, ctx.mount_host, false) {
                        if r.host_path.is_file() {
                            return Err(MountError::io_err(ErrorKind::Other, "Not a directory", &check_vpath));
                        }
                        if r.host_path.is_dir() {
                            // Real dir exists, no need to insert overlay entry.
                            continue;
                        }
                    }
                    state.insert(
                        current.clone(),
                        OverlayEntry::Directory {
                            mtime: current_timestamp(),
                        },
                    );
                }
            }
        }
    } else {
        // Without parents, the parent directory must exist (in overlay or real FS).
        if let Some(slash_pos) = relative.rfind('/') {
            let parent_rel = &relative[..slash_pos];
            let parent_exists = match state.get(parent_rel) {
                Some(OverlayEntry::Directory { .. }) => true,
                Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_) | OverlayEntry::Deleted) => false,
                None => {
                    // Check real FS.
                    let parent_vpath = format!("{}/{parent_rel}", ctx.mount_virtual);
                    resolve_path(&parent_vpath, ctx.mount_virtual, ctx.mount_host, false)
                        .is_ok_and(|r| r.host_path.is_dir())
                }
            };
            if !parent_exists {
                return Err(MountError::not_found(vpath));
            }
        }
        state.insert(
            relative.to_owned(),
            OverlayEntry::Directory {
                mtime: current_timestamp(),
            },
        );
    }

    Ok(MontyObject::None)
}

/// Deletes a file in the overlay (adds a tombstone).
fn overlay_unlink(
    state: &mut OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
            state.insert(relative.to_owned(), OverlayEntry::Deleted);
            Ok(MontyObject::None)
        }
        Some(OverlayEntry::Directory { .. }) => Err(MountError::io_err(ErrorKind::Other, "Is a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            if r.host_path.is_file() {
                state.insert(relative.to_owned(), OverlayEntry::Deleted);
                Ok(MontyObject::None)
            } else if r.host_path.is_dir() {
                Err(MountError::io_err(ErrorKind::Other, "Is a directory", vpath))
            } else {
                Err(MountError::not_found(vpath))
            }
        }
    }
}

/// Removes a directory in the overlay (adds a tombstone).
fn overlay_rmdir(
    state: &mut OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => {
            let prefix = if relative.is_empty() {
                String::new()
            } else {
                format!("{relative}/")
            };
            let has_children = state
                .prefix_iter(&prefix)
                .any(|(k, v)| k != relative && !matches!(v, OverlayEntry::Deleted));
            if has_children {
                return Err(MountError::io_err(
                    ErrorKind::DirectoryNotEmpty,
                    "Directory not empty",
                    vpath,
                ));
            }
            state.insert(relative.to_owned(), OverlayEntry::Deleted);
            Ok(MontyObject::None)
        }
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
            Err(MountError::io_err(ErrorKind::Other, "Not a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            if r.host_path.is_dir() {
                // Check that the real directory is empty (no real children that
                // aren't tombstoned in the overlay).
                if let Ok(entries) = fs::read_dir(&r.host_path) {
                    let prefix = if relative.is_empty() {
                        String::new()
                    } else {
                        format!("{relative}/")
                    };
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let child_rel = if prefix.is_empty() {
                            name
                        } else {
                            format!("{prefix}{name}")
                        };
                        if !matches!(state.get(&child_rel), Some(OverlayEntry::Deleted)) {
                            return Err(MountError::io_err(
                                ErrorKind::DirectoryNotEmpty,
                                "Directory not empty",
                                vpath,
                            ));
                        }
                    }
                }
                state.insert(relative.to_owned(), OverlayEntry::Deleted);
                Ok(MontyObject::None)
            } else {
                Err(MountError::not_found(vpath))
            }
        }
    }
}

/// Gets file status from the overlay or real filesystem.
fn overlay_stat(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(f)) => {
            let size = i64::try_from(f.content.len()).unwrap_or(i64::MAX);
            Ok(file_stat(0o644, size, f.mtime))
        }
        Some(OverlayEntry::RealFileRef(r)) => Ok(file_stat(0o644, r.size, r.mtime)),
        Some(OverlayEntry::Directory { mtime }) => Ok(dir_stat(0o755, *mtime)),
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            stat_fs(&r.host_path, vpath)
        }
    }
}

/// Lists directory contents, merging overlay entries with real filesystem entries.
fn overlay_iterdir(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    // Check if the directory itself is tombstoned or doesn't exist.
    // Cache the resolved host path to avoid resolving twice.
    let mut resolved_host: Option<PathBuf> = None;
    let real_dir_exists = match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
            return Err(MountError::io_err(ErrorKind::Other, "Not a directory", vpath));
        }
        Some(OverlayEntry::Deleted) => false,
        None => match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
            Ok(r) => {
                let is_dir = r.host_path.is_dir();
                resolved_host = Some(r.host_path);
                is_dir
            }
            Err(MountError::Io(_, _)) => false,
            Err(e) => return Err(e),
        },
    };

    let prefix = if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    };

    // Collect overlay children.
    let mut seen_names: AHashSet<String> = AHashSet::new();
    let mut entries: Vec<MontyObject> = Vec::new();

    for (path, entry) in state.prefix_iter(&prefix) {
        let rest = &path[prefix.len()..];
        if rest.contains('/') || rest.is_empty() {
            continue;
        }
        let child_name = rest.to_owned();

        seen_names.insert(child_name.clone());
        if !matches!(entry, OverlayEntry::Deleted) {
            entries.push(MontyObject::Path(format_child_path(vpath, &child_name)));
        }
    }

    // Merge real entries if directory exists and isn't tombstoned.
    if real_dir_exists
        && let Some(ref host_path) = resolved_host
        && let Ok(read_dir) = fs::read_dir(host_path)
    {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !seen_names.contains(&name) {
                entries.push(MontyObject::Path(format_child_path(vpath, &name)));
            }
        }
    }

    Ok(MontyObject::List(entries))
}

/// Renames a file or directory within the overlay.
///
/// For real-FS files, creates [`OverlayEntry::RealFileRef`] entries that store
/// only the host path rather than reading file content into memory. This avoids
/// unbounded memory consumption when renaming directories containing large files.
/// Original file timestamps are preserved rather than being replaced with "now".
fn overlay_rename(
    state: &mut OverlayState,
    src_vpath: &str,
    dst_vpath: &str,
    ctx: &MountContext<'_>,
) -> Result<MontyObject, MountError> {
    let src_norm = normalize_virtual_path(src_vpath);
    let dst_norm = normalize_virtual_path(dst_vpath);

    let src_rel = strip_mount_prefix(&src_norm, ctx.mount_virtual)
        .ok_or_else(|| MountError::NoMountPoint(src_vpath.to_owned()))?
        .to_owned();
    let dst_rel = strip_mount_prefix(&dst_norm, ctx.mount_virtual)
        .ok_or_else(|| MountError::CrossMountRename {
            src: src_vpath.to_owned(),
            dst: dst_vpath.to_owned(),
        })?
        .to_owned();

    // Verify the destination's parent directory exists (matches POSIX rename semantics).
    overlay_check_parent_exists(state, &dst_rel, ctx, dst_vpath)?;

    // Check for tombstone before removing — a deleted source is an error.
    if matches!(state.get(&src_rel), Some(OverlayEntry::Deleted)) {
        return Err(MountError::not_found(src_vpath));
    }

    // Move the source entry: remove from old key, insert at new key.
    // For entries already in the overlay, this is a zero-copy move.
    // For real-FS entries (None), create a lazy RealFileRef.
    let entry = if let Some(entry) = state.remove(&src_rel) {
        entry
    } else {
        // Source is on the real FS — create a lazy reference.
        let r = resolve_path(src_vpath, ctx.mount_virtual, ctx.mount_host, false)?;
        if r.host_path.is_file() {
            OverlayFileRef::from_host_path(&r.host_path)
                .map(OverlayEntry::RealFileRef)
                .ok_or_else(|| MountError::not_found(src_vpath))?
        } else if r.host_path.is_dir() {
            let mtime = dir_mtime(&r.host_path);
            OverlayEntry::Directory { mtime }
        } else {
            return Err(MountError::not_found(src_vpath));
        }
    };

    // For directories, collect and re-key all descendant entries.
    let mut descendants: Vec<(String, OverlayEntry)> = Vec::new();
    let mut tombstone_keys: Vec<String> = Vec::new();

    if matches!(entry, OverlayEntry::Directory { .. }) {
        let src_prefix = format!("{src_rel}/");
        let dst_prefix = format!("{dst_rel}/");

        // Collect overlay descendant keys first (can't mutate while iterating).
        let child_keys: Vec<String> = state.prefix_iter(&src_prefix).map(|(k, _)| k.to_owned()).collect();

        // Track keys that were already in the overlay so collect_real_descendants
        // skips them (they've been removed from state but shouldn't be re-created).
        let handled_keys: AHashSet<String> = child_keys.iter().cloned().collect();

        // Remove each child and re-key it — zero-copy move of owned entries.
        for key in child_keys {
            let suffix = &key[src_prefix.len()..];
            if let Some(child) = state.remove(&key) {
                descendants.push((format!("{dst_prefix}{suffix}"), child));
                // Tombstone the old key so real FS entries don't show through.
                tombstone_keys.push(key);
            }
        }

        // Create lazy references for real-FS children that aren't already in
        // the overlay, so they appear under the renamed directory without
        // reading file content into memory.
        if let Ok(r) = resolve_path(src_vpath, ctx.mount_virtual, ctx.mount_host, false)
            && let Ok(real_children) = collect_real_descendants(&r.host_path, &src_prefix, state, &handled_keys)
        {
            for (old_rel, child_entry) in real_children {
                let suffix = old_rel.strip_prefix(&src_prefix).unwrap_or(&old_rel);
                descendants.push((format!("{dst_prefix}{suffix}"), child_entry));
                tombstone_keys.push(old_rel);
            }
        }
    }

    // Tombstone the source so real FS entries don't show through at the old path.
    // For overlay-only entries that were removed above, we still need the tombstone
    // in case a real FS entry exists at the same path.
    state.insert(src_rel, OverlayEntry::Deleted);
    state.insert(dst_rel, entry);

    for key in tombstone_keys {
        state.insert(key, OverlayEntry::Deleted);
    }
    for (key, child) in descendants {
        state.insert(key, child);
    }

    Ok(MontyObject::None)
}

/// Recursively collects real-FS children of a directory that aren't already
/// in the overlay, returning `(relative_key, OverlayEntry)` pairs.
///
/// Creates [`OverlayEntry::RealFileRef`] entries for files (lazy — no content
/// read) and [`OverlayEntry::Directory`] entries for subdirectories, preserving
/// the original filesystem timestamps.
///
/// `already_handled` contains keys that were previously in the overlay (and have
/// since been removed for re-keying). These are skipped to avoid re-creating
/// entries that the caller is already moving to a new path.
fn collect_real_descendants(
    host_dir: &Path,
    prefix: &str,
    state: &OverlayState,
    already_handled: &AHashSet<String>,
) -> io::Result<Vec<(String, OverlayEntry)>> {
    let mut result = Vec::new();
    let mut dirs = vec![(host_dir.to_path_buf(), prefix.to_owned())];

    while let Some((dir, rel_prefix)) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let rel_key = format!("{rel_prefix}{name_str}");

            // Skip entries already in the overlay or already handled by the caller.
            if state.get(&rel_key).is_some() || already_handled.contains(&rel_key) {
                continue;
            }

            let ft = entry.file_type()?;
            if ft.is_file() {
                if let Some(file_ref) = OverlayFileRef::from_host_path(&entry.path()) {
                    result.push((rel_key, OverlayEntry::RealFileRef(file_ref)));
                }
            } else if ft.is_dir() {
                let mtime = dir_mtime(&entry.path());
                result.push((rel_key.clone(), OverlayEntry::Directory { mtime }));
                dirs.push((entry.path(), format!("{rel_key}/")));
            }
        }
    }

    Ok(result)
}

// =============================================================================
// Shared filesystem primitives
// =============================================================================

/// Reads a file as UTF-8 text.
///
/// Uses [`fs::read`] followed by [`String::from_utf8`] so that invalid-UTF-8
/// errors produce [`MountError::InvalidUtf8`] (→ `UnicodeDecodeError`) rather
/// than a generic `OSError`.
fn read_text_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    let bytes = fs::read(path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    let content = bytes_to_utf8(bytes)?;
    Ok(MontyObject::String(content))
}

/// Reads a file as raw bytes.
fn read_bytes_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    let content = fs::read(path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    Ok(MontyObject::Bytes(content))
}

/// Writes text to a file, returning the number of characters written.
///
/// Returns the character count (not byte length) to match CPython's
/// `Path.write_text()` behavior. For ASCII text these are identical,
/// but they differ for multi-byte UTF-8 characters.
fn write_text_fs(path: &Path, content: &str, vpath: &str) -> Result<MontyObject, MountError> {
    fs::write(path, content).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    Ok(MontyObject::Int(
        i64::try_from(content.chars().count()).unwrap_or(i64::MAX),
    ))
}

/// Writes bytes to a file, returning the number of bytes written.
fn write_bytes_fs(path: &Path, content: &[u8], vpath: &str) -> Result<MontyObject, MountError> {
    fs::write(path, content).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    Ok(MontyObject::Int(i64::try_from(content.len()).unwrap_or(i64::MAX)))
}

/// Creates a directory.
fn mkdir_fs(path: &Path, parents: bool, exist_ok: bool, vpath: &str) -> Result<MontyObject, MountError> {
    let result = if parents {
        fs::create_dir_all(path)
    } else {
        fs::create_dir(path)
    };
    match result {
        Ok(()) => Ok(MontyObject::None),
        Err(e) if e.kind() == ErrorKind::AlreadyExists && exist_ok => Ok(MontyObject::None),
        Err(e) => Err(MountError::Io(e, vpath.to_owned())),
    }
}

/// Removes a file.
fn unlink_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    fs::remove_file(path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    Ok(MontyObject::None)
}

/// Removes an empty directory.
fn rmdir_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    fs::remove_dir(path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    Ok(MontyObject::None)
}

/// Gets file metadata and returns a `stat_result` named tuple.
fn stat_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    let metadata = fs::metadata(path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    let mtime = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);

    // `fs::metadata` follows symlinks, so `is_symlink()` is always false here.
    // Symlink detection is handled separately via `OsFunction::IsSymlink`.
    if metadata.is_dir() {
        Ok(dir_stat(0o755, mtime))
    } else {
        Ok(file_stat(0o644, size, mtime))
    }
}

/// Lists directory contents directly from the host filesystem.
fn iterdir_fs(host_path: &Path, vpath: &str, mount_host_path: &Path) -> Result<MontyObject, MountError> {
    let read_dir = fs::read_dir(host_path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    let mut result = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|e| MountError::Io(e, vpath.to_owned()))?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Verify each entry is within mount boundary (defense in depth).
        if let Ok(canonical) = fs::canonicalize(entry.path())
            && !canonical.starts_with(mount_host_path)
        {
            continue;
        }

        result.push(MontyObject::Path(format_child_path(vpath, &name)));
    }

    Ok(MontyObject::List(result))
}

/// Renames a file or directory on the real filesystem.
fn rename_fs(src_vpath: &str, dst_vpath: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let src = resolve_path(src_vpath, ctx.mount_virtual, ctx.mount_host, false)?;
    let dst = resolve_path(dst_vpath, ctx.mount_virtual, ctx.mount_host, true)?;
    fs::rename(&src.host_path, &dst.host_path).map_err(|e| MountError::Io(e, src_vpath.to_owned()))?;
    Ok(MontyObject::None)
}

// =============================================================================
// Argument extraction helpers
// =============================================================================

/// Extracts the string content argument for `write_text`.
fn extract_string_arg<'a>(extra_args: &'a [MontyObject], op_name: &str) -> Result<&'a str, MountError> {
    match extra_args.first() {
        Some(MontyObject::String(s)) => Ok(s.as_str()),
        Some(a) => Err(MountError::InvalidMount(format!(
            "data must be str, not {}",
            a.type_name()
        ))),
        None => Err(MountError::InvalidMount(format!(
            "Path.{op_name}() missing 1 required positional argument: 'data'"
        ))),
    }
}

/// Extracts the bytes content argument for `write_bytes`.
fn extract_bytes_arg<'a>(extra_args: &'a [MontyObject], op_name: &str) -> Result<&'a [u8], MountError> {
    match extra_args.first() {
        Some(MontyObject::Bytes(b)) => Ok(b.as_slice()),
        Some(a) => Err(MountError::InvalidMount(format!(
            "memoryview: a bytes-like object is required, not '{}'",
            a.type_name()
        ))),
        None => Err(MountError::InvalidMount(format!(
            "Path.{op_name}() missing 1 required positional argument: 'data'"
        ))),
    }
}

/// Extracts the path argument (e.g., rename target).
fn extract_path_arg(extra_args: &[MontyObject], op_name: &str) -> Result<String, MountError> {
    match extra_args.first() {
        Some(MontyObject::Path(p)) => Ok(p.clone()),
        Some(MontyObject::String(s)) => Ok(s.clone()),
        _ => Err(MountError::InvalidMount(format!("{op_name}: expected path argument"))),
    }
}

/// Extracts `parents` and `exist_ok` keyword arguments for mkdir.
fn extract_mkdir_kwargs(kwargs: &[(MontyObject, MontyObject)]) -> (bool, bool) {
    let mut parents = false;
    let mut exist_ok = false;
    for (key, value) in kwargs {
        if let (MontyObject::String(k), MontyObject::Bool(v)) = (key, value) {
            match k.as_str() {
                "parents" => parents = *v,
                "exist_ok" => exist_ok = *v,
                _ => {}
            }
        }
    }
    (parents, exist_ok)
}

// =============================================================================
// Write limit check
// =============================================================================

/// Checks whether writing `bytes` would exceed the configured limit, and if
/// not, increments the cumulative counter in the mount context.
///
/// Returns `Ok(())` when no limit is set or the write fits within the limit.
fn check_write_limit(bytes: usize, ctx: &mut MountContext<'_>) -> Result<(), MountError> {
    if let Some(limit) = ctx.write_bytes_limit {
        let bytes = bytes as u64;
        if *ctx.write_bytes_used + bytes > limit {
            return Err(MountError::WriteLimitExceeded(limit));
        }
        *ctx.write_bytes_used += bytes;
    }
    Ok(())
}

// =============================================================================
// Utility functions
// =============================================================================

/// Converts raw bytes to a UTF-8 string, producing a [`MountError::InvalidUtf8`]
/// with the position and value of the first invalid byte on failure.
fn bytes_to_utf8(bytes: Vec<u8>) -> Result<String, MountError> {
    String::from_utf8(bytes).map_err(|e| {
        let position = e.utf8_error().valid_up_to();
        let invalid_byte = e.into_bytes()[position];
        MountError::InvalidUtf8 { position, invalid_byte }
    })
}

/// Returns the current Unix timestamp as seconds since epoch.
fn current_timestamp() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Reads the modification time of a directory from the host filesystem,
/// falling back to the current time if metadata is unavailable.
fn dir_mtime(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now())
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Constructs a child virtual path from a parent and child name.
fn format_child_path(parent: &str, child: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{child}")
    } else {
        format!("{parent}/{child}")
    }
}
