//! Lexical POSIX normalization shared by the interpreter and filesystem hosts.

use std::borrow::Cow;

/// Returns an absolute POSIX path with `.` removed and `..` resolved, stopping at `/`.
/// Relative inputs are rooted at `/`; this never reads the host cwd or follows symlinks.
/// Already normalized paths are borrowed. Validate NUL bytes and length limits first:
/// normalization can remove invalid components and does not provide filesystem confinement.
#[must_use]
pub fn normalize_virtual_path(path: &str) -> Cow<'_, str> {
    if is_normalized(path) {
        Cow::Borrowed(path)
    } else {
        let mut out = String::with_capacity(path.len());
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => out.truncate(out.rfind('/').unwrap_or(0)),
                segment => {
                    out.push('/');
                    out.push_str(segment);
                }
            }
        }
        if out.is_empty() {
            out.push('/');
        }
        Cow::Owned(out)
    }
}

/// Recognizes paths that can be returned without allocating or rewriting them.
fn is_normalized(path: &str) -> bool {
    path == "/"
        || (path.starts_with('/')
            && !path.ends_with('/')
            && path
                .split('/')
                .skip(1)
                .all(|segment| !matches!(segment, "" | "." | "..")))
}
