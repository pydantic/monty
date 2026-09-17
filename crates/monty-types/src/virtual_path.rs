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

/// Checks a host-supplied working directory: absolute, POSIX, no NUL bytes.
///
/// Trailing slashes are dropped so `os.getcwd()` never reports `/data/`; the
/// root itself stays `/`. `.` and `..` are left for the interpreter, which
/// normalizes the directory when it adopts it. The error is the message of
/// the `ValueError` hosts raise for it.
///
/// ```
/// use monty_types::validate_cwd;
///
/// assert_eq!(validate_cwd("/data/").unwrap(), "/data");
/// assert_eq!(validate_cwd("data").unwrap_err(), "cwd must be an absolute POSIX path: \"data\"");
/// ```
pub fn validate_cwd(cwd: &str) -> Result<String, String> {
    if cwd.contains('\0') {
        Err(format!("cwd must not contain NUL bytes: {cwd:?}"))
    } else if !cwd.starts_with('/') {
        Err(format!("cwd must be an absolute POSIX path: {cwd:?}"))
    } else {
        let trimmed = cwd.trim_end_matches('/');
        Ok(if trimmed.is_empty() {
            "/".to_owned()
        } else {
            trimmed.to_owned()
        })
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
