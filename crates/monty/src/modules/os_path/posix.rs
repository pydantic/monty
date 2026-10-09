//! The `posixpath` algorithms, on bytes.
//!
//! Each function mirrors the CPython 3.14 `posixpath` / `genericpath`
//! function of the same name. They work on `&[u8]` because `os.path` accepts
//! `str` and `bytes` alike and only ever splits at ASCII separators (`/`,
//! `.`), so one implementation serves both and slicing cannot break UTF-8.
//! Nothing here touches the VM: the module layer owns argument extraction,
//! allocation and host calls.

/// `os.path.isabs`: a leading `/`.
pub(super) fn isabs(path: &[u8]) -> bool {
    path.starts_with(b"/")
}

/// `os.path.split`: `(head, tail)` around the last `/`, with trailing
/// slashes stripped from `head` unless it is all slashes.
pub(super) fn split(path: &[u8]) -> (&[u8], &[u8]) {
    let (head, tail) = path.split_at(tail_start(path));
    (strip_trailing_slashes(head), tail)
}

/// `os.path.basename`: everything after the last `/`.
pub(super) fn basename(path: &[u8]) -> &[u8] {
    &path[tail_start(path)..]
}

/// `os.path.dirname`: everything before the last `/`, trailing slashes
/// stripped unless the head is the root.
pub(super) fn dirname(path: &[u8]) -> &[u8] {
    strip_trailing_slashes(&path[..tail_start(path)])
}

/// Index just past the last `/`, or 0 when there is none.
fn tail_start(path: &[u8]) -> usize {
    path.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1)
}

/// `head.rstrip('/')` unless `head` is empty or entirely slashes.
fn strip_trailing_slashes(head: &[u8]) -> &[u8] {
    if head.iter().all(|&c| c == b'/') {
        head
    } else {
        let end = head.iter().rposition(|&c| c != b'/').map_or(0, |i| i + 1);
        &head[..end]
    }
}

/// `os.path.splitext`: `(root, ext)` where `ext` starts at the last `.` of
/// the final component, unless that component is nothing but leading dots.
pub(super) fn splitext(path: &[u8]) -> (&[u8], &[u8]) {
    let sep_index = path.iter().rposition(|&c| c == b'/');
    let dot_index = path.iter().rposition(|&c| c == b'.');
    if let Some(dot) = dot_index
        && sep_index.is_none_or(|sep| dot > sep)
    {
        // Leading dots name a hidden file rather than an extension.
        let filename_start = sep_index.map_or(0, |sep| sep + 1);
        if path[filename_start..dot].iter().any(|&c| c != b'.') {
            return path.split_at(dot);
        }
    }
    (path, &path[..0])
}

/// `os.path.splitroot` without the always-empty drive: `(root, tail)`, where
/// the root is `/`, exactly `//` (implementation-defined per POSIX), or empty.
pub(super) fn splitroot(path: &[u8]) -> (&[u8], &[u8]) {
    if !path.starts_with(b"/") {
        (&path[..0], path)
    } else if path.get(1) != Some(&b'/') || path.get(2) == Some(&b'/') {
        path.split_at(1)
    } else {
        path.split_at(2)
    }
}

/// `os.path.normpath`: collapse `//`, `.` and `..` lexically. A leading `..`
/// survives in relative paths and is dropped under the root.
pub(super) fn normpath(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let (root, rest) = splitroot(path);
    let mut components: Vec<&[u8]> = Vec::new();
    for component in rest.split(|&c| c == b'/') {
        match component {
            b"" | b"." => {}
            b".." if !root.is_empty() || components.last().is_some_and(|last| *last != b"..") => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    let mut out = root.to_vec();
    out.extend(components.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `os.path.join`: an absolute part discards everything before it; a `/` is
/// inserted between parts unless the accumulated path is empty or already
/// ends in one.
pub(super) fn join<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut path = Vec::new();
    for part in parts {
        if part.starts_with(b"/") || path.is_empty() {
            path.clear();
        } else if !path.ends_with(b"/") {
            path.push(b'/');
        }
        path.extend_from_slice(part);
    }
    path
}

/// `os.path.abspath` against a known working directory: join then normalize.
pub(super) fn abspath(cwd: &[u8], path: &[u8]) -> Vec<u8> {
    if isabs(path) {
        normpath(path)
    } else {
        normpath(&join([cwd, path]))
    }
}

/// `os.path.relpath`: `path` relative to `start`, both made absolute against
/// `cwd` first. `..` climbs out of the components `start` does not share.
pub(super) fn relpath(cwd: &[u8], path: &[u8], start: &[u8]) -> Vec<u8> {
    let start_list = absolute_components(cwd, start);
    let path_list = absolute_components(cwd, path);
    let shared = start_list.iter().zip(&path_list).take_while(|(a, b)| a == b).count();
    let climbs = start_list.len() - shared;
    let parts: Vec<&[u8]> = (0..climbs)
        .map(|_| &b".."[..])
        .chain(path_list[shared..].iter().map(Vec::as_slice))
        .collect();
    if parts.is_empty() {
        b".".to_vec()
    } else {
        parts.join(&b'/')
    }
}

/// The components of `abspath(path)` with the leading slashes removed.
fn absolute_components(cwd: &[u8], path: &[u8]) -> Vec<Vec<u8>> {
    let absolute = abspath(cwd, path);
    let tail = &absolute[absolute.iter().position(|&c| c != b'/').unwrap_or(absolute.len())..];
    if tail.is_empty() {
        Vec::new()
    } else {
        tail.split(|&c| c == b'/').map(<[u8]>::to_vec).collect()
    }
}

/// `os.path.commonpath` over non-empty `paths` of one kind: the longest
/// shared component prefix, comparing the lexicographically smallest and
/// largest component lists. Errors when absolute and relative paths mix.
pub(super) fn commonpath(paths: &[&[u8]]) -> Result<Vec<u8>, &'static str> {
    let absolute = isabs(paths[0]);
    if paths.iter().any(|path| isabs(path) != absolute) {
        return Err("Can't mix absolute and relative paths");
    }
    let split_paths: Vec<Vec<&[u8]>> = paths
        .iter()
        .map(|path| {
            path.split(|&c| c == b'/')
                .filter(|component| !component.is_empty() && *component != b".")
                .collect()
        })
        .collect();
    let smallest = split_paths.iter().min().expect("caller rejects an empty sequence");
    let largest = split_paths.iter().max().expect("caller rejects an empty sequence");
    let shared = smallest.iter().zip(largest).take_while(|(a, b)| a == b).count();
    let mut out = if absolute { b"/".to_vec() } else { Vec::new() };
    out.extend(smallest[..shared].join(&b'/'));
    Ok(out)
}

/// Where `~`-expansion's user part ends: `Some(i)` for a path starting with
/// `~`, `i` being the index of the first `/` (or the length). `i == 1` is the
/// bare `~` form `expanduser` serves from `$HOME`.
pub(super) fn tilde_end(path: &[u8]) -> Option<usize> {
    path.starts_with(b"~")
        .then(|| path[1..].iter().position(|&c| c == b'/').map_or(path.len(), |i| i + 1))
}

/// `os.path.expanduser` once `$HOME` is known: `home` with trailing slashes
/// stripped, then the path after the `~`, or `/` when both are empty.
pub(crate) fn expand_home(home: &[u8], tail: &[u8]) -> Vec<u8> {
    let end = home.iter().rposition(|&c| c != b'/').map_or(0, |i| i + 1);
    let mut out = home[..end].to_vec();
    out.extend_from_slice(tail);
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

/// `os.path.expandvars`: replace `$name` and `${name}` with `lookup(name)`,
/// leaving unknown names and malformed references as written. `name` is an
/// ASCII `\w+` run for the bare form and anything up to the first `}` for
/// the braced form, which stays unexpanded when that `}` never comes.
pub(crate) fn expandvars(path: &[u8], mut lookup: impl FnMut(&[u8]) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(path.len());
    let mut rest = path;
    while let Some(dollar) = rest.iter().position(|&c| c == b'$') {
        out.extend_from_slice(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        let (reference, name) = if after.starts_with(b"{") {
            let Some(close) = after.iter().position(|&c| c == b'}') else {
                // `${` with no closing brace: the rest of the path is one unexpandable match.
                out.extend_from_slice(&rest[dollar..]);
                return out;
            };
            (&rest[dollar..dollar + close + 2], &after[1..close])
        } else {
            let len = after
                .iter()
                .position(|&c| !(c.is_ascii_alphanumeric() || c == b'_'))
                .unwrap_or(after.len());
            (&rest[dollar..=dollar + len], &after[..len])
        };
        if name.is_empty() && !reference.starts_with(b"${") {
            // A lone `$` matches nothing and is copied through.
            out.push(b'$');
            rest = after;
            continue;
        }
        match lookup(name) {
            Some(value) => out.extend_from_slice(&value),
            None => out.extend_from_slice(reference),
        }
        rest = &rest[dollar + reference.len()..];
    }
    out.extend_from_slice(rest);
    out
}
