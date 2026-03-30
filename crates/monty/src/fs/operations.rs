//! Filesystem operation handlers for each mount mode.
//!
//! Each [`OsFunction`] filesystem variant is dispatched here based on the [`MountMode`]:
//!
//! - **`ReadWrite`**: direct pass-through to [`std::fs`] on the resolved host path
//! - **`ReadOnly`**: read ops pass through; write ops return [`MountError::ReadOnly`]
//! - **`OverlayMemory`**: reads check overlay first then fall through; writes go to overlay
//! - **`OverlayDirectory`**: reads check upper dir first then lower; writes go to upper dir

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
    path_security::{normalize_virtual_path, resolve_path, strip_mount_prefix},
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
        MountMode::OverlayDirectory { upper_dir } => {
            execute_overlay_directory(function, virtual_path, extra_args, kwargs, ctx, upper_dir)
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
        // Build the host path directly from the (already-canonical) mount root.
        let normalized = normalize_virtual_path(vpath);
        let relative = strip_mount_prefix(&normalized, ctx.mount_virtual)
            .ok_or_else(|| MountError::NoMountPoint(vpath.to_owned()))?;
        Ok(if relative.is_empty() {
            ctx.mount_host.to_path_buf()
        } else {
            ctx.mount_host.join(relative)
        })
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
        OsFunction::Exists => ovl_exists(state, &relative, ctx, vpath),
        OsFunction::IsFile => ovl_is_file(state, &relative, ctx, vpath),
        OsFunction::IsDir => ovl_is_dir(state, &relative, ctx, vpath),
        OsFunction::IsSymlink => ovl_is_symlink(state, &relative, ctx, vpath),
        OsFunction::ReadText => ovl_read_text(state, &relative, ctx, vpath),
        OsFunction::ReadBytes => ovl_read_bytes(state, &relative, ctx, vpath),
        OsFunction::WriteText => {
            let content = extract_string_arg(extra_args, "write_text")?;
            ovl_write_text(state, relative, content)
        }
        OsFunction::WriteBytes => {
            let content = extract_bytes_arg(extra_args, "write_bytes")?;
            ovl_write_bytes(state, relative, content)
        }
        OsFunction::Mkdir => {
            let (parents, exist_ok) = extract_mkdir_kwargs(kwargs);
            ovl_mkdir(state, &relative, parents, exist_ok, ctx, vpath)
        }
        OsFunction::Unlink => ovl_unlink(state, &relative, ctx, vpath),
        OsFunction::Rmdir => ovl_rmdir(state, &relative, ctx, vpath),
        OsFunction::Stat => ovl_stat(state, &relative, ctx, vpath),
        OsFunction::Iterdir => ovl_iterdir(state, &relative, ctx, vpath),
        OsFunction::Rename => {
            let target = extract_path_arg(extra_args, "rename")?;
            ovl_rename(state, vpath, &target, ctx)
        }
        OsFunction::Resolve | OsFunction::Absolute => Ok(MontyObject::Path(normalize_virtual_path(vpath))),
        _ => Err(MountError::NoMountPoint(vpath.to_owned())),
    }
}

// --- Overlay read operations ---

/// Checks whether a path exists in the overlay or real filesystem.
fn ovl_exists(
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
fn ovl_is_file(
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
fn ovl_is_dir(
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
fn ovl_is_symlink(
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
fn ovl_read_text(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(f)) => {
            let text = String::from_utf8(f.content.clone())
                .map_err(|_| MountError::Io(io::Error::other("invalid UTF-8"), vpath.to_owned()))?;
            Ok(MontyObject::String(text))
        }
        Some(OverlayEntry::Directory { .. }) => Err(io_err(ErrorKind::Other, "Is a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            read_text_fs(&r.host_path, vpath)
        }
    }
}

/// Reads a file as bytes from the overlay or real filesystem.
fn ovl_read_bytes(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(f)) => Ok(MontyObject::Bytes(f.content.clone())),
        Some(OverlayEntry::Directory { .. }) => Err(io_err(ErrorKind::Other, "Is a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            read_bytes_fs(&r.host_path, vpath)
        }
    }
}

// --- Overlay write operations ---

/// Writes text content to the overlay.
#[expect(clippy::unnecessary_wraps)]
fn ovl_write_text(state: &mut OverlayState, relative: String, content: &str) -> Result<MontyObject, MountError> {
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
#[expect(clippy::unnecessary_wraps)]
fn ovl_write_bytes(state: &mut OverlayState, relative: String, content: &[u8]) -> Result<MontyObject, MountError> {
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

/// Creates a directory in the overlay.
fn ovl_mkdir(
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
                Err(io_err(ErrorKind::AlreadyExists, "File exists", vpath))
            };
        }
        Some(OverlayEntry::File(_)) => {
            return Err(io_err(ErrorKind::AlreadyExists, "File exists", vpath));
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
                    Err(io_err(ErrorKind::AlreadyExists, "File exists", vpath))
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
                return Err(not_found(vpath));
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
fn ovl_unlink(
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
        Some(OverlayEntry::Directory { .. }) => Err(io_err(ErrorKind::Other, "Is a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            if r.host_path.is_file() {
                state.insert(relative.to_owned(), OverlayEntry::Deleted);
                Ok(MontyObject::None)
            } else if r.host_path.is_dir() {
                Err(io_err(ErrorKind::Other, "Is a directory", vpath))
            } else {
                Err(not_found(vpath))
            }
        }
    }
}

/// Removes a directory in the overlay (adds a tombstone).
fn ovl_rmdir(
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
                .iter()
                .any(|(k, v)| k.starts_with(&prefix) && k != relative && !matches!(v, OverlayEntry::Deleted));
            if has_children {
                return Err(io_err(ErrorKind::Other, "Directory not empty", vpath));
            }
            state.insert(relative.to_owned(), OverlayEntry::Deleted);
            Ok(MontyObject::None)
        }
        Some(OverlayEntry::File(_)) => Err(io_err(ErrorKind::Other, "Not a directory", vpath)),
        Some(OverlayEntry::Deleted) => Err(not_found(vpath)),
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
                            return Err(io_err(ErrorKind::Other, "Directory not empty", vpath));
                        }
                    }
                }
                state.insert(relative.to_owned(), OverlayEntry::Deleted);
                Ok(MontyObject::None)
            } else {
                Err(not_found(vpath))
            }
        }
    }
}

/// Gets file status from the overlay or real filesystem.
fn ovl_stat(
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
        Some(OverlayEntry::Deleted) => Err(not_found(vpath)),
        None => {
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            stat_fs(&r.host_path, vpath)
        }
    }
}

/// Lists directory contents, merging overlay entries with real filesystem entries.
fn ovl_iterdir(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    // Check if the directory itself is tombstoned or doesn't exist.
    let real_dir_exists = match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::File(_)) => return Err(io_err(ErrorKind::Other, "Not a directory", vpath)),
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

    for (path, entry) in state.iter() {
        let child_name = if prefix.is_empty() {
            if path.contains('/') || path.is_empty() {
                continue;
            }
            path.to_owned()
        } else if let Some(rest) = path.strip_prefix(&prefix) {
            if rest.contains('/') || rest.is_empty() {
                continue;
            }
            rest.to_owned()
        } else {
            continue;
        };

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
fn ovl_rename(
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

    // Read the source content.
    let entry = match state.get(&src_rel) {
        Some(OverlayEntry::File(f)) => OverlayEntry::File(OverlayFile {
            content: f.content.clone(),
            mtime: current_timestamp(),
        }),
        Some(OverlayEntry::Directory { .. }) => OverlayEntry::Directory {
            mtime: current_timestamp(),
        },
        Some(OverlayEntry::Deleted) => return Err(not_found(src_vpath)),
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
                return Err(not_found(src_vpath));
            }
        }
    };

    state.insert(src_rel, OverlayEntry::Deleted);
    state.insert(dst_rel, entry);
    Ok(MontyObject::None)
}

// =============================================================================
// Overlay directory operations
// =============================================================================

/// Executes a filesystem operation with directory-backed overlay semantics.
fn execute_overlay_directory(
    function: OsFunction,
    vpath: &str,
    extra_args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    ctx: &MountContext<'_>,
    upper_dir: &Path,
) -> Result<MontyObject, MountError> {
    let normalized = normalize_virtual_path(vpath);
    let relative =
        strip_mount_prefix(&normalized, ctx.mount_virtual).ok_or_else(|| MountError::NoMountPoint(vpath.to_owned()))?;

    let upper_path = if relative.is_empty() {
        upper_dir.to_path_buf()
    } else {
        upper_dir.join(relative)
    };

    let is_whited_out = whiteout_path_for(&upper_path).is_some_and(|p| p.exists());

    match function {
        OsFunction::Exists => {
            if is_whited_out {
                return Ok(MontyObject::Bool(false));
            }
            if upper_path.exists() {
                return Ok(MontyObject::Bool(true));
            }
            Ok(MontyObject::Bool(
                resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false).is_ok_and(|r| r.host_path.exists()),
            ))
        }
        OsFunction::IsFile => ovl_dir_check(&upper_path, is_whited_out, Path::is_file, ctx, vpath),
        OsFunction::IsDir => ovl_dir_check(&upper_path, is_whited_out, Path::is_dir, ctx, vpath),
        OsFunction::IsSymlink => ovl_dir_check(&upper_path, is_whited_out, Path::is_symlink, ctx, vpath),
        OsFunction::ReadText => {
            if is_whited_out {
                return Err(not_found(vpath));
            }
            if upper_path.is_file() {
                return read_text_fs(&upper_path, vpath);
            }
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            read_text_fs(&r.host_path, vpath)
        }
        OsFunction::ReadBytes => {
            if is_whited_out {
                return Err(not_found(vpath));
            }
            if upper_path.is_file() {
                return read_bytes_fs(&upper_path, vpath);
            }
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            read_bytes_fs(&r.host_path, vpath)
        }
        OsFunction::Stat => {
            if is_whited_out {
                return Err(not_found(vpath));
            }
            if upper_path.exists() {
                return stat_fs(&upper_path, vpath);
            }
            let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
            stat_fs(&r.host_path, vpath)
        }
        OsFunction::Iterdir => iterdir_overlay_dir(&upper_path, ctx, vpath),

        // --- Write operations: go to upper dir ---
        OsFunction::WriteText => {
            let content = extract_string_arg(extra_args, "write_text")?;
            remove_whiteout(&upper_path);
            ensure_parent(&upper_path, vpath)?;
            write_text_fs(&upper_path, content, vpath)
        }
        OsFunction::WriteBytes => {
            let content = extract_bytes_arg(extra_args, "write_bytes")?;
            remove_whiteout(&upper_path);
            ensure_parent(&upper_path, vpath)?;
            write_bytes_fs(&upper_path, content, vpath)
        }
        OsFunction::Mkdir => {
            let (parents, exist_ok) = extract_mkdir_kwargs(kwargs);
            remove_whiteout(&upper_path);
            if parents {
                fs::create_dir_all(&upper_path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
            } else if exist_ok && upper_path.is_dir() {
                // Already exists.
            } else {
                fs::create_dir(&upper_path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
            }
            Ok(MontyObject::None)
        }
        OsFunction::Unlink => {
            if upper_path.is_file() {
                fs::remove_file(&upper_path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
            }
            create_whiteout(&upper_path, vpath)?;
            Ok(MontyObject::None)
        }
        OsFunction::Rmdir => {
            if upper_path.is_dir() {
                fs::remove_dir(&upper_path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
            }
            create_whiteout(&upper_path, vpath)?;
            Ok(MontyObject::None)
        }
        OsFunction::Rename => {
            let target_vpath = extract_path_arg(extra_args, "rename")?;
            let target_norm = normalize_virtual_path(&target_vpath);
            let target_rel =
                strip_mount_prefix(&target_norm, ctx.mount_virtual).ok_or_else(|| MountError::CrossMountRename {
                    src: vpath.to_owned(),
                    dst: target_vpath.clone(),
                })?;
            let target_upper = if target_rel.is_empty() {
                upper_dir.to_path_buf()
            } else {
                upper_dir.join(target_rel)
            };

            if upper_path.exists() {
                ensure_parent(&target_upper, vpath)?;
                fs::rename(&upper_path, &target_upper).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
            } else {
                let r = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)?;
                ensure_parent(&target_upper, vpath)?;
                fs::copy(&r.host_path, &target_upper).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
            }
            create_whiteout(&upper_path, vpath)?;
            remove_whiteout(&target_upper);
            Ok(MontyObject::None)
        }

        OsFunction::Resolve | OsFunction::Absolute => Ok(MontyObject::Path(normalize_virtual_path(vpath))),
        _ => Err(MountError::NoMountPoint(vpath.to_owned())),
    }
}

/// Shared logic for `is_file`, `is_dir`, `is_symlink` in overlay directory mode.
fn ovl_dir_check(
    upper_path: &Path,
    is_whited_out: bool,
    check_fn: fn(&Path) -> bool,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    if is_whited_out {
        return Ok(MontyObject::Bool(false));
    }
    if upper_path.exists() {
        return Ok(MontyObject::Bool(check_fn(upper_path)));
    }
    match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false) {
        Ok(r) => Ok(MontyObject::Bool(check_fn(&r.host_path))),
        Err(MountError::Io(_, _)) => Ok(MontyObject::Bool(false)),
        Err(e) => Err(e),
    }
}

/// Lists directory contents for overlay-directory mode, merging upper and lower.
#[expect(clippy::unnecessary_wraps)]
fn iterdir_overlay_dir(upper_path: &Path, ctx: &MountContext<'_>, vpath: &str) -> Result<MontyObject, MountError> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries: Vec<MontyObject> = Vec::new();
    let mut whiteouts: HashSet<String> = HashSet::new();

    if upper_path.is_dir()
        && let Ok(read_dir) = fs::read_dir(upper_path)
    {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(original) = name.strip_prefix(".wh.") {
                whiteouts.insert(original.to_owned());
            } else {
                seen.insert(name.clone());
                entries.push(MontyObject::Path(format_child_path(vpath, &name)));
            }
        }
    }

    if let Ok(r) = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, false)
        && let Ok(read_dir) = fs::read_dir(&r.host_path)
    {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !seen.contains(&name) && !whiteouts.contains(&name) {
                entries.push(MontyObject::Path(format_child_path(vpath, &name)));
            }
        }
    }

    Ok(MontyObject::List(entries))
}

// =============================================================================
// Shared filesystem primitives
// =============================================================================

/// Reads a file as UTF-8 text.
fn read_text_fs(path: &Path, vpath: &str) -> Result<MontyObject, MountError> {
    let content = fs::read_to_string(path).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
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
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
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
        _ => Err(MountError::InvalidMount(format!("{op_name}: expected string argument"))),
    }
}

/// Extracts the bytes content argument for `write_bytes`.
fn extract_bytes_arg<'a>(extra_args: &'a [MontyObject], op_name: &str) -> Result<&'a [u8], MountError> {
    match extra_args.first() {
        Some(MontyObject::Bytes(b)) => Ok(b.as_slice()),
        _ => Err(MountError::InvalidMount(format!("{op_name}: expected bytes argument"))),
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

/// Returns the current Unix timestamp as seconds since epoch.
fn current_timestamp() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Constructs a child virtual path from a parent and child name.
fn format_child_path(parent: &str, child: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{child}")
    } else {
        format!("{parent}/{child}")
    }
}

/// Creates a `MountError::Io` with a constructed `io::Error`.
fn io_err(kind: ErrorKind, msg: &str, vpath: &str) -> MountError {
    MountError::Io(io::Error::new(kind, msg), vpath.to_owned())
}

/// Shorthand for a "not found" error.
fn not_found(vpath: &str) -> MountError {
    io_err(ErrorKind::NotFound, "No such file or directory", vpath)
}

/// Ensures the parent directory of a path exists.
fn ensure_parent(path: &Path, vpath: &str) -> Result<(), MountError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    }
    Ok(())
}

/// Constructs the whiteout file path for a given path (overlay directory mode).
fn whiteout_path_for(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    Some(parent.join(format!(".wh.{file_name}")))
}

/// Creates a whiteout file for a path (overlay directory mode).
fn create_whiteout(path: &Path, vpath: &str) -> Result<(), MountError> {
    if let Some(whiteout) = whiteout_path_for(path) {
        ensure_parent(&whiteout, vpath)?;
        fs::write(&whiteout, b"").map_err(|e| MountError::Io(e, vpath.to_owned()))?;
    }
    Ok(())
}

/// Removes a whiteout file if it exists (overlay directory mode).
fn remove_whiteout(path: &Path) {
    if let Some(whiteout) = whiteout_path_for(path) {
        let _ = fs::remove_file(whiteout);
    }
}
