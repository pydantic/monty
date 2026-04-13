//! Typed dispatch for filesystem OS calls.
//!
//! The mount table parses a raw [`OsFunction`](crate::os::OsFunction) call once
//! into [`FsRequest`]. From that point onward the backends operate on semantic
//! requests instead of indexing into `MontyObject` arrays or re-parsing kwargs.

use super::{common::MountContext, direct, error::MountError, mount_mode::MountMode, overlay};
use crate::{MontyObject, os::OsFunction};

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
            | Self::Mkdir { path, .. }
            | Self::Unlink { path }
            | Self::Rmdir { path }
            | Self::Iterdir { path }
            | Self::Stat { path }
            | Self::Resolve { path }
            | Self::Absolute { path }
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
    #[must_use]
    pub fn is_write(self) -> bool {
        matches!(
            self,
            Self::WriteText { .. }
                | Self::WriteBytes { .. }
                | Self::Mkdir { .. }
                | Self::Unlink { .. }
                | Self::Rmdir { .. }
                | Self::Rename { .. }
        )
    }
}

/// Parses a filesystem OS call into a typed request.
///
/// TODO: this should all be replaced with a proper solution
/// once we have https://github.com/pydantic/monty/issues/294
pub(super) fn parse_fs_request<'a>(
    function: OsFunction,
    args: &'a [MontyObject],
    kwargs: &'a [(MontyObject, MontyObject)],
) -> Result<FsRequest<'a>, MountError> {
    let path = parse_primary_path(args)?;
    let extra_args = &args[1..];

    match function {
        OsFunction::Exists => Ok(FsRequest::Exists { path }),
        OsFunction::IsFile => Ok(FsRequest::IsFile { path }),
        OsFunction::IsDir => Ok(FsRequest::IsDir { path }),
        OsFunction::IsSymlink => Ok(FsRequest::IsSymlink { path }),
        OsFunction::ReadText => Ok(FsRequest::ReadText { path }),
        OsFunction::ReadBytes => Ok(FsRequest::ReadBytes { path }),
        OsFunction::WriteText => Ok(FsRequest::WriteText {
            path,
            data: parse_string_data(extra_args, "write_text")?,
        }),
        OsFunction::WriteBytes => Ok(FsRequest::WriteBytes {
            path,
            data: parse_bytes_data(extra_args, "write_bytes")?,
        }),
        OsFunction::Mkdir => {
            let (parents, exist_ok) = parse_mkdir_kwargs(kwargs);
            Ok(FsRequest::Mkdir {
                path,
                parents,
                exist_ok,
            })
        }
        OsFunction::Unlink => Ok(FsRequest::Unlink { path }),
        OsFunction::Rmdir => Ok(FsRequest::Rmdir { path }),
        OsFunction::Iterdir => Ok(FsRequest::Iterdir { path }),
        OsFunction::Stat => Ok(FsRequest::Stat { path }),
        OsFunction::Rename => Ok(FsRequest::Rename {
            src: path,
            dst: parse_path_arg(extra_args, "rename")?,
        }),
        OsFunction::Resolve => Ok(FsRequest::Resolve { path }),
        OsFunction::Absolute => Ok(FsRequest::Absolute { path }),
        _ => unreachable!("non-filesystem OS function reached filesystem parser"),
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

/// Extracts the first path argument from a raw OS call.
fn parse_primary_path(args: &[MontyObject]) -> Result<&str, MountError> {
    match args.first() {
        Some(MontyObject::Path(path)) => Ok(path.as_str()),
        Some(MontyObject::String(path)) => Ok(path.as_str()),
        _ => Err(MountError::InvalidMount(
            "filesystem operation missing path argument".to_owned(),
        )),
    }
}

/// Extracts the string payload for `Path.write_text`.
fn parse_string_data<'a>(extra_args: &'a [MontyObject], op_name: &str) -> Result<&'a str, MountError> {
    match extra_args.first() {
        Some(MontyObject::String(data)) => Ok(data.as_str()),
        Some(arg) => Err(MountError::InvalidMount(format!(
            "data must be str, not {}",
            arg.type_name()
        ))),
        None => Err(MountError::InvalidMount(format!(
            "Path.{op_name}() missing 1 required positional argument: 'data'"
        ))),
    }
}

/// Extracts the bytes payload for `Path.write_bytes`.
fn parse_bytes_data<'a>(extra_args: &'a [MontyObject], op_name: &str) -> Result<&'a [u8], MountError> {
    match extra_args.first() {
        Some(MontyObject::Bytes(data)) => Ok(data.as_slice()),
        Some(arg) => Err(MountError::InvalidMount(format!(
            "memoryview: a bytes-like object is required, not '{}'",
            arg.type_name()
        ))),
        None => Err(MountError::InvalidMount(format!(
            "Path.{op_name}() missing 1 required positional argument: 'data'"
        ))),
    }
}

/// Extracts a path-like argument such as `Path.rename(dst)`.
fn parse_path_arg<'a>(extra_args: &'a [MontyObject], op_name: &str) -> Result<&'a str, MountError> {
    match extra_args.first() {
        Some(MontyObject::Path(path)) => Ok(path.as_str()),
        Some(MontyObject::String(path)) => Ok(path.as_str()),
        _ => Err(MountError::InvalidMount(format!("{op_name}: expected path argument"))),
    }
}

/// Extracts the supported keyword arguments for `Path.mkdir`.
fn parse_mkdir_kwargs(kwargs: &[(MontyObject, MontyObject)]) -> (bool, bool) {
    let mut parents = false;
    let mut exist_ok = false;

    for (key, value) in kwargs {
        if let (MontyObject::String(name), MontyObject::Bool(flag)) = (key, value) {
            match name.as_str() {
                "parents" => parents = *flag,
                "exist_ok" => exist_ok = *flag,
                _ => {}
            }
        }
    }

    (parents, exist_ok)
}
