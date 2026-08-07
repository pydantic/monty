//! Virtual path handling for filesystem mounts.
//!
//! Maps sandbox virtual paths onto mount-relative paths. This is **not** the
//! sandbox boundary — that is the mount's `Dir` descriptor (see
//! [`MountContext::mount_dir`]). What remains here is Monty path policy:
//! normalization, null-byte rejection, and length limits applied uniformly
//! across hosts.
//!
//! [`MountContext::mount_dir`]: super::common::MountContext::mount_dir

use std::io::ErrorKind;

use super::error::MountError;

/// Maximum total path length in bytes (Linux `PATH_MAX`).
const PATH_MAX: usize = 4096;

/// Maximum single path component length in bytes (universal `NAME_MAX`).
const NAME_MAX: usize = 255;

/// A virtual path checked against Monty's path policy and made relative to its
/// mount, ready to hand to a [`Dir`](cap_std::fs::Dir) method.
///
/// `relative` is empty for the mount root itself; `cap-std` treats `""` as an
/// error, so callers wanting the root use `"."` via [`Self::for_dir_op`].
#[derive(Debug)]
pub(super) struct MountRelativePath {
    /// Path relative to the mount root, with no `.` or `..` components.
    relative: String,
}

impl MountRelativePath {
    /// The path to pass to a `Dir` method, mapping the mount root to `.`.
    pub fn for_dir_op(&self) -> &str {
        if self.relative.is_empty() { "." } else { &self.relative }
    }

    /// Whether this refers to the mount root rather than something inside it.
    pub fn is_mount_root(&self) -> bool {
        self.relative.is_empty()
    }
}

/// Maps a virtual path into its mount-relative form, applying path policy.
///
/// Rejects null bytes and over-long paths/components, and resolves `.` and `..`
/// within the *virtual* namespace so `..` cannot climb out of the sandbox's own
/// view. Escaping the mount on the host side is not this function's job: the
/// `Dir` descriptor makes it impossible.
pub(super) fn resolve_virtual_path(
    virtual_path: &str,
    mount_virtual_path: &str,
) -> Result<MountRelativePath, MountError> {
    reject_null_bytes(virtual_path)?;
    reject_overlong_path(virtual_path)?;

    let normalized = normalize_virtual_path(virtual_path);
    let relative = strip_mount_prefix(&normalized, mount_virtual_path)
        .ok_or_else(|| MountError::NoMountPoint(virtual_path.to_owned()))?
        .to_owned();
    reject_drive_or_unc_segments(&relative, &normalized)?;

    Ok(MountRelativePath { relative })
}

/// Normalizes a virtual sandbox path by removing `.` and resolving `..`.
///
/// The result is always absolute. Excess `..` components at the root collapse
/// to `/` instead of escaping the sandbox namespace.
#[must_use]
pub(super) fn normalize_virtual_path(path: &str) -> String {
    if is_already_normalized_absolute_path(path) {
        return path.to_owned();
    }

    let mut components = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(part),
        }
    }

    if components.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", components.join("/"))
    }
}

/// Strips a normalized mount prefix from a normalized sandbox path.
#[must_use]
pub(super) fn strip_mount_prefix<'a>(normalized_path: &'a str, mount_virtual_path: &str) -> Option<&'a str> {
    if mount_virtual_path == "/" {
        return Some(normalized_path.strip_prefix('/').unwrap_or(normalized_path));
    }

    if normalized_path == mount_virtual_path {
        return Some("");
    }

    normalized_path
        .strip_prefix(mount_virtual_path)
        .and_then(|rest| rest.strip_prefix('/'))
}

/// Rejects segments a host parser treats as drive/UNC/root-absolute (`C:\x`,
/// `C:`, `\\host\share`).
///
/// Windows parses these as a path prefix and the descriptor refuses them; Unix
/// reads them as ordinary filenames. Rejecting everywhere keeps hosts identical.
/// `OverlayMemory` needs it independently — its keys never reach the filesystem,
/// so the descriptor never sees them (#655).
pub(super) fn reject_drive_or_unc_segments(relative: &str, normalized_virtual_path: &str) -> Result<(), MountError> {
    // A backslash can only smuggle a Windows separator/UNC/root prefix; `X:` a drive.
    let has_escape_prefix = relative.contains('\\') || relative.split('/').any(is_windows_drive_prefix);
    if has_escape_prefix {
        Err(MountError::PathEscape {
            virtual_path: normalized_virtual_path.to_owned(),
        })
    } else {
        Ok(())
    }
}

/// Whether `segment` starts with a Windows drive prefix (`X:`).
fn is_windows_drive_prefix(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Rejects embedded null bytes before any path manipulation occurs.
fn reject_null_bytes(virtual_path: &str) -> Result<(), MountError> {
    if virtual_path.contains('\0') {
        Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        })
    } else {
        Ok(())
    }
}

/// Rejects paths that exceed Linux filesystem length limits.
///
/// Applied regardless of host OS so the sandbox behaves identically everywhere,
/// rather than inheriting whatever the host filesystem happens to allow. Measures
/// the path as sent, before normalization: a request padded with `..` collapses
/// short, and the kernel would reject the bytes handed to it, not the collapsed
/// form. Checking first also keeps the per-segment normalization off oversized
/// input.
pub(super) fn reject_overlong_path(path: &str) -> Result<(), MountError> {
    let too_long = path.len() > PATH_MAX || path.split('/').any(|component| component.len() > NAME_MAX);
    if too_long {
        Err(MountError::io_err(
            ErrorKind::InvalidFilename,
            "File name too long",
            path,
        ))
    } else {
        Ok(())
    }
}

/// Fast path for paths already in normalized absolute form.
fn is_already_normalized_absolute_path(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    if path == "/" {
        return true;
    }
    if path.ends_with('/') {
        return false;
    }

    !path
        .split('/')
        .skip(1)
        .any(|component| component.is_empty() || component == "." || component == "..")
}
