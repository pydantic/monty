//! Direct host-backed filesystem behavior for read-write and read-only mounts.
//!
//! This backend resolves a sandbox path to a validated host path and then calls
//! the corresponding `std::fs` operation without any overlay indirection.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::{
    common::{
        MountContext, check_write_limit, commit_write_bytes, iterdir_fs, mkdir_fs, read_bytes_fs, read_text_fs,
        readlink_fs, relative_target_to_host_path, rmdir_fs, stat_fs, symlink_fs, unlink_fs, write_bytes_fs,
        write_text_fs,
    },
    dispatch::FsRequest,
    error::MountError,
    path_security::{ResolveMode, normalize_virtual_path, reject_overlong_path, resolve_path, strip_mount_prefix},
};
use crate::MontyObject;

/// Internal result used for existence-style queries where "missing" is not an error.
enum ResolvedPathState {
    /// The path resolved successfully and can be queried on the host.
    Present(PathBuf),
    /// Resolution determined that the path should behave as nonexistent.
    Missing,
}

/// Executes a parsed filesystem request directly against the host filesystem.
pub(super) fn execute(request: FsRequest<'_>, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    match request {
        FsRequest::Exists { path } => exists(path, ctx),
        FsRequest::IsFile { path } => is_file(path, ctx),
        FsRequest::IsDir { path } => is_dir(path, ctx),
        FsRequest::IsSymlink { path } => is_symlink(path, ctx),
        FsRequest::Readlink { path } => readlink(path, ctx),
        FsRequest::ReadText { path } => {
            let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            read_text_fs(&resolved.host_path, path)
        }
        FsRequest::ReadBytes { path } => {
            let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            read_bytes_fs(&resolved.host_path, path)
        }
        FsRequest::WriteText { path, data } => write_text(path, data, ctx),
        FsRequest::WriteBytes { path, data } => write_bytes(path, data, ctx),
        FsRequest::Mkdir {
            path,
            parents,
            exist_ok,
        } => mkdir(path, parents, exist_ok, ctx),
        FsRequest::Unlink { path } => unlink(path, ctx),
        FsRequest::Rmdir { path } => {
            let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            let result = rmdir_fs(&resolved.host_path, path)?;
            ctx.chmod_modes.remove(&resolved.host_path);
            Ok(result)
        }
        FsRequest::Iterdir { path } => {
            let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            iterdir_fs(&resolved.host_path, path, ctx.mount_host)
        }
        FsRequest::Stat { path, follow_symlinks } => {
            let mode = if follow_symlinks {
                ResolveMode::Existing
            } else {
                ResolveMode::Lstat
            };
            let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, mode)?;
            let mode_override = ctx.chmod_modes.get(&resolved.host_path).copied();
            stat_fs(&resolved.host_path, path, follow_symlinks, mode_override)
        }
        FsRequest::Chmod {
            path,
            mode,
            follow_symlinks,
        } => chmod(path, mode, follow_symlinks, ctx),
        FsRequest::SymlinkTo {
            path,
            target,
            target_is_directory,
        } => symlink_to(path, target, target_is_directory, ctx),
        FsRequest::Rename { src, dst } => rename(src, dst, ctx),
        FsRequest::Resolve { path } | FsRequest::Absolute { path } => {
            Ok(MontyObject::Path(super::path_security::normalize_virtual_path(path)))
        }
    }
}

/// Implements `Path.exists()` without leaking path-resolution details.
fn exists(path: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolved = resolve_existence_state(path, ctx, ResolveMode::Existing)?;
    Ok(MontyObject::Bool(matches!(resolved, ResolvedPathState::Present(_))))
}

/// Implements `Path.is_file()` while treating resolution misses as `false`.
fn is_file(path: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolved = resolve_existence_state(path, ctx, ResolveMode::Existing)?;
    Ok(MontyObject::Bool(match resolved {
        ResolvedPathState::Present(host_path) => host_path.is_file(),
        ResolvedPathState::Missing => false,
    }))
}

/// Implements `Path.is_dir()` while treating resolution misses as `false`.
fn is_dir(path: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolved = resolve_existence_state(path, ctx, ResolveMode::Existing)?;
    Ok(MontyObject::Bool(match resolved {
        ResolvedPathState::Present(host_path) => host_path.is_dir(),
        ResolvedPathState::Missing => false,
    }))
}

/// Implements `Path.is_symlink()` without following the final symlink component.
fn is_symlink(path: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolved = resolve_existence_state(path, ctx, ResolveMode::Lstat)?;
    Ok(MontyObject::Bool(match resolved {
        ResolvedPathState::Present(host_path) => host_path.is_symlink(),
        ResolvedPathState::Missing => false,
    }))
}

/// Implements `Path.readlink()` while keeping host-only targets hidden.
fn readlink(path: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Lstat)?;
    readlink_fs(&resolved.host_path, path, ctx.mount_virtual, ctx.mount_host)
}

/// Writes text after validating quota and creation-path security.
fn write_text(path: &str, data: &str, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Creation)?;
    let result = write_text_fs(&resolved.host_path, data, path)?;
    commit_write_bytes(data.len(), ctx);
    Ok(result)
}

/// Writes bytes after validating quota and creation-path security.
fn write_bytes(path: &str, data: &[u8], ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Creation)?;
    let result = write_bytes_fs(&resolved.host_path, data, path)?;
    commit_write_bytes(data.len(), ctx);
    Ok(result)
}

/// Creates a directory with the resolution mode required by `parents=...`.
fn mkdir(path: &str, parents: bool, exist_ok: bool, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let mode = if parents {
        ResolveMode::MkdirParents
    } else {
        ResolveMode::Creation
    };
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, mode)?;
    mkdir_fs(&resolved.host_path, parents, exist_ok, path)
}

/// Changes writable bits using the cross-platform readonly permission flag.
fn chmod(path: &str, mode: i64, follow_symlinks: bool, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolve_mode = if follow_symlinks {
        ResolveMode::Existing
    } else {
        ResolveMode::Lstat
    };
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, resolve_mode)?;

    if !follow_symlinks && resolved.host_path.is_symlink() {
        return Err(MountError::io_err(
            ErrorKind::Unsupported,
            "Operation not supported",
            path,
        ));
    }

    let mut permissions = fs::metadata(&resolved.host_path)
        .map_err(|err| MountError::Io(err, path.to_owned()))?
        .permissions();
    permissions.set_readonly(mode & 0o222 == 0);
    fs::set_permissions(&resolved.host_path, permissions).map_err(|err| MountError::Io(err, path.to_owned()))?;
    ctx.chmod_modes.insert(resolved.host_path, mode);
    Ok(MontyObject::None)
}

/// Removes a file or symlink entry itself rather than following symlink targets.
fn unlink(path: &str, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Lstat)?;
    let result = unlink_fs(&resolved.host_path, path)?;
    ctx.chmod_modes.remove(&resolved.host_path);
    Ok(result)
}

/// Renames a filesystem entry within the same mount.
fn rename(src: &str, dst: &str, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    let src_resolved = resolve_path(src, ctx.mount_virtual, ctx.mount_host, ResolveMode::Lstat)?;
    let dst_resolved = resolve_path(dst, ctx.mount_virtual, ctx.mount_host, ResolveMode::Creation)?;
    fs::rename(&src_resolved.host_path, &dst_resolved.host_path).map_err(|err| MountError::Io(err, src.to_owned()))?;
    move_chmod_modes_for_rename(&src_resolved.host_path, &dst_resolved.host_path, ctx);
    Ok(MontyObject::None)
}

/// Creates a symlink whose target remains within the mounted virtual namespace.
fn symlink_to(
    path: &str,
    target: &str,
    target_is_directory: bool,
    ctx: &MountContext<'_>,
) -> Result<MontyObject, MountError> {
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Creation)?;
    let target_host = symlink_target_to_host_path(path, target, ctx)?;
    symlink_fs(&resolved.host_path, &target_host, target_is_directory, path)
}

/// Converts a sandbox symlink target into the host-native target used on disk.
fn symlink_target_to_host_path(path: &str, target: &str, ctx: &MountContext<'_>) -> Result<PathBuf, MountError> {
    if target.starts_with('/') {
        let normalized = normalize_virtual_path(target);
        reject_overlong_path(&normalized, target)?;
        let relative = strip_mount_prefix(&normalized, ctx.mount_virtual).ok_or_else(|| MountError::PathEscape {
            virtual_path: path.to_owned(),
        })?;
        return Ok(if relative.is_empty() {
            ctx.mount_host.to_path_buf()
        } else {
            ctx.mount_host.join(relative)
        });
    }

    let normalized_link = normalize_virtual_path(path);
    let parent = normalized_link
        .rsplit_once('/')
        .map_or("/", |(prefix, _)| if prefix.is_empty() { "/" } else { prefix });
    let normalized_target = normalize_virtual_path(&format!("{parent}/{target}"));
    reject_overlong_path(&normalized_target, target)?;
    if strip_mount_prefix(&normalized_target, ctx.mount_virtual).is_none() {
        return Err(MountError::PathEscape {
            virtual_path: path.to_owned(),
        });
    }

    Ok(relative_target_to_host_path(target))
}

/// Resolves a path for boolean existence-style operations.
///
/// These calls intentionally collapse host-side I/O misses into `Missing`
/// because `pathlib` returns `False` instead of raising for missing paths.
fn resolve_existence_state(
    path: &str,
    ctx: &MountContext<'_>,
    mode: ResolveMode,
) -> Result<ResolvedPathState, MountError> {
    match resolve_path(path, ctx.mount_virtual, ctx.mount_host, mode) {
        Ok(resolved) => Ok(ResolvedPathState::Present(resolved.host_path)),
        Err(MountError::Io(_, _)) => Ok(ResolvedPathState::Missing),
        Err(err) => Err(err),
    }
}

/// Moves any recorded chmod overrides after a rename succeeds.
///
/// Directory renames must also retarget descendant overrides so subsequent
/// `stat()` calls continue to report the requested mode bits on moved entries.
fn move_chmod_modes_for_rename(src: &Path, dst: &Path, ctx: &mut MountContext<'_>) {
    let moved_keys: Vec<PathBuf> = ctx
        .chmod_modes
        .keys()
        .filter(|path| **path == *src || path.starts_with(src))
        .cloned()
        .collect();

    for old_path in moved_keys {
        let Some(mode) = ctx.chmod_modes.remove(&old_path) else {
            continue;
        };
        let new_path = if old_path == src {
            dst.to_path_buf()
        } else if let Ok(relative) = old_path.strip_prefix(src) {
            dst.join(relative)
        } else {
            continue;
        };
        ctx.chmod_modes.insert(new_path, mode);
    }
}
