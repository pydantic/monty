//! Path resolution and security checks for filesystem mounts.
//!
//! This module is the **sole security boundary** preventing sandbox escape via
//! filesystem access. All virtual-to-host path mapping goes through
//! [`resolve_path`], which enforces:
//!
//! - Null byte rejection
//! - Virtual-space normalization (removing `.` and `..`)
//! - Host-path canonicalization via [`fs::canonicalize`]
//! - Boundary checks ensuring the canonical path remains within the mount
//! - Symlink escape detection (symlinks resolving outside the mount are rejected)
//!
//! # Security Invariant
//!
//! **The monty runtime MUST NEVER read, write, or obtain any information about
//! any file or directory outside the specific directory that is mounted.**
//!
//! Changes to this module require careful security review.
//!
//! # TOCTOU Note
//!
//! There is an inherent time-of-check-to-time-of-use (TOCTOU) race between
//! path canonicalization and the subsequent filesystem operation. An attacker
//! with write access to the mounted directory could swap a regular file for a
//! symlink between the check and the operation. This is acceptable for the
//! initial implementation because:
//! - The attacker would need write access to the host directory being mounted
//! - A future enhancement could use `openat2(RESOLVE_BENEATH)` on Linux

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use super::error::MountError;

/// The result of successfully resolving a virtual path against a mount.
///
/// Contains both the real host path (for filesystem operations) and the
/// relative path within the mount (for overlay lookups).
#[derive(Debug)]
pub struct ResolvedPath {
    /// The validated, canonical host filesystem path to operate on.
    pub host_path: PathBuf,
    /// The relative path within the mount, using forward slashes.
    /// Used as the key for overlay state lookups.
    /// Example: `"subdir/file.txt"` for a file at `<mount>/subdir/file.txt`.
    pub relative_path: String,
}

/// Normalizes a virtual path by resolving `.` and `..` components.
///
/// The result is always an absolute path (starts with `/`). The `..` component
/// at the root level is silently ignored (cannot go above `/`), matching
/// POSIX behavior.
///
/// # Examples
///
/// - `/data/../etc/passwd` → `/etc/passwd`
/// - `/data/./file.txt` → `/data/file.txt`
/// - `/../../../etc/passwd` → `/etc/passwd`
#[must_use]
pub fn normalize_virtual_path(path: &str) -> String {
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
/// Both paths must be normalized (no `.` or `..`). Returns `""` if the path
/// exactly matches the mount prefix.
///
/// Returns `None` if the path doesn't start with the mount prefix.
///
/// This is exposed publicly for use by overlay operations that need to compute
/// relative paths without full host-path resolution.
#[must_use]
pub fn strip_mount_prefix_public<'a>(normalized_path: &'a str, mount_virtual_path: &str) -> Option<&'a str> {
    strip_mount_prefix(normalized_path, mount_virtual_path)
}

/// Internal implementation of mount prefix stripping.
fn strip_mount_prefix<'a>(normalized_path: &'a str, mount_virtual_path: &str) -> Option<&'a str> {
    if mount_virtual_path == "/" {
        // Root mount — everything matches. Strip the leading `/`.
        return Some(normalized_path.strip_prefix('/').unwrap_or(normalized_path));
    }

    if normalized_path == mount_virtual_path {
        return Some("");
    }

    // The path must start with the mount prefix followed by `/`.
    normalized_path
        .strip_prefix(mount_virtual_path)
        .and_then(|rest| rest.strip_prefix('/'))
}

/// Resolves a virtual path to a validated host filesystem path.
///
/// This is the primary security function for the mount system. It maps a
/// virtual path (as seen by sandbox code) to a real host path, with full
/// security checks to prevent sandbox escape.
///
/// # Security guarantees
///
/// - Rejects paths containing null bytes
/// - Normalizes away `.` and `..` in virtual space first
/// - After mapping to a host path, canonicalizes via [`fs::canonicalize`]
/// - Verifies the canonical path starts with the mount's canonical host path
/// - Rejects symlinks that resolve outside the mount boundary
/// - For non-existing paths: canonicalizes the parent directory, validates
///   the boundary, then appends the final component (which is checked for
///   path separators and `..`)
///
/// # CRITICAL
///
/// The monty runtime MUST NEVER read, write, or obtain any information about
/// any file or directory outside the specific directory that is mounted.
/// This function is the sole enforcement point for that invariant.
///
/// # Arguments
///
/// * `virtual_path` — The path as seen by sandbox code (e.g., `/data/file.txt`)
/// * `mount_virtual_path` — The virtual prefix of the mount (e.g., `/data`)
/// * `mount_host_path` — The canonical host directory backing the mount
/// * `for_creation` — If `true`, the path itself need not exist, but its parent must.
///   Used for write operations that create new files or directories.
pub fn resolve_path(
    virtual_path: &str,
    mount_virtual_path: &str,
    mount_host_path: &Path,
    for_creation: bool,
) -> Result<ResolvedPath, MountError> {
    // Step 1: Reject null bytes — these can truncate C strings and bypass checks.
    if virtual_path.contains('\0') {
        return Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        });
    }

    // Step 2: Normalize the virtual path to remove `.` and `..`.
    let normalized = normalize_virtual_path(virtual_path);

    // Step 3: Strip the mount prefix to get the relative portion.
    let relative = strip_mount_prefix(&normalized, mount_virtual_path)
        .ok_or_else(|| MountError::NoMountPoint(virtual_path.to_owned()))?;

    // Step 4: Construct the candidate host path.
    let candidate = if relative.is_empty() {
        mount_host_path.to_path_buf()
    } else {
        mount_host_path.join(relative)
    };

    // Step 5: Validate that the candidate doesn't contain suspicious components.
    // Even after virtual normalization, we verify the joined path has no `..`
    // components (defense in depth).
    for component in candidate.components() {
        if matches!(component, Component::ParentDir) {
            return Err(MountError::PathEscape {
                virtual_path: normalized,
            });
        }
    }

    // Step 6: Canonicalize and boundary-check.
    let host_path = if for_creation {
        resolve_for_creation(&candidate, mount_host_path, &normalized)?
    } else {
        resolve_existing(&candidate, mount_host_path, &normalized)?
    };

    Ok(ResolvedPath {
        host_path,
        relative_path: relative.to_owned(),
    })
}

/// Resolves an existing path by canonicalizing it and checking the mount boundary.
///
/// # Security
///
/// Uses [`fs::canonicalize`] which resolves ALL symlinks to their real
/// targets. If the resolved path escapes the mount boundary, returns
/// [`MountError::PathEscape`].
fn resolve_existing(candidate: &Path, mount_host_path: &Path, virtual_path: &str) -> Result<PathBuf, MountError> {
    let canonical = fs::canonicalize(candidate).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;

    check_boundary(&canonical, mount_host_path, virtual_path)?;
    Ok(canonical)
}

/// Resolves a path for creation by canonicalizing its parent directory.
///
/// The path itself doesn't need to exist, but its parent must. The final
/// component is validated to not contain path separators or `..`.
///
/// # Security
///
/// The parent directory is canonicalized and boundary-checked. The final
/// component is validated to prevent injection of path separators.
fn resolve_for_creation(candidate: &Path, mount_host_path: &Path, virtual_path: &str) -> Result<PathBuf, MountError> {
    // If the path already exists, just canonicalize it directly.
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

    // Validate the final component doesn't contain path separators or `..`.
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
///
/// # Security
///
/// This is the core boundary check. Both `canonical` and `mount_host_path`
/// must be canonical (fully resolved, no symlinks) for this check to be sound.
/// The mount's host path is canonicalized once at mount time.
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
