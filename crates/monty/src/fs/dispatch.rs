//! Typed dispatch for filesystem OS calls.
//!
//! The mount table projects an [`OsFunctionCall`](crate::os::OsFunctionCall) into
//! [`FsRequest`] (a borrowed view over the call's typed args). From that point
//! onward the backends operate on semantic requests with `&str` / `FileMode`
//! fields rather than re-parsing `MontyObject` arrays or walking kwargs.

use super::{
    common::MountContext, direct, error::MountError, mount_mode::MountMode, overlay,
    path_security::normalize_virtual_path,
};
use crate::{MontyFileHandle, MontyObject, os::OsFunctionCall, types::file::FileMode};

/// Parsed filesystem request passed to the direct or overlay backend.
#[derive(Clone, Copy, Debug)]
pub(super) enum FsRequest<'a> {
    /// `Path.exists()`
    Exists { path: &'a str },
    /// `Path.is_file()`
    IsFile { path: &'a str },
    /// `Path.is_dir()`
    IsDir { path: &'a str },
    /// `Path.is_symlink()`
    IsSymlink { path: &'a str },
    /// `Path.read_text()`
    ReadText { path: &'a str },
    /// `Path.read_bytes()`
    ReadBytes { path: &'a str },
    /// `Path.write_text(data)`
    WriteText { path: &'a str, data: &'a str },
    /// `Path.write_bytes(data)`
    WriteBytes { path: &'a str, data: &'a [u8] },
    /// `Path.append_text(data)`
    AppendText { path: &'a str, data: &'a str },
    /// `Path.append_bytes(data)`
    AppendBytes { path: &'a str, data: &'a [u8] },
    /// `Path.mkdir(parents=..., exist_ok=...)`
    Mkdir {
        /// Target path.
        path: &'a str,
        /// Whether to create missing parents.
        parents: bool,
        /// Whether existing targets should be accepted.
        exist_ok: bool,
    },
    /// `Path.unlink()`
    Unlink { path: &'a str },
    /// `Path.rmdir()`
    Rmdir { path: &'a str },
    /// `Path.iterdir()`
    Iterdir { path: &'a str },
    /// `Path.stat()`
    Stat { path: &'a str },
    /// `Path.rename(dst)`
    Rename { src: &'a str, dst: &'a str },
    /// `Path.resolve()`
    Resolve { path: &'a str },
    /// `Path.absolute()`
    Absolute { path: &'a str },
    /// `open(path, mode)` — performs the open-time effect and returns a
    /// [`MontyObject::FileHandle`]. The mode is parsed once during dispatch
    /// so backends never re-parse the raw string.
    Open {
        /// Target path.
        path: &'a str,
        /// Parsed `open()` mode.
        mode: FileMode,
    },
}

impl<'a> FsRequest<'a> {
    /// Returns the request's primary path for mount lookup and error reporting.
    #[must_use]
    pub fn primary_path(self) -> &'a str {
        match self {
            Self::Exists { path }
            | Self::IsFile { path }
            | Self::IsDir { path }
            | Self::IsSymlink { path }
            | Self::ReadText { path }
            | Self::ReadBytes { path }
            | Self::WriteText { path, .. }
            | Self::WriteBytes { path, .. }
            | Self::AppendText { path, .. }
            | Self::AppendBytes { path, .. }
            | Self::Mkdir { path, .. }
            | Self::Unlink { path }
            | Self::Rmdir { path }
            | Self::Iterdir { path }
            | Self::Stat { path }
            | Self::Resolve { path }
            | Self::Absolute { path }
            | Self::Open { path, .. }
            | Self::Rename { src: path, .. } => path,
        }
    }

    /// Returns the rename destination when this request is a rename.
    #[must_use]
    pub fn rename_destination(self) -> Option<&'a str> {
        match self {
            Self::Rename { dst, .. } => Some(dst),
            _ => None,
        }
    }

    /// Returns whether the request mutates filesystem state.
    ///
    /// This is the read-only-mount gate (see [`execute`]). For `Open` it is
    /// mode-aware: `w`/`w+`/`a`/`a+` write (truncate or create), while pure
    /// `r`/`r+` only need read access.
    #[must_use]
    pub fn is_write(self) -> bool {
        match self {
            Self::WriteText { .. }
            | Self::WriteBytes { .. }
            | Self::AppendText { .. }
            | Self::AppendBytes { .. }
            | Self::Mkdir { .. }
            | Self::Unlink { .. }
            | Self::Rmdir { .. }
            | Self::Rename { .. } => true,
            Self::Open { mode, .. } => mode.create(),
            _ => false,
        }
    }
}

/// Projects an [`OsFunctionCall`] into a typed [`FsRequest`].
///
/// This is a trivial 1:1 mapping — every field is already typed on the
/// caller side (no `MontyObject` introspection, no kwarg walks, no mode
/// reparse), so the function is infallible. Non-FS variants are filtered
/// out by [`OsFunctionCall::is_filesystem`] before reaching here and panic
/// the catch-all arm if they slip through.
pub(super) fn fs_request_from_call(call: &OsFunctionCall) -> FsRequest<'_> {
    match call {
        OsFunctionCall::Exists(p) => FsRequest::Exists { path: p.as_str() },
        OsFunctionCall::IsFile(p) => FsRequest::IsFile { path: p.as_str() },
        OsFunctionCall::IsDir(p) => FsRequest::IsDir { path: p.as_str() },
        OsFunctionCall::IsSymlink(p) => FsRequest::IsSymlink { path: p.as_str() },
        OsFunctionCall::ReadText(p) => FsRequest::ReadText { path: p.as_str() },
        OsFunctionCall::ReadBytes(p) => FsRequest::ReadBytes { path: p.as_str() },
        OsFunctionCall::WriteText(a) => FsRequest::WriteText {
            path: a.path.as_str(),
            data: a.data.as_str(),
        },
        OsFunctionCall::WriteBytes(a) => FsRequest::WriteBytes {
            path: a.path.as_str(),
            data: a.data.as_slice(),
        },
        OsFunctionCall::AppendText(a) => FsRequest::AppendText {
            path: a.path.as_str(),
            data: a.data.as_str(),
        },
        OsFunctionCall::AppendBytes(a) => FsRequest::AppendBytes {
            path: a.path.as_str(),
            data: a.data.as_slice(),
        },
        OsFunctionCall::Mkdir(a) => FsRequest::Mkdir {
            path: a.path.as_str(),
            parents: a.parents,
            exist_ok: a.exist_ok,
        },
        OsFunctionCall::Unlink(p) => FsRequest::Unlink { path: p.as_str() },
        OsFunctionCall::Rmdir(p) => FsRequest::Rmdir { path: p.as_str() },
        OsFunctionCall::Iterdir(p) => FsRequest::Iterdir { path: p.as_str() },
        OsFunctionCall::Stat(p) => FsRequest::Stat { path: p.as_str() },
        OsFunctionCall::Rename(a) => FsRequest::Rename {
            src: a.src.as_str(),
            dst: a.dst.as_str(),
        },
        OsFunctionCall::Resolve(p) => FsRequest::Resolve { path: p.as_str() },
        OsFunctionCall::Absolute(p) => FsRequest::Absolute { path: p.as_str() },
        OsFunctionCall::Open(a) => FsRequest::Open {
            path: a.path.as_str(),
            mode: a.mode,
        },
        OsFunctionCall::Getenv(_)
        | OsFunctionCall::GetEnviron
        | OsFunctionCall::DateToday
        | OsFunctionCall::DateTimeNow(_) => unreachable!("non-filesystem OS function reached filesystem parser"),
        OsFunctionCall::Used => unreachable!("OsFunctionCall::Used reached filesystem parser"),
    }
}

/// Routes a parsed request to the correct backend for the mount mode.
pub(super) fn execute(
    request: FsRequest<'_>,
    ctx: &mut MountContext<'_>,
    mode: &mut MountMode,
) -> Result<MontyObject, MountError> {
    if request.is_write() && matches!(mode, MountMode::ReadOnly) {
        Err(MountError::ReadOnly(request.primary_path().to_owned()))
    } else {
        match mode {
            MountMode::ReadWrite | MountMode::ReadOnly => direct::execute(request, ctx),
            MountMode::OverlayMemory(state) => overlay::execute(request, ctx, state),
        }
    }
}

/// Builds the [`MontyObject::FileHandle`] an `Open` request resolves to.
///
/// The handle carries the **virtual** (sandbox) path — never a host path — so
/// subsequent `read`/`write` calls re-resolve it through `resolve_path`.
pub(super) fn file_handle_result(path: &str, mode: FileMode) -> MontyObject {
    MontyObject::FileHandle(MontyFileHandle {
        path: normalize_virtual_path(path),
        mode,
        position: 0,
    })
}
