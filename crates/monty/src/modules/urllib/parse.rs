//! The algorithms behind `urllib.parse`, ported from CPython's `Lib/urllib/parse.py`
//! and working on plain Rust strings; `mod.rs` binds them to Python values.
//!
//! Every delimiter is ASCII, so byte offsets never split a character.

use std::{
    fmt::Write,
    iter,
    net::{Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

use unicode_normalization::UnicodeNormalization;

use crate::{
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    types::str::StringRepr,
};

/// Schemes whose relative references `urljoin` resolves against the base.
const USES_RELATIVE: &[&str] = &[
    "", "ftp", "http", "gopher", "nntp", "imap", "wais", "file", "https", "shttp", "mms", "prospero", "rtsp", "rtsps",
    "rtspu", "sftp", "svn", "svn+ssh", "ws", "wss",
];

/// Schemes that take a `//netloc` part.
const USES_NETLOC: &[&str] = &[
    "",
    "ftp",
    "http",
    "gopher",
    "nntp",
    "telnet",
    "imap",
    "wais",
    "file",
    "mms",
    "https",
    "shttp",
    "snews",
    "prospero",
    "rtsp",
    "rtsps",
    "rtspu",
    "rsync",
    "svn",
    "svn+ssh",
    "sftp",
    "nfs",
    "git",
    "git+ssh",
    "ws",
    "wss",
    "itms-services",
];

/// Schemes whose path may carry `;params`, split off by `urlparse` alone.
const USES_PARAMS: &[&str] = &[
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps", "rtspu", "sip", "sips", "mms",
    "sftp", "tel",
];

/// The five parts `urlsplit` produces; `None` marks a part whose delimiter was
/// absent, which `urlunsplit` and `urljoin` tell apart from an empty one.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Split {
    pub scheme: Option<String>,
    pub netloc: Option<String>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

/// CPython's `_urlsplit`: WHATWG control stripping, scheme detection, netloc
/// validation (bracketed IPv6 and NFKC), then fragment and query splitting.
pub(super) fn urlsplit(url: &str, scheme: Option<&str>, allow_fragments: bool) -> RunResult<Split> {
    let mut url = remove_unsafe_bytes(url.trim_start_matches(is_c0_control_or_space));
    let mut scheme = scheme.map(|s| remove_unsafe_bytes(s.trim_matches(is_c0_control_or_space)));

    if let Some(i) = url.find(':')
        && i > 0
        && url.as_bytes()[0].is_ascii_alphabetic()
        && url[..i].bytes().all(is_scheme_char)
    {
        scheme = Some(url[..i].to_ascii_lowercase());
        url = url[i + 1..].to_owned();
    }

    let mut netloc = None;
    if url.starts_with("//") {
        let (host, rest) = split_netloc(&url, 2);
        let has_open = host.contains('[');
        let has_close = host.contains(']');
        if has_open != has_close {
            return Err(invalid_ipv6_url());
        }
        if has_open {
            check_bracketed_netloc(host)?;
        }
        netloc = Some(host.to_owned());
        url = rest.to_owned();
    }

    let mut fragment = None;
    if allow_fragments && let Some((before, after)) = url.split_once('#') {
        fragment = Some(after.to_owned());
        url = before.to_owned();
    }
    let mut query = None;
    if let Some((before, after)) = url.split_once('?') {
        query = Some(after.to_owned());
        url = before.to_owned();
    }
    if let Some(netloc) = &netloc {
        check_netloc(netloc)?;
    }
    Ok(Split {
        scheme,
        netloc,
        path: url,
        query,
        fragment,
    })
}

/// `urlparse`'s extra step: for schemes that use them, `;params` are split off
/// the last path segment. `None` when the path has no `;`.
pub(super) fn split_params(scheme: &str, path: &str) -> Option<(String, String)> {
    if !USES_PARAMS.contains(&scheme) || !path.contains(';') {
        return None;
    }
    let i = match path.rfind('/') {
        Some(slash) => path[slash..].find(';').map(|i| slash + i)?,
        None => path.find(';')?,
    };
    Some((path[..i].to_owned(), path[i + 1..].to_owned()))
}

/// CPython's `_urlunsplit`, on parts where `None` means "no delimiter".
pub(super) fn urlunsplit(
    scheme: Option<&str>,
    netloc: Option<&str>,
    path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> String {
    let mut url = String::new();
    if let Some(netloc) = netloc {
        url.push_str("//");
        url.push_str(netloc);
        if !path.is_empty() && !path.starts_with('/') {
            url.push('/');
        }
        url.push_str(path);
    } else if path.starts_with("//") {
        url.push_str("//");
        url.push_str(path);
    } else {
        url.push_str(path);
    }
    if let Some(scheme) = scheme.filter(|s| !s.is_empty()) {
        url.insert(0, ':');
        url.insert_str(0, scheme);
    }
    if let Some(query) = query {
        url.push('?');
        url.push_str(query);
    }
    if let Some(fragment) = fragment {
        url.push('#');
        url.push_str(fragment);
    }
    url
}

/// The netloc `urlunsplit`/`urlunparse` reassemble from a component tuple: an
/// empty netloc still gets its `//` for a scheme that uses one when the path is
/// empty or absolute, and is omitted otherwise.
pub(super) fn netloc_for_unsplit<'a>(scheme: &str, netloc: &'a str, path: &str) -> Option<&'a str> {
    if !netloc.is_empty() {
        Some(netloc)
    } else if !scheme.is_empty() && USES_NETLOC.contains(&scheme) && (path.is_empty() || path.starts_with('/')) {
        Some("")
    } else {
        None
    }
}

/// `urljoin(base, url)` on two non-empty strings: RFC 3986 reference resolution
/// with CPython's scheme tables and its handling of `.` and `..` segments.
pub(super) fn urljoin(base: &str, url: &str, allow_fragments: bool) -> RunResult<String> {
    let b = urlsplit(base, None, allow_fragments)?;
    let mut r = urlsplit(url, None, allow_fragments)?;

    let scheme = r.scheme.clone().or_else(|| b.scheme.clone());
    let scheme_str = scheme.as_deref().unwrap_or("");
    if scheme != b.scheme || (!scheme_str.is_empty() && !USES_RELATIVE.contains(&scheme_str)) {
        return Ok(url.to_owned());
    }
    if scheme_str.is_empty() || USES_NETLOC.contains(&scheme_str) {
        if r.netloc.as_deref().is_some_and(|n| !n.is_empty()) {
            return Ok(urlunsplit(
                scheme.as_deref(),
                r.netloc.as_deref(),
                &r.path,
                r.query.as_deref(),
                r.fragment.as_deref(),
            ));
        }
        r.netloc.clone_from(&b.netloc);
    }

    if r.path.is_empty() {
        // The base's query is used only when the reference has none, and the
        // base's fragment only when the query was taken from the base as well.
        let (query, fragment) = match r.query {
            None => (b.query.clone(), r.fragment.or_else(|| b.fragment.clone())),
            Some(query) => (Some(query), r.fragment),
        };
        return Ok(urlunsplit(
            scheme.as_deref(),
            r.netloc.as_deref(),
            &b.path,
            query.as_deref(),
            fragment.as_deref(),
        ));
    }

    let mut base_parts: Vec<&str> = b.path.split('/').collect();
    if base_parts.last() != Some(&"") {
        base_parts.pop();
    }
    let segments: Vec<&str> = if r.path.starts_with('/') {
        r.path.split('/').collect()
    } else {
        let mut segments = base_parts;
        segments.extend(r.path.split('/'));
        // Drop empty interior segments, which would join as redundant slashes;
        // a lone segment (`urljoin('a', 'b')`) has no interior.
        if segments.len() > 1 {
            let last = segments.len() - 1;
            let mut filtered = vec![segments[0]];
            filtered.extend(segments[1..last].iter().copied().filter(|s| !s.is_empty()));
            filtered.push(segments[last]);
            filtered
        } else {
            segments
        }
    };

    let mut resolved: Vec<&str> = Vec::new();
    for seg in &segments {
        match *seg {
            ".." => {
                resolved.pop();
            }
            "." => {}
            seg => resolved.push(seg),
        }
    }
    if matches!(segments.last(), Some(&"." | &"..")) {
        resolved.push("");
    }
    let mut path = resolved.join("/");
    if path.is_empty() {
        path.push('/');
    }
    Ok(urlunsplit(
        scheme.as_deref(),
        r.netloc.as_deref(),
        &path,
        r.query.as_deref(),
        r.fragment.as_deref(),
    ))
}

/// The userinfo of a netloc: `(username, password)`, as the result objects report them.
pub(super) fn userinfo(netloc: &str) -> (Option<&str>, Option<&str>) {
    match netloc.rsplit_once('@') {
        Some((userinfo, _)) => match userinfo.split_once(':') {
            Some((user, password)) => (Some(user), Some(password)),
            None => (Some(userinfo), None),
        },
        None => (None, None),
    }
}

/// The host part of a netloc: `(hostname, port)` before lowercasing or
/// integer conversion, as CPython's `_hostinfo` splits them.
pub(super) fn hostinfo(netloc: &str) -> (&str, Option<&str>) {
    let hostinfo = netloc.rsplit_once('@').map_or(netloc, |(_, host)| host);
    let (hostname, port) = match hostinfo.split_once('[') {
        Some((_, bracketed)) => match bracketed.split_once(']') {
            Some((hostname, port)) => (hostname, port.split_once(':').map(|(_, port)| port)),
            None => (bracketed, None),
        },
        None => match hostinfo.split_once(':') {
            Some((hostname, port)) => (hostname, Some(port)),
            None => (hostinfo, None),
        },
    };
    (hostname, port.filter(|port| !port.is_empty()))
}

/// `.hostname`: lowercased, except an IPv6 scope id after `%`.
pub(super) fn hostname(netloc: &str) -> Option<String> {
    let (hostname, _) = hostinfo(netloc);
    if hostname.is_empty() {
        return None;
    }
    Some(match hostname.split_once('%') {
        Some((host, zone)) => format!("{}%{zone}", host.to_lowercase()),
        None => hostname.to_lowercase(),
    })
}

/// `.port`: the port as an integer in `0..=65535`, with CPython's errors.
pub(super) fn port(netloc: &str) -> RunResult<Option<i64>> {
    let Some(port) = hostinfo(netloc).1 else {
        return Ok(None);
    };
    if !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ExcType::value_error(format!(
            "Port could not be cast to integer value as {}",
            StringRepr(port)
        )));
    }
    match port.parse::<i64>() {
        Ok(number) if (0..=65535).contains(&number) => Ok(Some(number)),
        _ => Err(ExcType::value_error("Port out of range 0-65535")),
    }
}

/// `quote_from_bytes`: percent-encodes every byte outside the always-safe set and
/// `safe`, with upper-case hex digits. `safe` holds ASCII bytes only.
pub(super) fn quote_bytes(bytes: &[u8], safe: &[u8]) -> String {
    // A lookup table keeps a long `safe` from being rescanned for every byte.
    let mut keep = [false; 128];
    for &b in safe {
        if let Some(slot) = keep.get_mut(usize::from(b)) {
            *slot = true;
        }
    }
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'_' | b'.' | b'-' | b'~')
            || keep.get(usize::from(b)) == Some(&true)
        {
            out.push(char::from(b));
        } else {
            // Writing to a `String` cannot fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// `unquote_to_bytes`: replaces each `%XX` with its byte, leaving malformed
/// escapes verbatim.
pub(super) fn unquote_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(&hi), Some(&lo)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(hi), Some(lo)) = (hex_digit(hi), hex_digit(lo))
        {
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// The byte runs `unquote` decodes separately: CPython decodes each maximal ASCII
/// run on its own and passes non-ASCII text through untouched, so a decoding
/// error handler never sees the characters around an escape.
pub(super) fn ascii_runs(s: &str) -> impl Iterator<Item = (bool, &str)> {
    let bytes = s.as_bytes();
    let mut start = 0;
    iter::from_fn(move || {
        if start >= bytes.len() {
            return None;
        }
        let ascii = bytes[start].is_ascii();
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii() == ascii {
            end += 1;
        }
        let run = &s[start..end];
        start = end;
        Some((ascii, run))
    })
}

/// Validates a netloc holding `[`: nothing before the bracket, only a port
/// after it, and an IPv6 or IPvFuture literal inside.
fn check_bracketed_netloc(netloc: &str) -> RunResult<()> {
    let host_and_port = netloc.rsplit_once('@').map_or(netloc, |(_, host)| host);
    let hostname = match host_and_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err(invalid_ipv6_url());
            }
            let (hostname, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return Err(invalid_ipv6_url());
            }
            hostname
        }
        None => host_and_port.split_once(':').map_or(host_and_port, |(host, _)| host),
    };
    check_bracketed_host(hostname)
}

/// A bracketed host is an IPvFuture literal (`v<hex>.<rest>`) or an IPv6 address
/// with an optional scope id; an IPv4 address is rejected, as CPython rejects it.
fn check_bracketed_host(hostname: &str) -> RunResult<()> {
    if hostname.starts_with(['v', 'V']) {
        let rest = &hostname[1..];
        let valid = rest.split_once('.').is_some_and(|(hex, tail)| {
            !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) && !tail.is_empty()
        });
        return if valid {
            Ok(())
        } else {
            Err(ExcType::value_error("IPvFuture address is invalid"))
        };
    }
    if hostname.parse::<Ipv4Addr>().is_ok() {
        return Err(ExcType::value_error("An IPv4 address cannot be in brackets"));
    }
    let address = match hostname.split_once('%') {
        Some((address, scope)) if !scope.is_empty() && !scope.contains('%') => address,
        Some(_) => "",
        None => hostname,
    };
    if Ipv6Addr::from_str(address).is_ok() {
        Ok(())
    } else {
        Err(ExcType::value_error(format!(
            "'{hostname}' does not appear to be an IPv4 or IPv6 address"
        )))
    }
}

/// Rejects a netloc whose NFKC normalization introduces a delimiter, so
/// look-alike characters cannot smuggle one past the split.
fn check_netloc(netloc: &str) -> RunResult<()> {
    if netloc.is_empty() || netloc.is_ascii() {
        return Ok(());
    }
    let stripped: String = netloc.chars().filter(|c| !matches!(c, '@' | ':' | '#' | '?')).collect();
    let normalized: String = stripped.nfkc().collect();
    if stripped == normalized || !normalized.contains(['/', '?', '#', '@', ':']) {
        Ok(())
    } else {
        Err(ExcType::value_error(format!(
            "netloc '{netloc}' contains invalid characters under NFKC normalization"
        )))
    }
}

/// Splits `//netloc/rest` at the first of `/`, `?` or `#` after `start`.
fn split_netloc(url: &str, start: usize) -> (&str, &str) {
    let rest = &url[start..];
    let delim = rest.find(['/', '?', '#']).map_or(url.len(), |i| start + i);
    (&url[start..delim], &url[delim..])
}

/// WHATWG's unsafe bytes, removed wherever they appear.
fn remove_unsafe_bytes(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect()
}

/// WHATWG's C0 controls and space, stripped from the ends.
fn is_c0_control_or_space(c: char) -> bool {
    c <= ' '
}

/// The characters a scheme may contain after its leading letter.
fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
}

fn hex_digit(b: u8) -> Option<u8> {
    char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

fn invalid_ipv6_url() -> RunError {
    ExcType::value_error("Invalid IPv6 URL")
}
