//! Direct host-backed filesystem behavior for read-write and read-only mounts.
//!
//! This backend maps a sandbox path to a mount-relative path and calls the
//! corresponding operation on the mount's descriptor. Confinement is structural:
//! there is no host path to validate, because nothing is ever resolved from the
//! filesystem root.

use cap_std::fs::Dir;
use monty_types::{FileMode, MontyObject};

use super::{
    common::{
        MemoryBudget, MountContext, check_write_limit, commit_write_bytes, host_append_bytes, host_append_text,
        host_is_dir, host_is_file, host_iterdir, host_mkdir, host_read_bytes, host_read_text, host_rmdir, host_stat,
        host_unlink, host_write_bytes, host_write_text, map_io, reject_non_regular,
    },
    dispatch::{FsRequest, file_handle_result},
    error::MountError,
    path_security::{MountRelativePath, normalize_virtual_path, resolve_virtual_path},
};

/// Executes a parsed filesystem request directly against the host filesystem.
///
/// This backend never retains payloads (they stream to disk), so the owned
/// request is simply borrowed from and dropped when the operation finishes.
pub(super) fn execute(request: FsRequest, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    match request {
        FsRequest::Exists { path } => bool_query(&path, ctx, |dir, rel| dir.exists(rel)),
        FsRequest::IsFile { path } => bool_query(&path, ctx, host_is_file),
        FsRequest::IsDir { path } => bool_query(&path, ctx, host_is_dir),
        FsRequest::IsSymlink { path } => bool_query(&path, ctx, |dir, rel| {
            dir.symlink_metadata(rel).is_ok_and(|meta| meta.is_symlink())
        }),
        FsRequest::ReadText { path } => {
            let target = resolve_virtual_path(&path, ctx.mount_virtual)?;
            host_read_text(
                ctx.mount_dir,
                target.for_dir_op(),
                &path,
                MemoryBudget::full(ctx.memory_usage_limit),
            )
        }
        FsRequest::ReadBytes { path } => {
            let target = resolve_virtual_path(&path, ctx.mount_virtual)?;
            host_read_bytes(
                ctx.mount_dir,
                target.for_dir_op(),
                &path,
                MemoryBudget::full(ctx.memory_usage_limit),
            )
        }
        FsRequest::WriteText { path, data } => write_text(&path, &data, ctx),
        FsRequest::WriteBytes { path, data } => write_bytes(&path, &data, ctx),
        FsRequest::AppendText { path, data } => append_text(&path, &data, ctx),
        FsRequest::AppendBytes { path, data } => append_bytes(&path, &data, ctx),
        FsRequest::Mkdir {
            path,
            parents,
            exist_ok,
        } => mkdir(&path, parents, exist_ok, ctx),
        FsRequest::Unlink { path } => {
            let target = resolve_virtual_path(&path, ctx.mount_virtual)?;
            host_unlink(ctx.mount_dir, target.for_dir_op(), &path)
        }
        FsRequest::Rmdir { path } => {
            let target = resolve_virtual_path(&path, ctx.mount_virtual)?;
            reject_mount_root(&target, &path)?;
            host_rmdir(ctx.mount_dir, target.for_dir_op(), &path)
        }
        FsRequest::Iterdir { path } => {
            let target = resolve_virtual_path(&path, ctx.mount_virtual)?;
            host_iterdir(
                ctx.mount_dir,
                target.for_dir_op(),
                &path,
                MemoryBudget::full(ctx.memory_usage_limit),
            )
        }
        FsRequest::Stat { path } => {
            let target = resolve_virtual_path(&path, ctx.mount_virtual)?;
            host_stat(ctx.mount_dir, target.for_dir_op(), &path)
        }
        FsRequest::Rename { src, dst } => rename(&src, &dst, ctx),
        FsRequest::Resolve { path } | FsRequest::Absolute { path } => {
            Ok(MontyObject::Path(normalize_virtual_path(&path)))
        }
        FsRequest::Open { path, mode } => open(&path, mode, ctx),
    }
}

/// Performs the open-time effect for `open()` and returns the file handle.
///
/// The effect depends on the [`FileMode`]: read modes only check the file
/// exists; write modes truncate or create an empty file; append modes create the
/// file if missing without disturbing existing content. The host keeps no handle
/// open — this single call opens, acts, and closes.
fn open(path: &str, mode: FileMode, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    let target = resolve_virtual_path(path, ctx.mount_virtual)?;
    let rel = target.for_dir_op();
    match mode {
        FileMode::Read(_) | FileMode::ReadUpdate(_) => {
            // Surface a missing file as `FileNotFoundError` before the mode's
            // effect applies.
            ctx.mount_dir.metadata(rel).map_err(|err| map_io(err, path))?;
            reject_non_regular(ctx.mount_dir, rel, path)?;
        }
        FileMode::Write(_) | FileMode::WriteUpdate(_) => {
            check_write_limit(0, ctx)?;
            host_write_text(ctx.mount_dir, rel, "", path)?;
            commit_write_bytes(0, ctx);
        }
        FileMode::Append(_) | FileMode::AppendUpdate(_) => {
            host_append_bytes(ctx.mount_dir, rel, &[], path)?;
        }
    }
    Ok(file_handle_result(path, mode))
}

/// Answers a `pathlib` boolean query.
///
/// A path leaving the mount simply fails to resolve against the descriptor and
/// answers `false`, which is what keeps out-of-mount files unobservable. A path
/// belonging to no mount never reaches here — `MountTable` returns those as
/// `NotHandled` before dispatch.
fn bool_query(
    path: &str,
    ctx: &MountContext<'_>,
    query: impl Fn(&Dir, &str) -> bool,
) -> Result<MontyObject, MountError> {
    let target = resolve_virtual_path(path, ctx.mount_virtual)?;
    Ok(MontyObject::Bool(query(ctx.mount_dir, target.for_dir_op())))
}

/// Writes text after validating quota.
fn write_text(path: &str, data: &str, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let target = resolve_virtual_path(path, ctx.mount_virtual)?;
    let result = host_write_text(ctx.mount_dir, target.for_dir_op(), data, path)?;
    commit_write_bytes(data.len(), ctx);
    Ok(result)
}

/// Writes bytes after validating quota.
fn write_bytes(path: &str, data: &[u8], ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let target = resolve_virtual_path(path, ctx.mount_virtual)?;
    let result = host_write_bytes(ctx.mount_dir, target.for_dir_op(), data, path)?;
    commit_write_bytes(data.len(), ctx);
    Ok(result)
}

/// Appends text after validating quota.
fn append_text(path: &str, data: &str, ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let target = resolve_virtual_path(path, ctx.mount_virtual)?;
    let result = host_append_text(ctx.mount_dir, target.for_dir_op(), data, path)?;
    commit_write_bytes(data.len(), ctx);
    Ok(result)
}

/// Appends bytes after validating quota.
fn append_bytes(path: &str, data: &[u8], ctx: &mut MountContext<'_>) -> Result<MontyObject, MountError> {
    check_write_limit(data.len(), ctx)?;
    let target = resolve_virtual_path(path, ctx.mount_virtual)?;
    let result = host_append_bytes(ctx.mount_dir, target.for_dir_op(), data, path)?;
    commit_write_bytes(data.len(), ctx);
    Ok(result)
}

/// Creates a directory, using `create_dir_all` only when `parents` is set.
fn mkdir(path: &str, parents: bool, exist_ok: bool, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let target = resolve_virtual_path(path, ctx.mount_virtual)?;
    host_mkdir(ctx.mount_dir, target.for_dir_op(), parents, exist_ok, path)
}

/// Renames an entry within the same mount.
///
/// Both ends resolve against the same descriptor, so neither can name anything
/// outside it and the operation itself is a single `renameat`.
fn rename(src: &str, dst: &str, ctx: &MountContext<'_>) -> Result<MontyObject, MountError> {
    let src_target = resolve_virtual_path(src, ctx.mount_virtual)?;
    let dst_target = resolve_virtual_path(dst, ctx.mount_virtual)?;
    reject_mount_root(&src_target, src)?;
    reject_mount_root(&dst_target, dst)?;

    ctx.mount_dir
        .rename(src_target.for_dir_op(), ctx.mount_dir, dst_target.for_dir_op())
        .map_err(|err| map_io(err, src))?;
    Ok(MontyObject::None)
}

/// Refuses to rename or remove the mount root itself, which has no name inside
/// the mount. An explicit guard gives every platform and mount mode the same
/// `PermissionError`, instead of whatever errno the OS picks for `"."`.
fn reject_mount_root(target: &MountRelativePath, vpath: &str) -> Result<(), MountError> {
    if target.is_mount_root() {
        Err(MountError::PathEscape {
            virtual_path: vpath.to_owned(),
        })
    } else {
        Ok(())
    }
}
