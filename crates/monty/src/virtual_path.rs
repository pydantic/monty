//! POSIX paths for the sandbox's working directory and script metadata.
//! Filesystem requests use `posix_join` to preserve input for host validation;
//! normalization is reserved for stored state and metadata.

use std::sync::Arc;

use monty_types::normalize_virtual_path;

/// Prepends the absolute virtual `cwd` to a relative path without normalizing it.
/// `cwd` must be canonical: absolute, with no `.` or `..`, and no trailing slash except `/`.
/// Preserves all components so the host can validate NUL bytes and path limits
/// before collapsing `.` and `..`. Absolute and empty paths stay as written.
pub(crate) fn posix_join(cwd: &str, path: &str) -> String {
    if path.is_empty() || path.starts_with('/') {
        path.to_owned()
    } else {
        let mut joined = String::with_capacity(cwd.len() + 1 + path.len());
        joined.push_str(cwd);
        if !cwd.ends_with('/') {
            joined.push('/');
        }
        joined.push_str(path);
        joined
    }
}

/// Normalizes an executor's host-supplied cwd, sharing it with snippet executors.
/// Already canonical paths need only the shared-string allocation.
pub(crate) fn canonical_cwd(cwd: &str) -> Arc<str> {
    Arc::from(normalize_virtual_path(cwd))
}
