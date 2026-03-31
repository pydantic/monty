//! Filesystem operation handlers for each mount mode.
//!
//! Each [`OsFunction`] filesystem variant is dispatched here based on the [`MountMode`]:
//!
//! - **`ReadWrite`**: direct pass-through to [`std::fs`] on the resolved host path
//! - **`ReadOnly`**: read ops pass through; write ops return [`MountError::ReadOnly`]
//! - **`OverlayMemory`**: reads check overlay first then fall through; writes go to overlay

use std::{
    collections::HashSet,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    time::SystemTime,
};

use super::{
    error::MountError,
    mount_mode::{MountMode, OverlayEntry, OverlayFile, OverlayState},
    path_security::{normalize_virtual_path, resolve_path, resolve_path_mkdir_parents, strip_mount_prefix},
};
use crate::{MontyObject, dir_stat, file_stat, os::OsFunction, symlink_stat};

/// Mount-specific context passed through the operation call chain.
pub(super) struct MountContext<'a> {
    /// The virtual path prefix of the mount (e.g., `/data`).
    pub mount_virtual: &'a str,
    /// The canonical host directory backing the mount.
    pub mount_host: &'a Path,
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
    ctx: &MountContext<'_>,
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
    ctx: &MountContext<'_>,
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
        OsFunction::WriteText => write_text_fs(&host, extract_string_arg(extra_args, "write_text")?, vpath),
        OsFunction::WriteBytes => write_bytes_fs(&host, extract_bytes_arg(extra_args, "write_bytes")?, vpath),
        OsFunction::Mkdir => {
            let (parents, exist_ok) = extract_mkdir_kwargs(kwargs);
            mkdir_fs(&host, parents, exist_ok, vpath)
        }
        OsFunction::Unlink => unlink_fs(&host, vpath),
        OsFunction::Rmdir => rmdir_fs(&host, vpath),
        OsFunction::Stat => stat_fs(&host, vpath),
        OsFunction::Iterdir => iterdir_fs(&host, vpath, ctx.mount_host),
        OsFunction::Rename => rename_fs(vpath, &extract_path_arg(extra_args, "rename")?, ctx),
        _ => Err(MountError::NoMountPoint(vpath.to_owned())),
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
    ctx: &MountContext<'_>,
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
            overlay_write_text(state, relative, content, ctx, vpath)
        }
        OsFunction::WriteBytes => {
            let content = extract_bytes_arg(extra_args, "write_bytes")?;
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
        _ => Err(MountError::NoMountPoint(vpath.to_owned())),
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
        Some(OverlayEntry::File(_) | OverlayEntry::Directory { .. }) => Ok(MontyObject::Bool(true)),
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
        Some(OverlayEntry::File(_)) => Ok(MontyObject::Bool(true)),
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
        Some(OverlayEntry::File(_) | OverlayEntry::Deleted) => Ok(MontyObject::Bool(false)),
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
        None => match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
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
        Some(OverlayEntry::Directory { .. }) => Err(MountError::io_err(ErrorKind::Other, "Is a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            read_bytes_fs(&r.host_path, vpath)
        }
    }
}

// --- Overlay write operations ---

/// Writes text content to the overlay.
fn overlay_write_text(
    state: &mut OverlayState,
    relative: String,
    content: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    overlay_check_parent_exists(state, &relative, ctx, vpath)?;
    let len = i64::try_from(content.len()).unwrap_or(i64::MAX);
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
            Some(OverlayEntry::File(_) | OverlayEntry::Deleted) => false,
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
        Some(OverlayEntry::File(_)) => {
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
            if state.get(&current).is_none() {
                state.insert(
                    current.clone(),
                    OverlayEntry::Directory {
                        mtime: current_timestamp(),
                    },
                );
            }
        }
    } else {
        // Without parents, the parent directory must exist (in overlay or real FS).
        if let Some(slash_pos) = relative.rfind('/') {
            let parent_rel = &relative[..slash_pos];
            let parent_exists = match state.get(parent_rel) {
                Some(OverlayEntry::Directory { .. }) => true,
                Some(OverlayEntry::File(_) | OverlayEntry::Deleted) => false,
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
        Some(OverlayEntry::File(_)) => {
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
        Some(OverlayEntry::File(_)) => Err(MountError::io_err(ErrorKind::Other, "Not a directory", vpath)),
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
    let real_dir_exists = match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::File(_)) => return Err(MountError::io_err(ErrorKind::Other, "Not a directory", vpath)),
        Some(OverlayEntry::Deleted) => false,
        None => match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
            Ok(r) => r.host_path.is_dir(),
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
    let mut seen_names: HashSet<String> = HashSet::new();
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
        && let Ok(r) = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)
        && let Ok(read_dir) = fs::read_dir(&r.host_path)
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

    // Read the source content.
    let entry = match state.get(&src_rel) {
        Some(OverlayEntry::File(f)) => OverlayEntry::File(OverlayFile {
            content: f.content.clone(),
            mtime: current_timestamp(),
        }),
        Some(OverlayEntry::Directory { .. }) => OverlayEntry::Directory {
            mtime: current_timestamp(),
        },
        Some(OverlayEntry::Deleted) => return Err(MountError::not_found(src_vpath)),
        None => {
            let r = resolve_path(src_vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            if r.host_path.is_file() {
                let content = fs::read(&r.host_path).map_err(|e| MountError::Io(e, src_vpath.to_owned()))?;
                OverlayEntry::File(OverlayFile {
                    content,
                    mtime: current_timestamp(),
                })
            } else if r.host_path.is_dir() {
                OverlayEntry::Directory {
                    mtime: current_timestamp(),
                }
            } else {
                return Err(MountError::not_found(src_vpath));
            }
        }
    };

    // Collect descendant entries to move along with the directory.
    // For a directory, all overlay entries under `src_rel/` must be re-keyed
    // to `dst_rel/`, and real-FS children must be tombstoned at the old path.
    let mut descendants: Vec<(String, OverlayEntry)> = Vec::new();
    let mut tombstones: Vec<String> = Vec::new();

    if matches!(entry, OverlayEntry::Directory { .. }) {
        let src_prefix = format!("{src_rel}/");
        let dst_prefix = format!("{dst_rel}/");

        // Move overlay descendants to the new prefix.
        for (key, child) in state.prefix_iter(&src_prefix) {
            let suffix = &key[src_prefix.len()..];
            match child {
                OverlayEntry::File(f) => {
                    descendants.push((
                        format!("{dst_prefix}{suffix}"),
                        OverlayEntry::File(OverlayFile {
                            content: f.content.clone(),
                            mtime: current_timestamp(),
                        }),
                    ));
                }
                OverlayEntry::Directory { .. } => {
                    descendants.push((
                        format!("{dst_prefix}{suffix}"),
                        OverlayEntry::Directory {
                            mtime: current_timestamp(),
                        },
                    ));
                }
                OverlayEntry::Deleted => {
                    descendants.push((format!("{dst_prefix}{suffix}"), OverlayEntry::Deleted));
                }
            }
            tombstones.push(key.to_owned());
        }

        // Tombstone real-FS children that aren't already in the overlay so they
        // don't "show through" at the old path after the rename.
        if let Ok(r) = resolve_path(src_vpath, ctx.mount_virtual, ctx.mount_host, false)
            && let Ok(iter) = collect_real_descendants(&r.host_path, &src_prefix, state)
        {
            for (old_rel, child_entry) in iter {
                let suffix = old_rel.strip_prefix(&src_prefix).unwrap_or(&old_rel);
                descendants.push((format!("{dst_prefix}{suffix}"), child_entry));
                tombstones.push(old_rel);
            }
        }
    }

    state.insert(src_rel, OverlayEntry::Deleted);
    state.insert(dst_rel, entry);

    for key in tombstones {
        state.insert(key, OverlayEntry::Deleted);
    }
    for (key, entry) in descendants {
        state.insert(key, entry);
    }

    Ok(MontyObject::None)
}

/// Recursively collects real-FS children of a directory that aren't already
/// in the overlay, returning `(relative_key, OverlayEntry)` pairs.
///
/// Used by `overlay_rename` to copy real-FS descendants into the overlay at the
/// new path so they appear under the renamed directory.
fn collect_real_descendants(
    host_dir: &Path,
    prefix: &str,
    state: &OverlayState,
) -> io::Result<Vec<(String, OverlayEntry)>> {
    let mut result = Vec::new();
    let mut dirs = vec![(host_dir.to_path_buf(), prefix.to_owned())];

    while let Some((dir, rel_prefix)) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let rel_key = format!("{rel_prefix}{name_str}");

            // Skip entries already in the overlay — they were handled above.
            if state.get(&rel_key).is_some() {
                continue;
            }

            let ft = entry.file_type()?;
            if ft.is_file() {
                let content = fs::read(entry.path())?;
                result.push((
                    rel_key,
                    OverlayEntry::File(OverlayFile {
                        content,
                        mtime: current_timestamp(),
                    }),
                ));
            } else if ft.is_dir() {
                result.push((
                    rel_key.clone(),
                    OverlayEntry::Directory {
                        mtime: current_timestamp(),
                    },
                ));
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
fn write_text_fs(path: &Path, content: &str, vpath: &str) -> Result<MontyObject, MountError> {
    fs::write(path, content).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    Ok(MontyObject::Int(i64::try_from(content.len()).unwrap_or(i64::MAX)))
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

    if metadata.is_dir() {
        Ok(dir_stat(0o755, mtime))
    } else if metadata.is_symlink() {
        Ok(symlink_stat(0o777, mtime))
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

/// Constructs a child virtual path from a parent and child name.
fn format_child_path(parent: &str, child: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{child}")
    } else {
        format!("{parent}/{child}")
    }
}
