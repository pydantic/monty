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
//! ## Symlink handling
//!
//! Symbolic links are followed and then validated: [`fs::canonicalize`] resolves
//! all symlinks to their final target, and [`check_boundary`] verifies the
//! canonical path remains within the mount. Symlinks that resolve outside the
//! mount are rejected with [`MountError::PathEscape`].
//!
//! Hard links (created with `ln` rather than `ln -s`) are transparent to this
//! check — a hard link is just another directory entry for the same inode, so
//! `canonicalize` returns the path within the mount as-is. This is acceptable
//! because sandboxed code cannot create hard links (no `os.link` is exposed),
//! so hard links can only exist if the host placed them in the mounted
//! directory, which is an explicit choice to expose that content.
//!
//! ## TOCTOU considerations
//!
//! There is an inherent TOCTOU race between canonicalization and the subsequent
//! file operation. This is not a practical concern because:
//! - Sandboxed code cannot create symlinks (`os.symlink` is not exposed)
//! - Sandboxed code cannot spawn host processes to modify the filesystem
//! - Only the host can modify the mounted directory concurrently, and the host
//!   is trusted

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
///
/// `fs::canonicalize` resolves all symbolic links to their final target,
/// so any symlink that ultimately points outside the mount will be caught
/// by `check_boundary`. Hard links are not affected by canonicalization
/// (they are indistinguishable from regular files at the path level).
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

/// Resolves a path for `is_symlink()` checks without following the final symlink.
///
/// Canonicalizes and boundary-checks the **parent** directory, then appends the
/// final component without canonicalization. This preserves symlink identity so
/// that `Path::is_symlink()` on the returned path reports correctly.
///
/// # Security
///
/// The parent is fully canonicalized and boundary-checked, so we know the
/// directory is within the mount. We only inspect the metadata of a direct
/// child of a validated directory. Even if the symlink points outside the mount,
/// `is_symlink()` only reveals that the entry is a symlink, not its target.
pub(super) fn resolve_path_for_lstat(
    virtual_path: &str,
    mount_virtual_path: &str,
    mount_host_path: &Path,
) -> Result<ResolvedPath, MountError> {
    if virtual_path.contains('\0') {
        return Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        });
    }

    let normalized = normalize_virtual_path(virtual_path);
    let relative = strip_mount_prefix(&normalized, mount_virtual_path)
        .ok_or_else(|| MountError::NoMountPoint(virtual_path.to_owned()))?;

    // Mount root itself is a directory, never a symlink.
    if relative.is_empty() {
        let canonical = fs::canonicalize(mount_host_path).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;
        check_boundary(&canonical, mount_host_path, &normalized)?;
        return Ok(ResolvedPath { host_path: canonical });
    }

    let candidate = mount_host_path.join(relative);

    // Defense in depth: reject `..` in the joined host path.
    for component in candidate.components() {
        if matches!(component, Component::ParentDir) {
            return Err(MountError::PathEscape {
                virtual_path: normalized,
            });
        }
    }

    let parent = candidate.parent().ok_or_else(|| MountError::PathEscape {
        virtual_path: virtual_path.to_owned(),
    })?;

    let file_name = candidate.file_name().ok_or_else(|| MountError::PathEscape {
        virtual_path: virtual_path.to_owned(),
    })?;

    // Canonicalize the parent to resolve any symlinks in ancestor directories,
    // then boundary-check it. The final component is NOT canonicalized.
    let canonical_parent = fs::canonicalize(parent).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;
    check_boundary(&canonical_parent, mount_host_path, &normalized)?;

    Ok(ResolvedPath {
        host_path: canonical_parent.join(file_name),
    })
}

/// Resolves a path for `mkdir -p` where intermediate directories may not exist.
///
/// Walks from the mount root downward through existing path components,
/// canonicalizing at each step to detect symlinks that escape the mount.
/// Once a non-existent component is found, the remaining components are
/// appended lexically (they will be created by `create_dir_all`).
///
/// This prevents a symlinked intermediate directory from redirecting
/// `create_dir_all` outside the mount boundary.
pub(super) fn resolve_path_mkdir_parents(
    virtual_path: &str,
    mount_virtual_path: &str,
    mount_host_path: &Path,
) -> Result<ResolvedPath, MountError> {
    if virtual_path.contains('\0') {
        return Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        });
    }

    let normalized = normalize_virtual_path(virtual_path);
    let relative = strip_mount_prefix(&normalized, mount_virtual_path)
        .ok_or_else(|| MountError::NoMountPoint(virtual_path.to_owned()))?;

    if relative.is_empty() {
        // Creating the mount root itself — just canonicalize it.
        let canonical = fs::canonicalize(mount_host_path).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;
        check_boundary(&canonical, mount_host_path, &normalized)?;
        return Ok(ResolvedPath { host_path: canonical });
    }

    // Walk through each component, canonicalizing each existing ancestor
    // to ensure symlinks don't escape the mount.
    let components: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();
    let mut current = mount_host_path.to_path_buf();

    for (i, component) in components.iter().enumerate() {
        // Defense in depth: reject traversal components even though
        // normalize_virtual_path should have removed them.
        if *component == ".." || *component == "." {
            return Err(MountError::PathEscape {
                virtual_path: normalized,
            });
        }

        let next = current.join(component);
        if next.exists() {
            // Canonicalize to resolve symlinks and check boundary.
            let canonical = fs::canonicalize(&next).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;
            check_boundary(&canonical, mount_host_path, &normalized)?;
            current = canonical;
        } else {
            // This component doesn't exist yet — append all remaining
            // components lexically and return. They'll be created by
            // create_dir_all.
            for remaining in &components[i..] {
                current = current.join(remaining);
            }
            return Ok(ResolvedPath { host_path: current });
        }
    }

    // All components exist and passed boundary checks.
    Ok(ResolvedPath { host_path: current })
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
