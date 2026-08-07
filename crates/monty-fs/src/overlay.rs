//! Overlay-backed filesystem behavior for in-memory copy-on-write mounts.
//!
//! Reads consult overlay entries first and fall through to the real host
//! filesystem when no overlay entry is present. Writes and deletions stay in
//! memory so the real mounted directory is never modified.

use std::io::ErrorKind;

use ahash::AHashSet;
use cap_std::fs::Dir;
use monty_types::{FileMode, MontyObject, dir_stat, file_stat};

use super::{
    common::{
        LISTING_ENTRY_MEMORY_USAGE, MemoryBudget, MountContext, as_u64, bytes_to_utf8, check_write_limit,
        commit_write_bytes, current_timestamp, format_child_path, host_dir_mtime, host_is_dir, host_is_file,
        host_list_visible_dir_entry_names, host_read_bytes, host_read_text, host_stat, join_mount_relative, map_io,
    },
    dispatch::{FsRequest, file_handle_result},
    error::MountError,
    overlay_state::{ENTRY_MEMORY_USAGE, OverlayEntry, OverlayFile, OverlayFileRef, OverlayState},
    path_security::{
        normalize_virtual_path, reject_drive_or_unc_segments, reject_overlong_path, resolve_virtual_path,
        strip_mount_prefix,
    },
};

/// Conservative per-entry charge while capturing a real directory tree for a
/// rename: result-vec slot, recursion-queue slot, and `OverlayEntry` metadata.
/// Key strings and host paths are charged separately by length.
const REAL_DESCENDANT_MEMORY_USAGE: u64 = 512;

/// Resolves a virtual path to the mount-relative overlay key.
///
/// Keys never reach a host path, but are rejected on the same grounds one
/// would be — a name only this mode accepts is a divergence in itself.
fn relative_path(path: &str, ctx: &MountContext<'_>) -> Result<String, MountError> {
    reject_overlong_path(path)?;
    let normalized = normalize_virtual_path(path);
    let relative = strip_mount_prefix(&normalized, ctx.mount_virtual)
        .map(str::to_owned)
        .ok_or_else(|| MountError::NoMountPoint(path.to_owned()))?;
    reject_drive_or_unc_segments(&relative, &normalized)?;
    Ok(relative)
}

/// Returns budget available for a transient result alongside retained state.
fn available_memory(state: &OverlayState, ctx: &MountContext<'_>) -> Result<MemoryBudget, MountError> {
    let available = ctx
        .memory_usage_limit
        .checked_sub(state.memory_usage())
        .ok_or(MountError::MemoryUsageLimitExceeded(ctx.memory_usage_limit))?;
    Ok(MemoryBudget {
        available,
        limit: ctx.memory_usage_limit,
    })
}

/// Executes a parsed filesystem request using overlay semantics.
///
/// Truncating writes move their payload into overlay storage; appends borrow
/// it (extending the retained buffer copies regardless).
pub(super) fn execute(
    request: FsRequest,
    ctx: &mut MountContext<'_>,
    state: &mut OverlayState,
) -> Result<MontyObject, MountError> {
    match request {
        FsRequest::Exists { path } => exists(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::IsFile { path } => is_file(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::IsDir { path } => is_dir(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::IsSymlink { path } => is_symlink(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::ReadText { path } => read_text(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::ReadBytes { path } => read_bytes(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::WriteText { path, data } => write_text(state, &path, data, ctx),
        FsRequest::WriteBytes { path, data } => write_bytes(state, &path, data, ctx),
        FsRequest::AppendText { path, data } => append_text(state, &path, &data, ctx),
        FsRequest::AppendBytes { path, data } => append_bytes(state, &path, &data, ctx),
        FsRequest::Mkdir {
            path,
            parents,
            exist_ok,
        } => mkdir(state, &relative_path(&path, ctx)?, parents, exist_ok, ctx, &path),
        FsRequest::Unlink { path } => unlink(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::Rmdir { path } => rmdir(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::Iterdir { path } => iterdir(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::Stat { path } => stat(state, &relative_path(&path, ctx)?, ctx, &path),
        FsRequest::Rename { src, dst } => rename(state, &src, &dst, ctx),
        FsRequest::Resolve { path } | FsRequest::Absolute { path } => {
            Ok(MontyObject::Path(normalize_virtual_path(&path)))
        }
        FsRequest::Open { path, mode } => open(state, &path, mode, ctx),
    }
}

/// Performs the open-time effect for `open()` against overlay state.
///
/// `Read` checks the file exists (in the overlay or via real-filesystem
/// fallthrough); `Write` truncates by inserting an empty overlay file;
/// `Append` creates the file if missing while preserving existing content.
/// All writes stay in the overlay — the real mounted directory is untouched.
fn open(
    state: &mut OverlayState,
    path: &str,
    file_mode: FileMode,
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    match file_mode {
        FileMode::Read(_) | FileMode::ReadUpdate(_) => {
            let relative = relative_path(path, ctx)?;
            match state.get(&relative) {
                Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {}
                Some(OverlayEntry::Directory { .. }) => {
                    return Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", path));
                }
                Some(OverlayEntry::Deleted) => return Err(MountError::not_found(path)),
                None => match resolve_real_path_state(path, ctx, Follow::Yes, OnLookupFailure::Propagate)? {
                    RealPathState::Present(rel) if host_is_dir(ctx.mount_dir, &rel) => {
                        return Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", path));
                    }
                    RealPathState::Present(_) => {}
                    RealPathState::Missing => return Err(MountError::not_found(path)),
                },
            }
        }
        // `write_text` with empty data gives exactly the truncating
        // create-or-clobber semantics `open(w)` needs.
        FileMode::Write(_) | FileMode::WriteUpdate(_) => {
            write_text(state, path, String::new(), ctx)?;
        }
        // `open(a)` only needs the file to exist — it must NOT pull the real
        // file's content into the overlay, because that would O(file_size)
        // copy on every `open(..., 'a')` even when the handle is closed
        // without writing. Just create-if-missing; the append-time bytes
        // pull only happens if user code actually writes.
        FileMode::Append(_) | FileMode::AppendUpdate(_) => {
            ensure_append_target_exists(state, path, ctx)?;
        }
    }
    Ok(file_handle_result(path, file_mode))
}

/// Ensures the append target exists without pulling real-file content into
/// the overlay.
///
/// Used by `open(path, 'a')` so that opening an append handle on a 1GB real
/// file does not copy 1GB of bytes into the overlay just to satisfy "create
/// if missing" semantics. If the file already exists (either in overlay or
/// on the real backing store) this is a no-op; if it does not, an empty
/// overlay file is inserted.
fn ensure_append_target_exists(
    state: &mut OverlayState,
    vpath: &str,
    ctx: &mut MountContext<'_>,
) -> Result<(), MountError> {
    let relative = relative_path(vpath, ctx)?;
    match state.get(&relative) {
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => Ok(()),
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => {
            ensure_parent_exists(state, &relative, ctx, vpath)?;
            state.insert(
                relative,
                OverlayEntry::File(OverlayFile {
                    content: Vec::new(),
                    mtime: current_timestamp(),
                }),
                ctx.memory_usage_limit,
            )?;
            Ok(())
        }
        None => {
            let target = resolve_virtual_path(vpath, ctx.mount_virtual)?;
            match classify_write_target(ctx.mount_dir, target.for_dir_op(), vpath)? {
                RealTarget::Dir => Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath)),
                RealTarget::Symlink => Err(MountError::PathEscape {
                    virtual_path: vpath.to_owned(),
                }),
                // The file is already there, but the write policy still refuses
                // a symlink anywhere in the path; checking it here rather than
                // at the first append makes `open` fail where the user asked.
                RealTarget::File => ensure_parent_exists(state, &relative, ctx, vpath),
                RealTarget::Absent => {
                    ensure_parent_exists(state, &relative, ctx, vpath)?;
                    state.insert(
                        relative,
                        OverlayEntry::File(OverlayFile {
                            content: Vec::new(),
                            mtime: current_timestamp(),
                        }),
                        ctx.memory_usage_limit,
                    )?;
                    Ok(())
                }
            }
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
        None => match resolve_real_path_state(vpath, ctx, Follow::Yes, OnLookupFailure::Missing)? {
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
        None => match resolve_real_path_state(vpath, ctx, Follow::Yes, OnLookupFailure::Missing)? {
            RealPathState::Present(rel) => host_is_file(ctx.mount_dir, &rel),
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
        None => match resolve_real_path_state(vpath, ctx, Follow::Yes, OnLookupFailure::Missing)? {
            RealPathState::Present(rel) => host_is_dir(ctx.mount_dir, &rel),
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
        None => match resolve_real_path_state(vpath, ctx, Follow::No, OnLookupFailure::Missing)? {
            RealPathState::Present(rel) => ctx.mount_dir.symlink_metadata(&rel).is_ok_and(|m| m.is_symlink()),
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
        Some(OverlayEntry::File(file)) => {
            available_memory(state, ctx)?.check(as_u64(file.content.len()))?;
            Ok(MontyObject::String(bytes_to_utf8(file.content.clone())?))
        }
        Some(OverlayEntry::RealFileRef(file_ref)) => {
            host_read_text(ctx.mount_dir, &file_ref.relative, vpath, available_memory(state, ctx)?)
        }
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let target = resolve_virtual_path(vpath, ctx.mount_virtual)?;
            host_read_text(ctx.mount_dir, target.for_dir_op(), vpath, available_memory(state, ctx)?)
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
        Some(OverlayEntry::File(file)) => {
            available_memory(state, ctx)?.check(as_u64(file.content.len()))?;
            Ok(MontyObject::Bytes(file.content.clone()))
        }
        Some(OverlayEntry::RealFileRef(file_ref)) => {
            host_read_bytes(ctx.mount_dir, &file_ref.relative, vpath, available_memory(state, ctx)?)
        }
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            let target = resolve_virtual_path(vpath, ctx.mount_virtual)?;
            host_read_bytes(ctx.mount_dir, target.for_dir_op(), vpath, available_memory(state, ctx)?)
        }
    }
}

/// Writes text into the overlay after validating quota and parent existence.
/// Takes the payload by value so the retained content is a move, not a copy.
fn write_text(
    state: &mut OverlayState,
    vpath: &str,
    data: String,
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    // The return value is the CPython char count — computed up front since
    // the bytes move into the overlay below.
    let char_count = data.chars().count();
    let byte_len = data.len();
    check_write_limit(byte_len, ctx)?;
    let relative = relative_path(vpath, ctx)?;
    ensure_parent_exists(state, &relative, ctx, vpath)?;
    reject_directory_target(state, &relative, ctx, vpath)?;
    state.check_file_replacement(&relative, byte_len, ctx.memory_usage_limit)?;

    state.insert(
        relative,
        OverlayEntry::File(OverlayFile {
            content: data.into_bytes(),
            mtime: current_timestamp(),
        }),
        ctx.memory_usage_limit,
    )?;

    commit_write_bytes(byte_len, ctx);
    Ok(MontyObject::Int(i64::try_from(char_count).unwrap_or(i64::MAX)))
}

/// Writes bytes into the overlay after validating quota and parent existence.
/// Takes the payload by value so the retained content is a move, not a copy.
fn write_bytes(
    state: &mut OverlayState,
    vpath: &str,
    data: Vec<u8>,
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    let byte_len = data.len();
    check_write_limit(byte_len, ctx)?;
    let relative = relative_path(vpath, ctx)?;
    ensure_parent_exists(state, &relative, ctx, vpath)?;
    reject_directory_target(state, &relative, ctx, vpath)?;
    state.check_file_replacement(&relative, byte_len, ctx.memory_usage_limit)?;

    state.insert(
        relative,
        OverlayEntry::File(OverlayFile {
            content: data,
            mtime: current_timestamp(),
        }),
        ctx.memory_usage_limit,
    )?;

    commit_write_bytes(byte_len, ctx);
    Ok(MontyObject::Int(i64::try_from(byte_len).unwrap_or(i64::MAX)))
}

/// Appends text in the overlay without leaving a host file handle open.
fn append_text(
    state: &mut OverlayState,
    vpath: &str,
    data: &str,
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    append_bytes(state, vpath, data.as_bytes(), ctx)?;
    Ok(MontyObject::Int(
        i64::try_from(data.chars().count()).unwrap_or(i64::MAX),
    ))
}

/// Appends bytes in the overlay, copying through real mounted content if needed.
///
/// Existing overlay files are extended in place ([`OverlayState::append_file`]) —
/// cloning and re-inserting would make repeated appends to the same file
/// O(n²) in total content. A real backing file is read only after its final
/// size is known to fit the mount memory budget.
fn append_bytes(
    state: &mut OverlayState,
    vpath: &str,
    data: &[u8],
    ctx: &mut MountContext<'_>,
) -> Result<MontyObject, MountError> {
    let relative = relative_path(vpath, ctx)?;
    ensure_parent_exists(state, &relative, ctx, vpath)?;
    reject_directory_target(state, &relative, ctx, vpath)?;
    let target_is_overlay_file = matches!(state.get(&relative), Some(OverlayEntry::File(_)));
    let existing_len = existing_file_len(state, &relative, ctx, vpath)?;
    let charged_bytes = if ctx.write_bytes_limit.is_some() && !target_is_overlay_file {
        existing_len.saturating_add(data.len())
    } else {
        data.len()
    };
    check_write_limit(charged_bytes, ctx)?;

    if !state.append_file(&relative, data, current_timestamp(), ctx.memory_usage_limit)? {
        let final_len = existing_len.saturating_add(data.len());
        state.check_file_replacement(&relative, final_len, ctx.memory_usage_limit)?;
        let budget = available_memory(state, ctx)?;
        budget.check(as_u64(final_len))?;
        let mut content = existing_file_bytes(state, &relative, ctx, vpath, budget.shrink(as_u64(data.len()))?)?;
        content.extend_from_slice(data);
        state.insert(
            relative,
            OverlayEntry::File(OverlayFile {
                content,
                mtime: current_timestamp(),
            }),
            ctx.memory_usage_limit,
        )?;
    }

    commit_write_bytes(charged_bytes, ctx);
    Ok(MontyObject::Int(i64::try_from(data.len()).unwrap_or(i64::MAX)))
}

/// Returns the visible file length for append accounting without loading bytes.
///
/// Overlay append may need to copy a real backing file into memory before
/// extending it. Counting that existing file size before materialization keeps
/// `write_bytes_limit` aligned with the amount of overlay memory the operation
/// can create.
fn existing_file_len(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<usize, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(file)) => Ok(file.content.len()),
        Some(OverlayEntry::Deleted) => Ok(0),
        Some(OverlayEntry::RealFileRef(file_ref)) => file_len(ctx.mount_dir, &file_ref.relative, vpath),
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        None => match resolve_real_path_state(vpath, ctx, Follow::Yes, OnLookupFailure::Propagate)? {
            RealPathState::Present(rel) => file_len(ctx.mount_dir, &rel, vpath),
            RealPathState::Missing => Ok(0),
        },
    }
}

/// Returns a host file's byte length for quota checks.
///
/// File sizes larger than addressable memory saturate so quota comparison fails
/// closed instead of wrapping before the overlay tries to allocate.
fn file_len(dir: &Dir, rel: &str, vpath: &str) -> Result<usize, MountError> {
    let len = dir.metadata(rel).map_err(|error| map_io(error, vpath))?.len();
    Ok(usize::try_from(len).unwrap_or(usize::MAX))
}

/// Loads the current visible file content for append operations.
fn existing_file_bytes(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
    budget: MemoryBudget,
) -> Result<Vec<u8>, MountError> {
    match state.get(relative) {
        Some(OverlayEntry::File(file)) => {
            budget.check(as_u64(file.content.len()))?;
            Ok(file.content.clone())
        }
        Some(OverlayEntry::Deleted) => Ok(Vec::new()),
        Some(OverlayEntry::RealFileRef(file_ref)) => {
            match host_read_bytes(ctx.mount_dir, &file_ref.relative, vpath, budget)? {
                MontyObject::Bytes(bytes) => Ok(bytes),
                _ => unreachable!("host_read_bytes should return bytes"),
            }
        }
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        None => match resolve_real_path_state(vpath, ctx, Follow::Yes, OnLookupFailure::Propagate)? {
            RealPathState::Present(rel) => match host_read_bytes(ctx.mount_dir, &rel, vpath, budget)? {
                MontyObject::Bytes(bytes) => Ok(bytes),
                _ => unreachable!("host_read_bytes should return bytes"),
            },
            RealPathState::Missing => Ok(Vec::new()),
        },
    }
}

/// Rejects writes when the target path is an existing directory or a symlink.
///
/// On real filesystems, writing to a directory returns `EISDIR`; the overlay
/// enforces the same. A direct mount writes *through* a symlink to its target,
/// which the overlay cannot replicate without aliasing the name, so writing
/// over any symlink raises `PermissionError` instead of silently shadowing it.
fn reject_directory_target(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<(), MountError> {
    match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        // An overlay file, ref, or tombstone already shadows whatever is real.
        Some(_) => Ok(()),
        None => match resolve_virtual_path(vpath, ctx.mount_virtual) {
            Ok(target) => match classify_write_target(ctx.mount_dir, target.for_dir_op(), vpath)? {
                RealTarget::Dir => Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath)),
                RealTarget::Symlink => Err(MountError::PathEscape {
                    virtual_path: vpath.to_owned(),
                }),
                RealTarget::File | RealTarget::Absent => Ok(()),
            },
            Err(_) => Ok(()),
        },
    }
}

/// Ensures every component of `relative`'s parent chain is a directory the
/// overlay may write beneath.
///
/// The chain is walked component by component because a single lookup of the
/// immediate parent resolves *through* intermediate symlinks without seeing
/// them — and overlay writes must refuse symlinks anywhere in the path, not
/// just at its end (see `limitations/filesystem.md`).
fn ensure_parent_exists(
    state: &OverlayState,
    relative: &str,
    ctx: &MountContext<'_>,
    vpath: &str,
) -> Result<(), MountError> {
    let Some((parents, _)) = relative.rsplit_once('/') else {
        return Ok(());
    };
    let mut current = String::new();
    for component in parents.split('/') {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(component);
        match state.get(&current) {
            // Overlay directories are symlink-free by construction.
            Some(OverlayEntry::Directory { .. }) => {}
            Some(_) => return Err(MountError::not_found(vpath)),
            None => {
                let dir_vpath = format!("{}/{current}", ctx.mount_virtual);
                let Ok(target) = resolve_virtual_path(&dir_vpath, ctx.mount_virtual) else {
                    return Err(MountError::not_found(vpath));
                };
                match classify_write_target(ctx.mount_dir, target.for_dir_op(), vpath)? {
                    RealTarget::Dir => {}
                    RealTarget::Symlink => {
                        return Err(MountError::PathEscape {
                            virtual_path: vpath.to_owned(),
                        });
                    }
                    RealTarget::File | RealTarget::Absent => return Err(MountError::not_found(vpath)),
                }
            }
        }
    }
    Ok(())
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
            if let Ok(target) = resolve_virtual_path(vpath, ctx.mount_virtual)
                && let Ok(meta) = ctx.mount_dir.symlink_metadata(target.for_dir_op())
            {
                return if meta.is_dir() && exist_ok {
                    Ok(MontyObject::None)
                } else {
                    // Either it's a file (always an error) or a dir with exist_ok=false.
                    Err(MountError::io_err(ErrorKind::AlreadyExists, "File exists", vpath))
                };
            }
        }
    }

    if parents {
        create_overlay_parents(state, relative, ctx)?;
    } else {
        ensure_parent_exists(state, relative, ctx, vpath)?;
    }

    state.insert(
        relative.to_owned(),
        OverlayEntry::Directory {
            mtime: current_timestamp(),
        },
        ctx.memory_usage_limit,
    )?;
    Ok(MontyObject::None)
}

/// Creates parent directories for `mkdir(parents=True)` with overlay semantics.
///
/// Each component is classified through the descriptor: any symlink raises
/// `PermissionError` instead of being shadowed by (or aliased under) an
/// in-memory tree.
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
                    ctx.memory_usage_limit,
                )?;
            }
            None => {
                let current_vpath = format!("{}/{current}", ctx.mount_virtual);
                if let Ok(resolved) = resolve_virtual_path(&current_vpath, ctx.mount_virtual) {
                    match classify_write_target(ctx.mount_dir, resolved.for_dir_op(), &current_vpath)? {
                        RealTarget::Dir => continue,
                        RealTarget::Symlink => {
                            return Err(MountError::PathEscape {
                                virtual_path: current_vpath,
                            });
                        }
                        RealTarget::File => {
                            return Err(MountError::io_err(
                                ErrorKind::NotADirectory,
                                "Not a directory",
                                &current_vpath,
                            ));
                        }
                        RealTarget::Absent => {}
                    }
                }

                state.insert(
                    current.clone(),
                    OverlayEntry::Directory {
                        mtime: current_timestamp(),
                    },
                    ctx.memory_usage_limit,
                )?;
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
            state.insert(relative.to_owned(), OverlayEntry::Deleted, ctx.memory_usage_limit)?;
            Ok(MontyObject::None)
        }
        Some(OverlayEntry::Directory { .. }) => {
            Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            // A delete is a write: the same symlink refusal applies. Tombstoning
            // a link's spelling would leave its target readable under the real
            // name — the aliasing the write policy exists to prevent.
            ensure_parent_exists(state, relative, ctx, vpath)?;
            let resolved = resolve_virtual_path(vpath, ctx.mount_virtual)?;
            match classify_write_target(ctx.mount_dir, resolved.for_dir_op(), vpath)? {
                RealTarget::File => {
                    state.insert(relative.to_owned(), OverlayEntry::Deleted, ctx.memory_usage_limit)?;
                    Ok(MontyObject::None)
                }
                RealTarget::Dir => Err(MountError::io_err(ErrorKind::IsADirectory, "Is a directory", vpath)),
                RealTarget::Symlink => Err(MountError::PathEscape {
                    virtual_path: vpath.to_owned(),
                }),
                RealTarget::Absent => Err(MountError::not_found(vpath)),
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
    // Without this guard the root would tombstone in memory while writes to
    // its children kept succeeding — self-contradictory state the direct
    // backend refuses to enter.
    reject_mount_root(relative, vpath)?;
    match state.get(relative) {
        Some(OverlayEntry::Directory { .. }) => {
            if overlay_directory_has_children(state, relative) {
                return Err(MountError::io_err(
                    ErrorKind::DirectoryNotEmpty,
                    "Directory not empty",
                    vpath,
                ));
            }
            state.insert(relative.to_owned(), OverlayEntry::Deleted, ctx.memory_usage_limit)?;
            Ok(MontyObject::None)
        }
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => {
            Err(MountError::io_err(ErrorKind::NotADirectory, "Not a directory", vpath))
        }
        Some(OverlayEntry::Deleted) => Err(MountError::not_found(vpath)),
        None => {
            ensure_parent_exists(state, relative, ctx, vpath)?;
            let resolved = resolve_virtual_path(vpath, ctx.mount_virtual)?;
            let rel = resolved.for_dir_op();
            // A symlink is not a directory to remove — POSIX answers ENOTDIR —
            // and tombstoning its name would alias it against its target.
            if matches!(classify_write_target(ctx.mount_dir, rel, vpath)?, RealTarget::Symlink) {
                return Err(MountError::PathEscape {
                    virtual_path: vpath.to_owned(),
                });
            }
            if !host_is_dir(ctx.mount_dir, rel) {
                // `resolve_virtual_path` never touches the filesystem, so a
                // missing path arrives as `Ok` and must be told apart from one
                // that exists but is not a directory.
                return Err(if ctx.mount_dir.exists(rel) {
                    MountError::io_err(ErrorKind::NotADirectory, "Not a directory", vpath)
                } else {
                    MountError::not_found(vpath)
                });
            }
            if real_directory_has_visible_children(state, relative, ctx.mount_dir, resolved.for_dir_op(), vpath)? {
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
            state.insert(relative.to_owned(), OverlayEntry::Deleted, ctx.memory_usage_limit)?;
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
    dir: &Dir,
    rel: &str,
    vpath: &str,
) -> Result<bool, MountError> {
    let prefix = directory_prefix(relative);
    let entries = dir.read_dir(rel).map_err(|err| map_io(err, vpath))?;

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
            let target = resolve_virtual_path(vpath, ctx.mount_virtual)?;
            host_stat(ctx.mount_dir, target.for_dir_op(), vpath)
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
        None => match resolve_virtual_path(vpath, ctx.mount_virtual) {
            Ok(target) if host_is_dir(ctx.mount_dir, target.for_dir_op()) => Some(target.for_dir_op().to_owned()),
            // `resolve_virtual_path` never touches the filesystem, so a missing
            // path arrives as `Ok` and must be told apart from a non-directory.
            Ok(target) if !ctx.mount_dir.exists(target.for_dir_op()) => return Err(MountError::not_found(vpath)),
            Ok(_) => return Err(MountError::io_err(ErrorKind::NotADirectory, "Not a directory", vpath)),
            Err(err) => return Err(err),
        },
    };

    let prefix = directory_prefix(relative);
    let mut seen_names: AHashSet<String> = AHashSet::new();
    let mut entries = Vec::new();
    let budget = available_memory(state, ctx)?;
    let mut transient_usage = 0_u64;

    for (path, entry) in state.prefix_iter(&prefix) {
        let rest = &path[prefix.len()..];
        if rest.is_empty() || rest.contains('/') {
            continue;
        }

        // Names are charged twice: once for the dedup set, once for the clone.
        let child_name = rest.to_owned();
        transient_usage = transient_usage
            .saturating_add(as_u64(child_name.len().saturating_mul(2)))
            .saturating_add(LISTING_ENTRY_MEMORY_USAGE);
        budget.check(transient_usage)?;
        seen_names.insert(child_name.clone());

        if !matches!(entry, OverlayEntry::Deleted) {
            let child_path = format_child_path(vpath, &child_name);
            transient_usage = transient_usage
                .saturating_add(as_u64(child_path.len()))
                .saturating_add(LISTING_ENTRY_MEMORY_USAGE);
            budget.check(transient_usage)?;
            entries.push(MontyObject::Path(child_path));
        }
    }

    if let Some(host_dir) = host_dir_to_merge {
        let remaining = budget.shrink(transient_usage)?;
        let names = match host_list_visible_dir_entry_names(ctx.mount_dir, &host_dir, vpath, remaining.halved()) {
            Ok(names) => names,
            Err(error @ MountError::MemoryUsageLimitExceeded(_)) => return Err(error),
            Err(_) => Vec::new(),
        };
        if !names.is_empty() {
            transient_usage = names.iter().fold(transient_usage, |usage, name| {
                usage
                    .saturating_add(as_u64(name.len()))
                    .saturating_add(LISTING_ENTRY_MEMORY_USAGE)
            });
            for name in names {
                if !seen_names.contains(&name) {
                    let child_path = format_child_path(vpath, &name);
                    transient_usage = transient_usage
                        .saturating_add(as_u64(child_path.len()))
                        .saturating_add(LISTING_ENTRY_MEMORY_USAGE);
                    budget.check(transient_usage)?;
                    entries.push(MontyObject::Path(child_path));
                }
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

    // The mount root has no name inside the mount. The descriptor would refuse,
    // but the overlay commits its in-memory plan first, so it has to refuse too
    // or the two backends disagree.
    reject_mount_root(&src_rel, src_vpath)?;
    reject_mount_root(&dst_rel, dst_vpath)?;

    ensure_parent_exists(state, &dst_rel, ctx, dst_vpath)?;

    // The destination's own name must not be a symlink either. An overlay entry
    // under it would shadow the link and alias it against its target — exactly
    // what `write_text` on the same path refuses.
    if state.get(&dst_rel).is_none()
        && let Ok(target) = resolve_virtual_path(dst_vpath, ctx.mount_virtual)
        && matches!(
            classify_write_target(ctx.mount_dir, target.for_dir_op(), dst_vpath)?,
            RealTarget::Symlink
        )
    {
        return Err(MountError::PathEscape {
            virtual_path: dst_vpath.to_owned(),
        });
    }

    if matches!(state.get(&src_rel), Some(OverlayEntry::Deleted)) {
        return Err(MountError::not_found(src_vpath));
    }

    // Determine whether the source is a directory before removing it from state,
    // so that validation checks below don't lose the entry on failure.
    let src_is_dir = match state.get(&src_rel) {
        Some(OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => false,
        Some(OverlayEntry::Deleted) => return Err(MountError::not_found(src_vpath)),
        None => resolve_virtual_path(src_vpath, ctx.mount_virtual)
            .is_ok_and(|target| host_is_dir(ctx.mount_dir, target.for_dir_op())),
    };

    reject_rename_type_mismatch(state, &dst_rel, src_is_dir, ctx, dst_vpath)?;

    // Renaming a directory onto an existing non-empty directory must fail,
    // matching POSIX/CPython semantics.
    if src_is_dir {
        reject_rename_onto_nonempty_dir(state, &dst_rel, ctx, dst_vpath)?;
    }

    // Reject renaming a directory into its own descendant before building the
    // atomic overlay update plan.
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

    let source_is_overlay = state.get(&src_rel).is_some();
    let real_source_entry = if source_is_overlay {
        None
    } else {
        let target = resolve_virtual_path(src_vpath, ctx.mount_virtual)?;
        let rel = target.for_dir_op();
        let is_symlink = ctx.mount_dir.symlink_metadata(rel).is_ok_and(|meta| meta.is_symlink());
        let entry = if is_symlink {
            // A stored reference could not escape anyway, but failing here puts
            // the error where the user caused it rather than on a later read.
            // Route through `map_io` so only a confinement failure reads as
            // `PathEscape` — a denied in-mount target is a permission error.
            if let Err(err) = ctx.mount_dir.metadata(rel) {
                return Err(map_io(err, src_vpath));
            }
            // Preserve the symlink entry itself rather than its target.
            OverlayFileRef::from_lstat(ctx.mount_dir, rel)
                .map(OverlayEntry::RealFileRef)
                .ok_or_else(|| MountError::not_found(src_vpath))?
        } else if host_is_file(ctx.mount_dir, rel) {
            OverlayFileRef::from_relative(ctx.mount_dir, rel)
                .map(OverlayEntry::RealFileRef)
                .ok_or_else(|| MountError::not_found(src_vpath))?
        } else if host_is_dir(ctx.mount_dir, rel) {
            OverlayEntry::Directory {
                mtime: host_dir_mtime(ctx.mount_dir, rel),
            }
        } else {
            return Err(MountError::not_found(src_vpath));
        };
        Some(entry)
    };

    let source_entry = state
        .get(&src_rel)
        .or(real_source_entry.as_ref())
        .expect("rename source was resolved above");
    let mut overlay_moves = Vec::new();
    let mut real_moves = Vec::new();

    if src_is_dir {
        let src_prefix = format!("{src_rel}/");
        let dst_prefix = format!("{dst_rel}/");
        // Each descendant's plan holds the old key twice (child_keys + handled_keys)
        // and its new destination key, plus per-entry container overhead.
        let plan_usage = state.prefix_iter(&src_prefix).fold(0_u64, |usage, (key, _)| {
            let suffix_len = key.len().saturating_sub(src_prefix.len());
            usage
                .saturating_add(as_u64(key.len().saturating_mul(2)))
                .saturating_add(as_u64(dst_prefix.len().saturating_add(suffix_len)))
                .saturating_add(ENTRY_MEMORY_USAGE)
        });
        let remaining = available_memory(state, ctx)?.shrink(plan_usage)?;
        let child_keys: Vec<String> = state.prefix_iter(&src_prefix).map(|(key, _)| key.to_owned()).collect();
        let handled_keys: AHashSet<String> = child_keys.iter().cloned().collect();

        for key in child_keys {
            let new_key = format!("{dst_prefix}{}", &key[src_prefix.len()..]);
            overlay_moves.push((key, new_key));
        }

        if let Ok(target) = resolve_virtual_path(src_vpath, ctx.mount_virtual)
            && host_is_dir(ctx.mount_dir, target.for_dir_op())
        {
            let real_children = match collect_real_descendants(
                ctx.mount_dir,
                target.for_dir_op(),
                &src_prefix,
                state,
                &handled_keys,
                src_vpath,
                remaining,
            ) {
                Ok(children) => children,
                // Lookup failures degrade to "no real children" (the host may
                // have removed them mid-plan), but resource errors, refusals
                // and unrepresentable names must fail the rename rather than
                // silently shrink it.
                Err(error) => match &error {
                    MountError::MemoryUsageLimitExceeded(_) | MountError::PathEscape { .. } => return Err(error),
                    MountError::Io(io_error, _) if io_error.kind() == ErrorKind::InvalidData => {
                        return Err(error);
                    }
                    _ => Vec::new(),
                },
            };
            for (old_rel, child_entry) in real_children {
                let new_rel = format!("{dst_prefix}{}", old_rel.strip_prefix(&src_prefix).unwrap_or(&old_rel));
                real_moves.push((old_rel, new_rel, child_entry));
            }
        }
    }

    let deleted = OverlayEntry::Deleted;
    let mut replacements = vec![(src_rel.as_str(), &deleted), (dst_rel.as_str(), source_entry)];
    for (old_rel, new_rel) in &overlay_moves {
        replacements.push((old_rel, &deleted));
        replacements.push((new_rel, state.get(old_rel).expect("overlay descendant still exists")));
    }
    for (old_rel, new_rel, entry) in &real_moves {
        replacements.push((old_rel, &deleted));
        replacements.push((new_rel, entry));
    }
    state.check_replacements(replacements, ctx.memory_usage_limit)?;

    let entry = if source_is_overlay {
        state.remove(&src_rel).expect("overlay rename source still exists")
    } else {
        real_source_entry.expect("real rename source was captured")
    };
    let descendants: Vec<(String, String, OverlayEntry)> = overlay_moves
        .into_iter()
        .map(|(old_rel, new_rel)| {
            let entry = state.remove(&old_rel).expect("overlay rename descendant still exists");
            (old_rel, new_rel, entry)
        })
        .chain(real_moves)
        .collect();

    state.insert_unchecked(src_rel, OverlayEntry::Deleted);
    state.insert_unchecked(dst_rel, entry);

    for (old_rel, new_rel, child) in descendants {
        state.insert_unchecked(old_rel, OverlayEntry::Deleted);
        state.insert_unchecked(new_rel, child);
    }

    Ok(MontyObject::None)
}

/// Refuses to rename or remove the mount root, which has no name inside the
/// mount — matching the direct backend's guard, since the overlay commits its
/// in-memory plan before any descriptor could refuse.
fn reject_mount_root(relative: &str, vpath: &str) -> Result<(), MountError> {
    if relative.is_empty() {
        Err(MountError::PathEscape {
            virtual_path: vpath.to_owned(),
        })
    } else {
        Ok(())
    }
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
        Some(OverlayEntry::Deleted) | None => match resolve_virtual_path(dst_vpath, ctx.mount_virtual) {
            Ok(target) if host_is_dir(ctx.mount_dir, target.for_dir_op()) => Some(true),
            Ok(target) if ctx.mount_dir.exists(target.for_dir_op()) => Some(false),
            _ => None,
        },
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

/// Rejects renaming a directory onto an existing non-empty directory.
///
/// Matches POSIX semantics: `rename(src_dir, dst_dir)` only succeeds when
/// `dst_dir` is empty. Checks both overlay children and real filesystem
/// children, reusing the same helpers as `rmdir`.
fn reject_rename_onto_nonempty_dir(
    state: &OverlayState,
    dst_rel: &str,
    ctx: &MountContext<'_>,
    dst_vpath: &str,
) -> Result<(), MountError> {
    let dst_is_dir = match state.get(dst_rel) {
        Some(OverlayEntry::Directory { .. }) => true,
        Some(OverlayEntry::Deleted | OverlayEntry::File(_) | OverlayEntry::RealFileRef(_)) => return Ok(()),
        None => match resolve_virtual_path(dst_vpath, ctx.mount_virtual) {
            Ok(target) if host_is_dir(ctx.mount_dir, target.for_dir_op()) => true,
            _ => return Ok(()),
        },
    };

    if !dst_is_dir {
        return Ok(());
    }

    if overlay_directory_has_children(state, dst_rel) {
        return Err(MountError::io_err(
            ErrorKind::DirectoryNotEmpty,
            "Directory not empty",
            dst_vpath,
        ));
    }
    if let Ok(target) = resolve_virtual_path(dst_vpath, ctx.mount_virtual)
        && host_is_dir(ctx.mount_dir, target.for_dir_op())
        && real_directory_has_visible_children(state, dst_rel, ctx.mount_dir, target.for_dir_op(), dst_vpath)?
    {
        return Err(MountError::io_err(
            ErrorKind::DirectoryNotEmpty,
            "Directory not empty",
            dst_vpath,
        ));
    }

    Ok(())
}

/// Recursively collects real descendants that should follow an overlay rename.
///
/// Each captured entry is charged for its key, its relative path, and
/// [`REAL_DESCENDANT_MEMORY_USAGE`] of container overhead against `budget`.
/// A descendant whose name is not valid UTF-8 fails the collection (and so the
/// rename) with `OSError`: overlay keys are `String`s, so such an entry cannot
/// follow the move and must not be silently dropped.
fn collect_real_descendants(
    dir: &Dir,
    root_rel: &str,
    prefix: &str,
    state: &OverlayState,
    already_handled: &AHashSet<String>,
    vpath: &str,
    budget: MemoryBudget,
) -> Result<Vec<(String, OverlayEntry)>, MountError> {
    let mut result = Vec::new();
    let mut dirs = vec![(root_rel.to_owned(), prefix.to_owned())];
    let mut memory_usage = as_u64(root_rel.len().saturating_add(prefix.len()));

    while let Some((current_rel, rel_prefix)) = dirs.pop() {
        let entries = dir.read_dir(&current_rel).map_err(|error| map_io(error, vpath))?;
        for entry in entries {
            let entry = entry.map_err(|error| map_io(error, vpath))?;
            let name = entry.file_name();
            // A non-UTF-8 name has no overlay-key representation, so the entry
            // cannot follow the rename. Refuse the whole operation: a lossy key
            // would look up a path that does not exist, silently leaving the
            // file behind in a directory the overlay then claims is deleted.
            let Some(name) = name.to_str() else {
                return Err(MountError::io_err(
                    ErrorKind::InvalidData,
                    "directory contains an entry with a non-UTF-8 name",
                    vpath,
                ));
            };
            let rel_key = format!("{rel_prefix}{name}");

            if state.get(&rel_key).is_some() || already_handled.contains(&rel_key) {
                continue;
            }

            let file_type = entry
                .file_type()
                .map_err(|error| MountError::Io(error, vpath.to_owned()))?;
            // A symlink can move neither way: capturing it as a file ref would
            // resolve the target the overlay refuses to write through, and
            // skipping it strands the link — unreachable at the new name, yet
            // still live at the old one inside a directory the overlay now
            // reports as deleted. Refuse the move, as for a non-UTF-8 name.
            if file_type.is_symlink() {
                return Err(MountError::PathEscape {
                    virtual_path: vpath.to_owned(),
                });
            }
            let child_rel = join_mount_relative(&current_rel, name);
            if file_type.is_file() {
                if let Some(file_ref) = OverlayFileRef::from_relative(dir, &child_rel) {
                    memory_usage = memory_usage
                        .saturating_add(as_u64(rel_key.len()))
                        .saturating_add(as_u64(file_ref.relative.len()))
                        .saturating_add(REAL_DESCENDANT_MEMORY_USAGE);
                    budget.check(memory_usage)?;
                    result.push((rel_key, OverlayEntry::RealFileRef(file_ref)));
                }
            } else if file_type.is_dir() {
                // Directory keys are charged twice: the result entry and the
                // recursion-queue prefix.
                memory_usage = memory_usage
                    .saturating_add(as_u64(rel_key.len().saturating_mul(2)))
                    .saturating_add(as_u64(child_rel.len()))
                    .saturating_add(REAL_DESCENDANT_MEMORY_USAGE);
                budget.check(memory_usage)?;
                result.push((
                    rel_key.clone(),
                    OverlayEntry::Directory {
                        mtime: host_dir_mtime(dir, &child_rel),
                    },
                ));
                dirs.push((child_rel, format!("{rel_key}/")));
            }
        }
    }

    Ok(result)
}

/// Resolves a real host path for an overlay fallthrough lookup.
///
/// Path *policy* rejections (null bytes, drive segments) always raise, as they
/// do in direct mode and CPython. A failed host *lookup* is governed by
/// `on_failure`: only a genuinely absent path is `Missing` for everyone.
fn resolve_real_path_state(
    vpath: &str,
    ctx: &MountContext<'_>,
    follow: Follow,
    on_failure: OnLookupFailure,
) -> Result<RealPathState, MountError> {
    let target = match resolve_virtual_path(vpath, ctx.mount_virtual) {
        Ok(target) => target,
        Err(MountError::Io(_, _)) => return Ok(RealPathState::Missing),
        Err(err) => return Err(err),
    };
    let rel = target.for_dir_op();
    let metadata = match follow {
        Follow::Yes => ctx.mount_dir.metadata(rel),
        Follow::No => ctx.mount_dir.symlink_metadata(rel),
    };
    match metadata {
        Ok(_) => Ok(RealPathState::Present(rel.to_owned())),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(RealPathState::Missing),
        Err(_) if matches!(on_failure, OnLookupFailure::Missing) => Ok(RealPathState::Missing),
        Err(err) => Err(map_io(err, vpath)),
    }
}

/// What a caller wants done with a host lookup that failed for a reason other
/// than the path being absent — a permission error, say.
#[derive(Clone, Copy)]
enum OnLookupFailure {
    /// Treat as absent. `pathlib` predicates answer `False` rather than raise,
    /// which also keeps out-of-mount paths unobservable.
    Missing,
    /// Report it, as a direct mount would. Collapsing it made `open` say
    /// `FileNotFoundError` for a denied file, and let `append` create an
    /// overlay entry shadowing a real file it could not see.
    Propagate,
}

/// Whether a lookup follows a final symlink, replacing the old resolve modes.
#[derive(Clone, Copy)]
enum Follow {
    /// Resolve the target, as `stat` does.
    Yes,
    /// Report the entry itself, as `lstat` does.
    No,
}

/// Result of resolving a real fallthrough path for overlay queries.
enum RealPathState {
    /// The path exists; the payload is its mount-relative form.
    Present(String),
    /// The path should behave as nonexistent.
    Missing,
}

/// What a *write*-path lookup found on the real filesystem at a mount-relative
/// path. Symlinks are surfaced, never resolved: overlay writes refuse them.
enum RealTarget {
    /// A directory.
    Dir,
    /// A non-directory, non-symlink entry.
    File,
    /// A symlink of any kind — its target is deliberately not examined.
    Symlink,
    /// Nothing there.
    Absent,
}

/// Classifies what sits at `rel` for overlay write paths.
///
/// Overlay writes never touch symlinks: a direct mount writes *through* a link
/// to its target, which the overlay cannot replicate without silently aliasing
/// the two names, so callers refuse `Symlink` with `PermissionError` wherever
/// it appears in a write path. The single `lstat` never resolves the target,
/// so the refusal reveals nothing about where the link points.
fn classify_write_target(dir: &Dir, rel: &str, vpath: &str) -> Result<RealTarget, MountError> {
    match dir.symlink_metadata(rel) {
        Ok(meta) if meta.is_symlink() => Ok(RealTarget::Symlink),
        Ok(meta) if meta.is_dir() => Ok(RealTarget::Dir),
        Ok(_) => Ok(RealTarget::File),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(RealTarget::Absent),
        // An escaping symlink earlier in the walk fails here; `map_io` turns
        // cap-std's synthetic denial into `PathEscape`.
        Err(err) => Err(map_io(err, vpath)),
    }
}

/// Returns the prefix used to scan direct children of `relative`.
fn directory_prefix(relative: &str) -> String {
    if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    }
}
