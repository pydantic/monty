//! Path resolution and security checks for filesystem mounts.
//!
//! This module is the **sole security boundary** preventing sandbox escape via
//! filesystem access. All virtual-to-host path mapping goes through
//! [`resolve_path`], which enforces null byte rejection, virtual-space
//! normalization, host-path canonicalization, boundary checks, and symlink
//! escape detection.
//!
//! **The monty runtime MUST NEVER read, write, or obtain any information about
//! any file or directory outside the specific directory that is mounted.**
//!
//! Changes to this module require careful security review.
//!
//! There is an inherent TOCTOU race between canonicalization and the subsequent
//! operation; a future enhancement could use `openat2(RESOLVE_BENEATH)` on Linux.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use super::error::MountError;

/// Result of successfully resolving a virtual path against a mount.
#[derive(Debug)]
pub(super) struct ResolvedPath {
    /// The validated, canonical host filesystem path.
    pub host_path: PathBuf,
}

/// Resolves a virtual path to a validated host filesystem path.
///
/// # Security guarantees
///
/// - Rejects paths containing null bytes
/// - Normalizes `.` and `..` in virtual space
/// - Canonicalizes the host path via [`fs::canonicalize`]
/// - Verifies the canonical path remains within the mount boundary
/// - Rejects symlinks that resolve outside the mount
/// - For new paths (`for_creation`): canonicalizes the parent, validates the
///   boundary, and checks the final component for path separators
pub(super) fn resolve_path(
    virtual_path: &str,
    mount_virtual_path: &str,
    mount_host_path: &Path,
    for_creation: bool,
) -> Result<ResolvedPath, MountError> {
    // Reject null bytes — can truncate C strings and bypass checks.
    if virtual_path.contains('\0') {
        return Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        });
    }

    let normalized = normalize_virtual_path(virtual_path);

    let relative = strip_mount_prefix(&normalized, mount_virtual_path)
        .ok_or_else(|| MountError::NoMountPoint(virtual_path.to_owned()))?;

    let candidate = if relative.is_empty() {
        mount_host_path.to_path_buf()
    } else {
        mount_host_path.join(relative)
    };

    // Defense in depth: reject `..` in the joined host path even though
    // virtual normalization should have removed them.
    for component in candidate.components() {
        if matches!(component, Component::ParentDir) {
            return Err(MountError::PathEscape {
                virtual_path: normalized,
            });
        }
    }

    let host_path = if for_creation {
        resolve_for_creation(&candidate, mount_host_path, &normalized)?
    } else {
        resolve_existing(&candidate, mount_host_path, &normalized)?
    };

    Ok(ResolvedPath { host_path })
}

/// Normalizes a virtual path by resolving `.` and `..` components.
///
/// Always returns an absolute path. `..` at the root is silently ignored.
#[must_use]
pub(super) fn normalize_virtual_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();

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

/// Strips a mount prefix from a normalized virtual path, returning the relative portion.
///
/// Both paths must be normalized. Returns `""` if the path exactly matches the
/// mount prefix, or `None` if it doesn't match.
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

// =============================================================================
// Private helpers
// =============================================================================

/// Canonicalizes an existing path and checks the mount boundary.
fn resolve_existing(candidate: &Path, mount_host_path: &Path, virtual_path: &str) -> Result<PathBuf, MountError> {
    let canonical = fs::canonicalize(candidate).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;
    check_boundary(&canonical, mount_host_path, virtual_path)?;
    Ok(canonical)
}

/// Canonicalizes the parent of a not-yet-existing path and checks the mount boundary.
fn resolve_for_creation(candidate: &Path, mount_host_path: &Path, virtual_path: &str) -> Result<PathBuf, MountError> {
    if candidate.exists() {
        return resolve_existing(candidate, mount_host_path, virtual_path);
    }

    let parent = candidate.parent().ok_or_else(|| MountError::PathEscape {
        virtual_path: virtual_path.to_owned(),
    })?;

    let file_name = candidate
        .file_name()
        .ok_or_else(|| MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        })?
        .to_str()
        .ok_or_else(|| MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        })?;

    if file_name.contains('/') || file_name.contains('\\') || file_name == ".." || file_name == "." {
        return Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        });
    }

    let canonical_parent = fs::canonicalize(parent).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;
    check_boundary(&canonical_parent, mount_host_path, virtual_path)?;
    Ok(canonical_parent.join(file_name))
}

/// Verifies that a canonical path is within the mount boundary.
fn check_boundary(canonical: &Path, mount_host_path: &Path, virtual_path: &str) -> Result<(), MountError> {
    if canonical.starts_with(mount_host_path) {
        Ok(())
    } else {
        Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_virtual_path() {
        assert_eq!(normalize_virtual_path("/data/file.txt"), "/data/file.txt");
        assert_eq!(normalize_virtual_path("/data/./file.txt"), "/data/file.txt");
        assert_eq!(normalize_virtual_path("/data/../etc/passwd"), "/etc/passwd");
        assert_eq!(normalize_virtual_path("/../../../etc/passwd"), "/etc/passwd");
        assert_eq!(normalize_virtual_path("/"), "/");
        assert_eq!(normalize_virtual_path("/data/"), "/data");
        assert_eq!(normalize_virtual_path("/a/b/../c/./d"), "/a/c/d");
    }

    #[test]
    fn test_strip_mount_prefix() {
        assert_eq!(strip_mount_prefix("/data/file.txt", "/data"), Some("file.txt"));
        assert_eq!(strip_mount_prefix("/data", "/data"), Some(""));
        assert_eq!(strip_mount_prefix("/data/sub/file", "/data"), Some("sub/file"));
        assert_eq!(strip_mount_prefix("/other/file", "/data"), None);
        // Root mount
        assert_eq!(strip_mount_prefix("/anything", "/"), Some("anything"));
        assert_eq!(strip_mount_prefix("/", "/"), Some(""));
        // Must not match partial prefixes
        assert_eq!(strip_mount_prefix("/data2/file", "/data"), None);
    }
}
