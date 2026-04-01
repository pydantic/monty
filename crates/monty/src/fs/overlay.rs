//! Overlay-backed filesystem behavior for in-memory copy-on-write mounts.
//!
//! Reads consult overlay entries first and fall through to the real host
//! filesystem when no overlay entry is present. Writes and deletions stay in
//! memory so the real mounted directory is never modified.

use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use ahash::AHashSet;

use super::{
    common::{
        MountContext, bytes_to_utf8, check_write_limit, commit_write_bytes, current_timestamp, dir_mtime,
        format_child_path, list_visible_real_dir_entry_names, read_bytes_fs, read_text_fs, stat_fs,
    },
    dispatch::FsRequest,
    error::MountError,
    overlay_state::{OverlayEntry, OverlayFile, OverlayFileRef, OverlayState},
    path_security::{
        ResolveMode, normalize_virtual_path, reject_escaping_symlink, reject_overlong_path, resolve_path,
        strip_mount_prefix,
    },
};
use crate::{MontyObject, dir_stat, file_stat};

/// Resolves a virtual path to the mount-relative overlay key.
fn relative_path(path: &str, ctx: &MountContext<'_>) -> Result<String, MountError> {
    let normalized = normalize_virtual_path(path);
    reject_overlong_path(&normalized, path)?;
    strip_mount_prefix(&normalized, ctx.mount_virtual)
        .map(str::to_owned)
        .ok_or_else(|| MountError::NoMountPoint(path.to_owned()))
}

/// Executes a parsed filesystem request using overlay semantics.
pub(super) fn execute(
    request: FsRequest<'_>,
    ctx: &mut MountContext<'_>,
    state: &mut OverlayState,
) -> Result<MontyObject, MountError> {
    match request {
        FsRequest::Exists { path } => exists(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::IsFile { path } => is_file(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::IsDir { path } => is_dir(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::IsSymlink { path } => is_symlink(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::ReadText { path } => read_text(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::ReadBytes { path } => read_bytes(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::WriteText { path, data } => write_text(state, path, data, ctx),
        FsRequest::WriteBytes { path, data } => write_bytes(state, path, data, ctx),
        FsRequest::Mkdir {
            path,
            parents,
            exist_ok,
        } => mkdir(state, &relative_path(path, ctx)?, parents, exist_ok, ctx, path),
        FsRequest::Unlink { path } => unlink(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::Rmdir { path } => rmdir(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::Iterdir { path } => iterdir(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::Stat { path } => stat(state, &relative_path(path, ctx)?, ctx, path),
        FsRequest::Rename { src, dst } => rename(state, src, dst, ctx),
        FsRequest::Resolve { path } | FsRequest::Absolute { path } => {
            Ok(MontyObject::Path(normalize_virtual_path(path)))
        }
    }
}

/// Implements `Path.exists()` against overlay state plus real filesystem fallback.
fn exists(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    let exists = match state.get(relative) {
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_) | OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::Deleted) => false,
        None => match resolve_real_path_state(vpath, ctx, ResolveMode::Existing)? {
            RealPathState::Present(_) => true,
            RealPathState::Missing => false,
        },
    };
    Ok(MontyObject::Bool(exists))
}

/// Implements `Path.is_file()` against overlay state plus real filesystem fallback.
fn is_file(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    let is_file = match state.get(relative) {
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => true,
        Some(OverlayEntry::Directory { .. } | OverlayEntry::Deleted) => false,
        None => match resolve_real_path_state(vpath, ctx, ResolveMode::Existing)? {
            RealPathState::Present(host_path) => host_path.is_file(),
            RealPathState::Missing => false,
        },
    };
    Ok(MontyObject::Bool(is_file))
}

/// Implements `Path.is_dir()` against overlay state plus real filesystem fallback.
fn is_dir(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    let is_dir = match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_) | OverlayEntry::Deleted) => false,
        None => match resolve_real_path_state(vpath, ctx, ResolveMode::Existing)? {
            RealPathState::Present(host_path) => host_path.is_dir(),
            RealPathState::Missing => false,
        },
    };
    Ok(MontyObject::Bool(is_dir))
}

/// Implements `Path.is_symlink()`. Overlay entries are never symlinks.
fn is_symlink(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    let is_symlink = match state.get(relative) {
        Some(_) => false,
        None => match resolve_real_path_state(vpath, ctx, ResolveMode::Lstat)? {
            RealPathState::Present(host_path) => host_path.is_symlink(),
            RealPathState::Missing => false,
        },
    };
    Ok(MontyObject::Bool(is_symlink))
}

/// Reads text from the overlay or from the real filesystem on fallback.
fn read_text(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(file)) => Ok(MontyObject::String(bytes_to_utf8(file.content.clone())?)),
        Some(OverlayEntry::RealFileRef(file_ref)) => read_text_fs(&file_ref.host_path, vpath),
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let resolved = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            read_text_fs(&resolved.host_path, vpath)
        }
    }
}

/// Reads bytes from the overlay or from the real filesystem on fallback.
fn read_bytes(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(file)) => Ok(MontyObject::Bytes(file.content.clone())),
        Some(OverlayEntry::RealFileRef(file_ref)) => read_bytes_fs(&file_ref.host_path, vpath),
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let resolved = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            read_bytes_fs(&resolved.host_path, vpath)
        }
    }
}

/// Writes text into the overlay after validating quota and parent existence.
fn write_text(
    state: &mut OverlayState,
    vpath: &str,
    data: &str,
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let relative = relative_path(vpath, ctx)?;
    ensure_parent_exists(state, &relative, ctx, vpath)?;
    reject_directory_target(state, &relative, ctx, vpath)?;

    state.insert(
        relative,
        OverlayEntry::File(OverlayFile {
            content: data.as_bytes().to_vec(),
            mtime: current_timestamp(),
        }),
    );

    commit_write_bytes(data.len(), ctx);
    Ok(MontyObject::Int(
        i64::try_from(data.chars().count()).unwrap_or(i64::MAX),
    ))
}

/// Writes bytes into the overlay after validating quota and parent existence.
fn write_bytes(
    state: &mut OverlayState,
    vpath: &str,
    data: &[u8],
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let relative = relative_path(vpath, ctx)?;
    ensure_parent_exists(state, &relative, ctx, vpath)?;
    reject_directory_target(state, &relative, ctx, vpath)?;

    state.insert(
        relative,
        OverlayEntry::File(OverlayFile {
            content: data.to_vec(),
            mtime: current_timestamp(),
        }),
    );

    commit_write_bytes(data.len(), ctx);
    Ok(MontyObject::Int(i64::try_from(data.len()).unwrap_or(i64::MAX)))
}

/// Rejects writes when the target path is an existing directory.
///
/// On real filesystems, writing to a directory path returns `EISDIR`.
/// The overlay must enforce the same invariant to prevent silently
/// overwriting a directory entry with a file.
fn reject_directory_target(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<(), MountError> {
    if relative_dir_exists(state, relative, ctx) {
        return Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath));
    }
    Ok(())
}

/// Ensures the parent directory of `relative` exists in overlay or real storage.
fn ensure_parent_exists(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<(), MountError> {
    if let Some((parent_rel, _)) = relative.rsplit_once('/')
        && !relative_dir_exists(state, parent_rel, ctx)
    {
        return Err(MountError::not_found(vpath));
    }
    Ok(())
}

/// Returns whether `relative` exists as a directory in the overlay or real filesystem.
fn relative_dir_exists(state: &OverlayState, relative: &str, ctx: &MountContext<'_>) -> bool {
    match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_) | OverlayEntry::Deleted) => false,
        None => {
            let parent_vpath = format!("{}/{relative}", ctx.mount_virtual);
            resolve_path(&parent_vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)
                .is_ok_and(|resolved| resolved.host_path.is_dir())
        }
    }
}

/// Creates a directory inside the overlay.
fn mkdir(
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
        Some(OverlayEntry::Deleted) => {}
        None => {
            if let Ok(resolved) = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)
                && resolved.host_path.exists()
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
        create_overlay_parents(state, relative, ctx)?;
    } else if let Some((parent_rel, _)) = relative.rsplit_once('/')
        && !relative_dir_exists(state, parent_rel, ctx)
    {
        return Err(MountError::not_found(vpath));
    }

    state.insert(
        relative.to_owned(),
        OverlayEntry::Directory {
            mtime: current_timestamp(),
        },
    );
    Ok(MontyObject::None)
}

/// Creates parent directories for `mkdir(parents=True)` with overlay semantics.
fn create_overlay_parents(state: &mut OverlayState, relative: &str, ctx: &MountContext<'_>) -> Result<(), MountError> {
    let mut current = String::new();

    for component in relative.split('/') {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(component);

        match state.get(&current) {
            Some(OverlayEntry::Directory { .. }) => {}
            Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
                let current_vpath = format!("{}/{current}", ctx.mount_virtual);
                return Err(MountError::io_err(
                    ErrorKind::NotADirectory,
                    "Not a directory",
                    &current_vpath,
                ));
            }
            Some(OverlayEntry::Deleted) => {
                state.insert(
                    current.clone(),
                    OverlayEntry::Directory {
                        mtime: current_timestamp(),
                    },
                );
            }
            None => {
                let current_vpath = format!("{}/{current}", ctx.mount_virtual);
                if let Ok(resolved) =
                    resolve_path(&current_vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)
                {
                    if resolved.host_path.is_file() {
                        return Err(MountError::io_err(
                            ErrorKind::NotADirectory,
                            "Not a directory",
                            &current_vpath,
                        ));
                    }
                    if resolved.host_path.is_dir() {
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

    Ok(())
}

/// Removes a file in the overlay by adding a tombstone.
fn unlink(
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
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let resolved = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            if resolved.host_path.is_file() {
                state.insert(relative.to_owned(), OverlayEntry::Deleted);
                Ok(MontyObject::None)
            } else if resolved.host_path.is_dir() {
                Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
            } else {
                Err(MountError::not_found(vpath))
            }
        }
    }
}

/// Removes an empty directory in the overlay by adding a tombstone.
fn rmdir(
    state: &mut OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => {
            if overlay_directory_has_children(state, relative) {
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
            Err(MountError::io_err(ErrorKind::NotADirectory, "Not a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let resolved = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            if !resolved.host_path.is_dir() {
                return Err(MountError::io_err(ErrorKind::NotADirectory, "Not a directory", vpath));
            }
            if real_directory_has_visible_children(state, relative, &resolved.host_path, vpath)? {
                return Err(MountError::io_err(
                    ErrorKind::DirectoryNotEmpty,
                    "Directory not empty",
                    vpath,
                ));
            }
            // Also check for overlay-only children that were written into this
            // real directory. Without this check, rmdir would succeed and orphan
            // the overlay entries.
            if overlay_directory_has_children(state, relative) {
                return Err(MountError::io_err(
                    ErrorKind::DirectoryNotEmpty,
                    "Directory not empty",
                    vpath,
                ));
            }
            state.insert(relative.to_owned(), OverlayEntry::Deleted);
            Ok(MontyObject::None)
        }
    }
}

/// Returns whether an overlay directory has any visible non-deleted descendants.
fn overlay_directory_has_children(state: &OverlayState, relative: &str) -> bool {
    let prefix = directory_prefix(relative);
    state
        .prefix_iter(&prefix)
        .any(|(path, entry)| path != relative && !matches!(entry, OverlayEntry::Deleted))
}

/// Returns whether a real directory still has visible children after tombstones.
fn real_directory_has_visible_children(
    state: &OverlayState,
    relative: &str,
    host_path: &Path,
    vpath: &str,
) -> Result<bool, MountError> {
    let prefix = directory_prefix(relative);
    let entries = fs::read_dir(host_path).map_err(|err| MountError::Io(err, vpath.to_owned()))?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let child_rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}{name}")
        };

        if !matches!(state.get(&child_rel), Some(OverlayEntry::Deleted)) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Returns the `stat()` result for an overlay or fallthrough path.
fn stat(state: &OverlayState, relative: &str, ctx: &MountContext<'_>, vpath: &str) -> Result<MontyObject, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(file)) => {
            let size = i64::try_from(file.content.len()).unwrap_or(i64::MAX);
            Ok(file_stat(0o644, size, file.mtime))
        }
        Some(OverlayEntry::RealFileRef(file_ref)) => Ok(file_stat(0o644, file_ref.size, file_ref.mtime)),
        Some(OverlayEntry::Directory { mtime }) => Ok(dir_stat(0o755, *mtime)),
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let resolved = resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            stat_fs(&resolved.host_path, vpath)
        }
    }
}

/// Lists directory contents while merging overlay and real entries.
fn iterdir(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<MontyObject, MountError> {
    let host_dir_to_merge = match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => None,
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
            return Err(MountError::io_err(ErrorKind::NotADirectory, "Not a directory", vpath));
        }
        Some(OverlayEntry::Deleted) => return Err(MountError::not_found(vpath)),
        None => match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing) {
            Ok(resolved) if resolved.host_path.is_dir() => Some(resolved.host_path),
            Ok(_) => return Err(MountError::io_err(ErrorKind::NotADirectory, "Not a directory", vpath)),
            Err(MountError::Io(err, _)) if err.kind() == ErrorKind::NotFound => {
                return Err(MountError::not_found(vpath));
            }
            Err(err) => return Err(err),
        },
    };

    let prefix = directory_prefix(relative);
    let mut seen_names: AHashSet<String> = AHashSet::new();
    let mut entries = Vec::new();

    for (path, entry) in state.prefix_iter(&prefix) {
        let rest = &path[prefix.len()..];
        if rest.is_empty() || rest.contains('/') {
            continue;
        }

        let child_name = rest.to_owned();
        seen_names.insert(child_name.clone());

        if !matches!(entry, OverlayEntry::Deleted) {
            entries.push(MontyObject::Path(format_child_path(vpath, &child_name)));
        }
    }

    if let Some(host_dir) = host_dir_to_merge
        && let Ok(names) = list_visible_real_dir_entry_names(&host_dir, ctx.mount_host, vpath)
    {
        for name in names {
            if !seen_names.contains(&name) {
                entries.push(MontyObject::Path(format_child_path(vpath, &name)));
            }
        }
    }

    Ok(MontyObject::List(entries))
}

/// Renames a path within the overlay, lazily referencing real files when needed.
///
/// Validates destination type compatibility to match real filesystem semantics:
/// - file → existing directory raises `IsADirectoryError`
/// - directory → existing file raises `NotADirectoryError`
/// - directory → its own descendant raises `OSError` (invalid argument)
fn rename(
    state: &mut OverlayState,
    src_vpath: &str,
    dst_vpath: &str,
    ctx: &MountContext<'_>,
) -> Result<MontyObject, MountError> {
    let src_rel = relative_path(src_vpath, ctx)?;
    let dst_rel = relative_path(dst_vpath, ctx)?;

    ensure_parent_exists(state, &dst_rel, ctx, dst_vpath)?;

    if matches!(state.get(&src_rel), Some(OverlayEntry::Deleted)) {
        return Err(MountError::not_found(src_vpath));
    }

    let entry = if let Some(entry) = state.remove(&src_rel) {
        entry
    } else {
        // Use Lstat so symlinks are detected without following them,
        // matching the direct-mode rename behavior.
        let resolved = resolve_path(src_vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Lstat)?;
        if resolved.host_path.is_symlink() {
            // Block symlinks whose target escapes the mount boundary — allowing
            // them into the overlay as a `RealFileRef` would let subsequent
            // reads bypass boundary checks and leak host files.
            reject_escaping_symlink(&resolved.host_path, ctx.mount_host, src_vpath)?;
            // Preserve the symlink entry itself rather than its target.
            OverlayFileRef::from_lstat(&resolved.host_path)
                .map(OverlayEntry::RealFileRef)
                .ok_or_else(|| MountError::not_found(src_vpath))?
        } else if resolved.host_path.is_file() {
            OverlayFileRef::from_host_path(&resolved.host_path)
                .map(OverlayEntry::RealFileRef)
                .ok_or_else(|| MountError::not_found(src_vpath))?
        } else if resolved.host_path.is_dir() {
            OverlayEntry::Directory {
                mtime: dir_mtime(&resolved.host_path),
            }
        } else {
            return Err(MountError::not_found(src_vpath));
        }
    };

    let src_is_dir = matches!(entry, OverlayEntry::Directory { .. });
    reject_rename_type_mismatch(state, &dst_rel, src_is_dir, ctx, dst_vpath)?;

    // Reject renaming a directory into its own descendant.
    if src_is_dir {
        let src_prefix = format!("{src_rel}/");
        if dst_rel.starts_with(&src_prefix) {
            return Err(MountError::io_err(
                ErrorKind::InvalidInput,
                "Invalid argument",
                src_vpath,
            ));
        }
    }

    let mut descendants: Vec<(String, OverlayEntry)> = Vec::new();
    let mut tombstone_keys: Vec<String> = Vec::new();

    if src_is_dir {
        let src_prefix = format!("{src_rel}/");
        let dst_prefix = format!("{dst_rel}/");
        let child_keys: Vec<String> = state.prefix_iter(&src_prefix).map(|(key, _)| key.to_owned()).collect();
        let handled_keys: AHashSet<String> = child_keys.iter().cloned().collect();

        for key in child_keys {
            let suffix = &key[src_prefix.len()..];
            if let Some(child) = state.remove(&key) {
                descendants.push((format!("{dst_prefix}{suffix}"), child));
                tombstone_keys.push(key);
            }
        }

        if let Ok(resolved) = resolve_path(src_vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)
            && let Ok(real_children) = collect_real_descendants(&resolved.host_path, &src_prefix, state, &handled_keys)
        {
            for (old_rel, child_entry) in real_children {
                let suffix = old_rel.strip_prefix(&src_prefix).unwrap_or(&old_rel);
                descendants.push((format!("{dst_prefix}{suffix}"), child_entry));
                tombstone_keys.push(old_rel);
            }
        }
    }

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

/// Rejects rename when the source and destination types are incompatible.
///
/// Matches real filesystem semantics:
/// - renaming a non-directory onto an existing directory → `IsADirectoryError`
/// - renaming a directory onto an existing non-directory → `NotADirectoryError`
fn reject_rename_type_mismatch(
    state: &OverlayState,
    dst_rel: &str,
    src_is_dir: bool,
    ctx: &MountContext<'_>,
    dst_vpath: &str,
) -> Result<(), MountError> {
    let dst_is_dir = match state.get(dst_rel) {
        Some(OverlayEntry::Directory { .. }) => Some(true),
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => Some(false),
        Some(OverlayEntry::Deleted) | None => {
            match resolve_path(dst_vpath, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing) {
                Ok(resolved) if resolved.host_path.is_dir() => Some(true),
                Ok(resolved) if resolved.host_path.exists() => Some(false),
                _ => None,
            }
        }
    };

    match dst_is_dir {
        Some(true) if !src_is_dir => Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", dst_vpath)),
        Some(false) if src_is_dir => Err(MountError::io_err(
            ErrorKind::NotADirectory,
            "Not a directory",
            dst_vpath,
        )),
        _ => Ok(()),
    }
}

/// Recursively collects real descendants that should follow an overlay rename.
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
            let name = name.to_string_lossy();
            let rel_key = format!("{rel_prefix}{name}");

            if state.get(&rel_key).is_some() || already_handled.contains(&rel_key) {
                continue;
            }

            let file_type = entry.file_type()?;
            // Defense-in-depth: explicitly skip symlinks so that a symlink
            // pointing outside the mount boundary cannot be captured as an
            // OverlayFileRef during a directory rename. On Unix,
            // DirEntry::file_type() already distinguishes symlinks from files
            // and dirs, but Windows behavior may differ.
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_file() {
                if let Some(file_ref) = OverlayFileRef::from_host_path(&entry.path()) {
                    result.push((rel_key, OverlayEntry::RealFileRef(file_ref)));
                }
            } else if file_type.is_dir() {
                result.push((
                    rel_key.clone(),
                    OverlayEntry::Directory {
                        mtime: dir_mtime(&entry.path()),
                    },
                ));
                dirs.push((entry.path(), format!("{rel_key}/")));
            }
        }
    }

    Ok(result)
}

/// Resolves a real host path for an overlay fallthrough lookup.
///
/// Overlay existence-style queries intentionally collapse host-side I/O misses
/// into `Missing` so they return `false` instead of raising.
fn resolve_real_path_state(
    vpath: &str,
    ctx: &MountContext<'_>,
    mode: ResolveMode,
) -> Result<RealPathState, MountError> {
    match resolve_path(vpath, ctx.mount_virtual, ctx.mount_host, mode) {
        Ok(resolved) => Ok(RealPathState::Present(resolved.host_path)),
        Err(MountError::Io(_, _)) => Ok(RealPathState::Missing),
        Err(err) => Err(err),
    }
}

/// Result of resolving a real fallthrough path for overlay queries.
enum RealPathState {
    /// The path exists and can be queried on the host.
    Present(PathBuf),
    /// The path should behave as nonexistent.
    Missing,
}

/// Returns the prefix used to scan direct children of `relative`.
fn directory_prefix(relative: &str) -> String {
    if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    }
}
