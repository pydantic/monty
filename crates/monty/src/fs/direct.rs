//! Direct host-backed filesystem behavior for read-write and read-only mounts.
//!
//! This backend resolves a sandbox path to a validated host path and then calls
//! the corresponding `std::fs` operation without any overlay indirection.

use std::{fs, path::PathBuf};

use super::{
    common::{
        MountContext, check_write_limit, commit_write_bytes, iterdir_fs, mkdir_fs, read_bytes_fs, read_text_fs,
        rmdir_fs, stat_fs, unlink_fs, write_bytes_fs, write_text_fs,
    },
    dispatch::FsRequest,
    error::MountError,
    path_security::{ResolveMode, resolve_path},
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
            rmdir_fs(&resolved.host_path, path)
        }
        FsRequest::Iterdir { path } => {
            let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            iterdir_fs(&resolved.host_path, path, ctx.mount_host)
        }
        FsRequest::Stat { path } => {
            let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Existing)?;
            stat_fs(&resolved.host_path, path)
        }
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

/// Removes a file or symlink entry itself rather than following symlink targets.
fn unlink(path: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let resolved = resolve_path(path, ctx.mount_virtual, ctx.mount_host, ResolveMode::Lstat)?;
    unlink_fs(&resolved.host_path, path)
}

/// Renames a filesystem entry within the same mount.
fn rename(src: &str, dst: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let src_resolved = resolve_path(src, ctx.mount_virtual, ctx.mount_host, ResolveMode::Lstat)?;
    let dst_resolved = resolve_path(dst, ctx.mount_virtual, ctx.mount_host, ResolveMode::Creation)?;
    fs::rename(&src_resolved.host_path, &dst_resolved.host_path).map_err(|err| MountError::Io(err, src.to_owned()))?;
    Ok(MontyObject::None)
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
