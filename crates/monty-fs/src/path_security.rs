//! Path resolution and security checks for filesystem mounts.
//!
//! This module is the sole security boundary between sandbox virtual paths and
//! host filesystem paths. Every virtual-path lookup flows through
//! [`resolve_path`], which performs normalization, mount membership checks,
//! host-path construction, symlink-aware canonicalization, and final boundary
//! validation before any host path is returned to the caller.
//!
//! The key invariant is that sandbox code must never learn about or access
//! filesystem state outside the mounted host directory.

use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use super::error::MountError;

/// Maximum total path length in bytes (Linux `PATH_MAX`).
const PATH_MAX: usize = 4096;

/// Maximum single path component length in bytes (universal `NAME_MAX`).
const NAME_MAX: usize = 255;

/// Host path returned after successful security validation.
#[derive(Debug)]
pub(super) struct ResolvedPath {
    /// Validated host filesystem path suitable for the requested operation.
    pub host_path: PathBuf,
}

/// Resolution strategy for a filesystem operation.
///
/// Each mode shares the same preprocessing pipeline and only differs in how the
/// final host path is validated and canonicalized.
#[derive(Clone, Copy, Debug)]
pub(super) enum ResolveMode {
    /// Resolve an existing path by canonicalizing the full target.
    Existing,
    /// Resolve a path for `lstat`-style operations that must preserve the final
    /// symlink entry rather than following it.
    Lstat,
    /// Resolve a creation target by canonicalizing the parent and validating
    /// the final component.
    Creation,
    /// Resolve a `mkdir(parents=True)` target by walking existing ancestors and
    /// then appending missing components lexically.
    MkdirParents,
}

/// Resolves a virtual path into a validated host path for `mode`.
pub(super) fn resolve_path(
    virtual_path: &str,
    mount_virtual_path: &str,
    mount_host_path: &Path,
    mode: ResolveMode,
) -> Result<ResolvedPath, MountError> {
    let request = ResolutionRequest::new(virtual_path, mount_virtual_path, mount_host_path)?;
    let host_path = match mode {
        ResolveMode::Existing => resolve_existing(&request, mount_host_path)?,
        ResolveMode::Lstat => resolve_lstat(&request, mount_host_path)?,
        ResolveMode::Creation => resolve_creation(&request, mount_host_path)?,
        ResolveMode::MkdirParents => resolve_mkdir_parents(&request, mount_host_path)?,
    };
    Ok(ResolvedPath { host_path })
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

/// Shared preprocessed resolution input.
struct ResolutionRequest {
    /// Normalized absolute sandbox path used for boundary-safe error reporting.
    normalized_virtual: String,
    /// Mount-relative path inside the sandbox namespace.
    relative: String,
    /// Lexically joined host candidate path before mode-specific validation.
    candidate_host: PathBuf,
}

impl ResolutionRequest {
    /// Builds the normalized resolution request shared by all modes.
    fn new(virtual_path: &str, mount_virtual_path: &str, mount_host_path: &Path) -> Result<Self, MountError> {
        reject_null_bytes(virtual_path)?;

        let normalized_virtual = normalize_virtual_path(virtual_path);
        reject_overlong_path(&normalized_virtual, virtual_path)?;
        let relative = strip_mount_prefix(&normalized_virtual, mount_virtual_path)
            .ok_or_else(|| MountError::NoMountPoint(virtual_path.to_owned()))?
            .to_owned();

        let candidate_host = if relative.is_empty() {
            mount_host_path.to_path_buf()
        } else {
            mount_host_path.join(&relative)
        };
        reject_parent_components(&candidate_host, &normalized_virtual)?;

        Ok(Self {
            normalized_virtual,
            relative,
            candidate_host,
        })
    }

    /// Returns the final path component as a UTF-8 file name.
    fn final_component(&self) -> Result<&str, MountError> {
        let file_name = self
            .candidate_host
            .file_name()
            .ok_or_else(|| MountError::PathEscape {
                virtual_path: self.normalized_virtual.clone(),
            })?
            .to_str()
            .ok_or_else(|| MountError::PathEscape {
                virtual_path: self.normalized_virtual.clone(),
            })?;

        if file_name.contains('/') || file_name.contains('\\') || matches!(file_name, "." | "..") {
            return Err(MountError::PathEscape {
                virtual_path: self.normalized_virtual.clone(),
            });
        }

        Ok(file_name)
    }
}

/// Resolves an existing path by canonicalizing the full target.
fn resolve_existing(request: &ResolutionRequest, mount_host_path: &Path) -> Result<PathBuf, MountError> {
    let canonical = fs::canonicalize(&request.candidate_host)
        .map_err(|err| MountError::Io(err, request.normalized_virtual.clone()))?;
    check_boundary(&canonical, mount_host_path, &request.normalized_virtual)?;
    Ok(canonical)
}

/// Resolves a path for `lstat`-style calls without following the final component.
fn resolve_lstat(request: &ResolutionRequest, mount_host_path: &Path) -> Result<PathBuf, MountError> {
    if request.relative.is_empty() {
        let canonical =
            fs::canonicalize(mount_host_path).map_err(|err| MountError::Io(err, request.normalized_virtual.clone()))?;
        check_boundary(&canonical, mount_host_path, &request.normalized_virtual)?;
        return Ok(canonical);
    }

    let parent = request.candidate_host.parent().ok_or_else(|| MountError::PathEscape {
        virtual_path: request.normalized_virtual.clone(),
    })?;
    let file_name = request
        .candidate_host
        .file_name()
        .ok_or_else(|| MountError::PathEscape {
            virtual_path: request.normalized_virtual.clone(),
        })?;

    let canonical_parent =
        fs::canonicalize(parent).map_err(|err| MountError::Io(err, request.normalized_virtual.clone()))?;
    check_boundary(&canonical_parent, mount_host_path, &request.normalized_virtual)?;
    Ok(canonical_parent.join(file_name))
}

/// Resolves a path for creation by validating the parent directory first.
fn resolve_creation(request: &ResolutionRequest, mount_host_path: &Path) -> Result<PathBuf, MountError> {
    if request.candidate_host.exists() {
        return resolve_existing(request, mount_host_path);
    }

    let parent = request.candidate_host.parent().ok_or_else(|| MountError::PathEscape {
        virtual_path: request.normalized_virtual.clone(),
    })?;
    let file_name = request.final_component()?;

    let canonical_parent =
        fs::canonicalize(parent).map_err(|err| MountError::Io(err, request.normalized_virtual.clone()))?;
    check_boundary(&canonical_parent, mount_host_path, &request.normalized_virtual)?;

    let resolved_path = canonical_parent.join(file_name);
    validate_creation_symlink_target(
        &resolved_path,
        &canonical_parent,
        mount_host_path,
        &request.normalized_virtual,
    )?;
    Ok(resolved_path)
}

/// Resolves a path for `mkdir(parents=True)` while checking every existing ancestor.
fn resolve_mkdir_parents(request: &ResolutionRequest, mount_host_path: &Path) -> Result<PathBuf, MountError> {
    if request.relative.is_empty() {
        let canonical =
            fs::canonicalize(mount_host_path).map_err(|err| MountError::Io(err, request.normalized_virtual.clone()))?;
        check_boundary(&canonical, mount_host_path, &request.normalized_virtual)?;
        return Ok(canonical);
    }

    let components: Vec<&str> = request
        .relative
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    let mut current = mount_host_path.to_path_buf();

    for (index, component) in components.iter().enumerate() {
        if matches!(*component, "." | "..") {
            return Err(MountError::PathEscape {
                virtual_path: request.normalized_virtual.clone(),
            });
        }

        let next = current.join(component);
        if next.exists() {
            let canonical =
                fs::canonicalize(&next).map_err(|err| MountError::Io(err, request.normalized_virtual.clone()))?;
            check_boundary(&canonical, mount_host_path, &request.normalized_virtual)?;
            current = canonical;
        } else {
            for remaining in &components[index..] {
                current = current.join(remaining);
            }
            return Ok(current);
        }
    }

    Ok(current)
}

/// Rejects embedded null bytes before any path manipulation occurs.
fn reject_null_bytes(virtual_path: &str) -> Result<(), MountError> {
    if virtual_path.contains('\0') {
        return Err(MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        });
    }
    Ok(())
}

/// Rejects paths that exceed Linux filesystem length limits.
///
/// Enforces `PATH_MAX` (4096) for the total normalized path and `NAME_MAX`
/// (255) for each individual component. These match Linux defaults and are
/// applied regardless of the host OS so the sandbox behaves consistently.
pub(super) fn reject_overlong_path(normalized: &str, original: &str) -> Result<(), MountError> {
    if normalized.len() > PATH_MAX {
        return Err(MountError::io_err(
            ErrorKind::InvalidFilename,
            "File name too long",
            original,
        ));
    }
    for component in normalized.split('/') {
        if component.len() > NAME_MAX {
            return Err(MountError::io_err(
                ErrorKind::InvalidFilename,
                "File name too long",
                original,
            ));
        }
    }
    Ok(())
}

/// Rejects `..` components in the joined host candidate as defense in depth.
fn reject_parent_components(candidate_host_path: &Path, normalized_virtual_path: &str) -> Result<(), MountError> {
    for component in candidate_host_path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(MountError::PathEscape {
                virtual_path: normalized_virtual_path.to_owned(),
            });
        }
    }
    Ok(())
}

/// Validates that creation does not follow a dangling or outbound final symlink.
fn validate_creation_symlink_target(
    resolved_path: &Path,
    canonical_parent: &Path,
    mount_host_path: &Path,
    normalized_virtual_path: &str,
) -> Result<(), MountError> {
    if !resolved_path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Ok(());
    }

    let link_target =
        fs::read_link(resolved_path).map_err(|err| MountError::Io(err, normalized_virtual_path.to_owned()))?;
    let resolved_target = if link_target.is_absolute() {
        link_target
    } else {
        canonical_parent.join(&link_target)
    };

    let canonical_target = if let Ok(canonical) = fs::canonicalize(&resolved_target) {
        canonical
    } else if let Some(parent) = resolved_target.parent() {
        match fs::canonicalize(parent) {
            Ok(canonical_parent) => canonical_parent.join(resolved_target.file_name().unwrap_or_default()),
            Err(_) => {
                return Err(MountError::PathEscape {
                    virtual_path: normalized_virtual_path.to_owned(),
                });
            }
        }
    } else {
        return Err(MountError::PathEscape {
            virtual_path: normalized_virtual_path.to_owned(),
        });
    };

    check_boundary(&canonical_target, mount_host_path, normalized_virtual_path)
}

/// Returns whether `path` is already an absolute normalized sandbox path.
fn is_already_normalized_absolute_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    if !path.starts_with('/') || path.ends_with('/') {
        return false;
    }

    for part in path[1..].split('/') {
        if part.is_empty() || matches!(part, "." | "..") {
            return false;
        }
    }

    true
}

/// Checks whether a symlink's target escapes the mount boundary.
///
/// If `host_path` is a symlink, this resolves its target (relative to the
/// symlink's parent directory) and verifies the result stays within
/// `mount_host_path`. Returns `Err(PathEscape)` if the target escapes.
///
/// This prevents attacks where a symlink pointing outside the mount is
/// renamed within the overlay, creating a `RealFileRef` that later bypasses
/// boundary checks when the path is read.
pub(super) fn reject_escaping_symlink(
    host_path: &Path,
    mount_host_path: &Path,
    virtual_path: &str,
) -> Result<(), MountError> {
    let target = fs::read_link(host_path).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;

    // Resolve relative targets against the symlink's parent directory.
    let resolved = if target.is_relative() {
        let parent = host_path.parent().ok_or_else(|| MountError::PathEscape {
            virtual_path: virtual_path.to_owned(),
        })?;
        parent.join(&target)
    } else {
        target
    };

    // Canonicalize the resolved target and check the boundary.
    let canonical = fs::canonicalize(&resolved).map_err(|_| MountError::PathEscape {
        virtual_path: virtual_path.to_owned(),
    })?;
    let canonical_mount = fs::canonicalize(mount_host_path).map_err(|e| MountError::Io(e, virtual_path.to_owned()))?;

    check_boundary(&canonical, &canonical_mount, virtual_path)
}

/// Ensures a canonical host path stays within the canonical mount boundary.
fn check_boundary(canonical_path: &Path, mount_host_path: &Path, virtual_path: &str) -> Result<(), MountError> {
    if canonical_path.starts_with(mount_host_path) {
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
        assert_eq!(strip_mount_prefix("/anything", "/"), Some("anything"));
        assert_eq!(strip_mount_prefix("/", "/"), Some(""));
        assert_eq!(strip_mount_prefix("/data2/file", "/data"), None);
    }
}
