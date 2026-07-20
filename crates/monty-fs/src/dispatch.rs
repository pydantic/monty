//! Typed dispatch for filesystem OS calls.
//!
//! The mount table projects an [`OsFunctionCall`](crate::os::OsFunctionCall)
//! into [`FsRequest`] (an owned projection of the call's typed args). From
//! that point onward the backends operate on semantic requests with typed
//! fields rather than re-parsing `MontyObject` arrays or walking kwargs.
//! Ownership matters: write payloads are *moved* through here so overlay
//! storage can retain them without a copy.

use monty::{FileMode, MontyFileHandle, MontyObject, MontyPath, OsFunctionCall};

use super::{
    common::MountContext, direct, error::MountError, mount_mode::MountMode, overlay,
    path_security::normalize_virtual_path,
};

/// Parsed filesystem request passed to the direct or overlay backend.
#[derive(Debug)]
pub(super) enum FsRequest {
    /// `Path.exists()`
    Exists { path: MontyPath },
    /// `Path.is_file()`
    IsFile { path: MontyPath },
    /// `Path.is_dir()`
    IsDir { path: MontyPath },
    /// `Path.is_symlink()`
    IsSymlink { path: MontyPath },
    /// `Path.read_text()`
    ReadText { path: MontyPath },
    /// `Path.read_bytes()`
    ReadBytes { path: MontyPath },
    /// `Path.write_text(data)`
    WriteText { path: MontyPath, data: String },
    /// `Path.write_bytes(data)`
    WriteBytes { path: MontyPath, data: Vec<u8> },
    /// `Path.append_text(data)`
    AppendText { path: MontyPath, data: String },
    /// `Path.append_bytes(data)`
    AppendBytes { path: MontyPath, data: Vec<u8> },
    /// `Path.mkdir(parents=..., exist_ok=...)`
    Mkdir {
        /// Target path.
        path: MontyPath,
        /// Whether to create missing parents.
        parents: bool,
        /// Whether existing targets should be accepted.
        exist_ok: bool,
    },
    /// `Path.unlink()`
    Unlink { path: MontyPath },
    /// `Path.rmdir()`
    Rmdir { path: MontyPath },
    /// `Path.iterdir()`
    Iterdir { path: MontyPath },
    /// `Path.stat()`
    Stat { path: MontyPath },
    /// `Path.rename(dst)`
    Rename { src: MontyPath, dst: MontyPath },
    /// `Path.resolve()`
    Resolve { path: MontyPath },
    /// `Path.absolute()`
    Absolute { path: MontyPath },
    /// `open(path, mode)` — performs the open-time effect and returns a
    /// [`MontyObject::FileHandle`]. The mode is parsed once during dispatch
    /// so backends never re-parse the raw string.
    Open {
        /// Target path.
        path: MontyPath,
        /// Parsed `open()` mode.
        mode: FileMode,
    },
}

impl FsRequest {
    /// Returns the request's primary path for error reporting.
    #[must_use]
    pub fn primary_path(&self) -> &str {
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

    /// Returns whether the request mutates filesystem state.
    ///
    /// This is the read-only-mount gate (see [`execute`]). For `Open` it is
    /// mode-aware: `w`/`w+`/`a`/`a+` write (truncate or create), while pure
    /// `r`/`r+` only need read access.
    #[must_use]
    pub fn is_write(&self) -> bool {
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

/// Consumes an [`OsFunctionCall`] into a typed [`FsRequest`], moving write
/// payloads rather than copying them.
///
/// This is a trivial 1:1 mapping — every field is already typed on the
/// caller side (no `MontyObject` introspection, no kwarg walks, no mode
/// reparse), so the function is infallible. Non-FS variants are filtered
/// out by [`OsFunctionCall::is_filesystem`] before reaching here and panic
/// the catch-all arm if they slip through.
pub(super) fn fs_request_from_call(call: OsFunctionCall) -> FsRequest {
    match call {
        OsFunctionCall::Exists(path) => FsRequest::Exists { path },
        OsFunctionCall::IsFile(path) => FsRequest::IsFile { path },
        OsFunctionCall::IsDir(path) => FsRequest::IsDir { path },
        OsFunctionCall::IsSymlink(path) => FsRequest::IsSymlink { path },
        OsFunctionCall::ReadText(path) => FsRequest::ReadText { path },
        OsFunctionCall::ReadBytes(path) => FsRequest::ReadBytes { path },
        OsFunctionCall::WriteText(a) => FsRequest::WriteText {
            path: a.path,
            data: a.data,
        },
        OsFunctionCall::WriteBytes(a) => FsRequest::WriteBytes {
            path: a.path,
            data: a.data,
        },
        OsFunctionCall::AppendText(a) => FsRequest::AppendText {
            path: a.path,
            data: a.data,
        },
        OsFunctionCall::AppendBytes(a) => FsRequest::AppendBytes {
            path: a.path,
            data: a.data,
        },
        OsFunctionCall::Mkdir(a) => FsRequest::Mkdir {
            path: a.path,
            parents: a.parents,
            exist_ok: a.exist_ok,
        },
        OsFunctionCall::Unlink(path) => FsRequest::Unlink { path },
        OsFunctionCall::Rmdir(path) => FsRequest::Rmdir { path },
        OsFunctionCall::Iterdir(path) => FsRequest::Iterdir { path },
        OsFunctionCall::Stat(path) => FsRequest::Stat { path },
        OsFunctionCall::Rename(a) => FsRequest::Rename { src: a.src, dst: a.dst },
        OsFunctionCall::Resolve(path) => FsRequest::Resolve { path },
        OsFunctionCall::Absolute(path) => FsRequest::Absolute { path },
        OsFunctionCall::Open(a) => FsRequest::Open {
            path: a.path,
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
    request: FsRequest,
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
