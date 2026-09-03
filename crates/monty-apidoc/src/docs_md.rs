//! Converts rustdoc doc comments into page-ready markdown.
//!
//! Doc comments assume rustdoc's renderer: `# Errors` headings that become
//! sections, ```` ``` ```` fences with hidden `# ` lines, and intra-doc links
//! in all three markdown spellings — `` [`Pool::checkout`] `` shorthand,
//! inline `` [`X`](crate::x::X) `` and reference definitions
//! `` [`X`]: crate::x::X ``. This module rewrites all of that for
//! mkdocs/Starlight: headings are demoted below the item's own heading,
//! hidden lines are stripped, resolved links become real markdown links and
//! unresolved ones degrade to plain code spans (never broken links —
//! `mkdocs build --strict` fails on those).

use std::{borrow::Cow, cmp::Reverse, collections::HashMap};

use rustdoc_types::{Crate, Id};

use crate::symbols::SymbolMap;

/// Fence info tokens that mean "this is Rust" to rustdoc; all map to `rust`.
const RUST_FENCE_TOKENS: &[&str] = &[
    "",
    "rust",
    "ignore",
    "no_run",
    "should_panic",
    "compile_fail",
    "edition2015",
    "edition2018",
    "edition2021",
    "edition2024",
];

/// Path prefixes that only mean something inside the defining module, so
/// they are dropped from link text and code spans on the page.
const RELATIVE_PREFIXES: &[&str] = &["crate::", "super::", "self::"];

/// Renders one item's doc comment for a page position where the item's own
/// heading is at `heading_level` (doc headings are shifted below it).
pub fn process_docs(
    docs: &str,
    heading_level: usize,
    links: &HashMap<String, Id>,
    from_crate: &str,
    krate: &Crate,
    symbols: &SymbolMap,
) -> String {
    let resolver = Resolver::new(docs, links, from_crate, krate, symbols);
    let mut out = Vec::new();
    // the open fence's marker and language: a block closes only on a fence
    // at least as long, so ```` blocks can embed ``` fences
    let mut in_fence: Option<(String, String)> = None;
    for line in docs.lines() {
        if let Some((marker, fence_lang)) = &in_fence {
            if is_closing_fence(line, marker) {
                out.push(marker.clone());
                in_fence = None;
            } else if fence_lang == "rust" && (line == "#" || line.starts_with("# ")) {
                // rustdoc hidden line
            } else if fence_lang == "rust" && line.starts_with("##") {
                out.push(line.replacen("##", "#", 1));
            } else {
                out.push(line.to_owned());
            }
        } else if let Some((marker, info)) = split_fence(line) {
            let lang = normalize_fence_info(info);
            out.push(format!("{marker}{lang}"));
            in_fence = Some((marker.to_owned(), lang));
        } else if is_rust_path_definition(line) {
            // dropped: its label occurrences are resolved via the links map
        } else {
            out.push(resolver.prose_line(line, heading_level));
        }
    }
    out.join("\n")
}

/// Splits an opening code fence into its marker (three or more backticks or
/// tildes) and info string; `None` for any other line.
fn split_fence(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let first = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = trimmed.chars().take_while(|c| *c == first).count();
    (len >= 3).then(|| trimmed.split_at(len))
}

/// Whether `line` closes a fence opened with `marker`: the same character,
/// at least as many of them, and nothing else.
fn is_closing_fence(line: &str, marker: &str) -> bool {
    let trimmed = line.trim();
    let first = marker.chars().next();
    trimmed.len() >= marker.len() && trimmed.chars().all(|c| Some(c) == first)
}

/// Maps a fence info string to the language mkdocs/Starlight should
/// highlight: rustdoc attribute combos (`no_run`, `rust,ignore`, ...) all
/// mean Rust; anything else is kept as-is.
fn normalize_fence_info(info: &str) -> String {
    let tokens: Vec<&str> = info.split(',').map(str::trim).collect();
    if tokens.iter().all(|t| RUST_FENCE_TOKENS.contains(t)) {
        "rust".to_owned()
    } else {
        tokens[0].to_owned()
    }
}

/// Link rewriting for one doc comment: the rustdoc-resolved `links` map plus
/// the doc's own `[label]: rust::path` reference definitions.
struct Resolver<'a> {
    links: &'a HashMap<String, Id>,
    /// Reference-definition labels mapped to their rust-path target key.
    definitions: HashMap<String, String>,
    from_crate: &'a str,
    krate: &'a Crate,
    symbols: &'a SymbolMap,
}

impl<'a> Resolver<'a> {
    fn new(
        docs: &str,
        links: &'a HashMap<String, Id>,
        from_crate: &'a str,
        krate: &'a Crate,
        symbols: &'a SymbolMap,
    ) -> Self {
        // collect `[label]: rust::path` definitions up front so label
        // occurrences anywhere in the doc can resolve through them
        let mut definitions = HashMap::new();
        for line in docs.lines() {
            if let Some((label, target)) = split_definition(line)
                && is_rust_path(target)
            {
                definitions.insert(label.to_owned(), target.to_owned());
            }
        }
        Self {
            links,
            definitions,
            from_crate,
            krate,
            symbols,
        }
    }

    /// One non-fence line: demote headings, then rewrite intra-doc links.
    fn prose_line(&self, line: &str, heading_level: usize) -> String {
        let line = demote_heading(line, heading_level);
        let line = self.rewrite_shorthand(&line);
        let line = self.rewrite_inline_rust_links(&line);
        strip_unresolved_shorthand(&line)
    }

    /// Rewrites `[key]` / `[key][]` occurrences of every resolvable key —
    /// from the links map directly and via reference definitions.
    fn rewrite_shorthand(&self, line: &str) -> String {
        let mut line = line.to_owned();
        // longest first so `[`Pool::checkout`]` wins over a hypothetical `[`Pool`]`
        let mut keys: Vec<(&String, &String)> = self
            .links
            .keys()
            .map(|k| (k, k))
            .chain(self.definitions.iter())
            .collect();
        keys.sort_by_key(|(label, _)| Reverse(label.len()));
        for (label, key) in keys {
            let bracketed = format!("[{label}]");
            if !line.contains(&bracketed) {
                continue;
            }
            let replacement = match self.resolve_key(key) {
                Some(url) => format!("[{}]({url})", display_text(label)),
                // unresolvable: leave the label text (usually a code span) unlinked
                None => display_text(label).into_owned(),
            };
            line = replace_shorthand(&line, &bracketed, &replacement);
        }
        line
    }

    /// Rewrites inline links whose target is a Rust path,
    /// `` [`X`](crate::x::X) `` — resolved to a page URL or reduced to text.
    fn rewrite_inline_rust_links(&self, line: &str) -> String {
        rewrite_inline_links(line, |text, target| {
            is_rust_path(target).then(|| match self.resolve_key(target) {
                Some(url) => format!("[{}]({url})", display_text(text)),
                None => display_text(text).into_owned(),
            })
        })
    }

    /// A links-map key to a page-relative URL, when the target is rendered.
    fn resolve_key(&self, key: &str) -> Option<String> {
        let id = self.links.get(key)?;
        self.symbols.resolve(self.from_crate, self.krate, *id)
    }
}

/// Whether `line` is a reference definition targeting a Rust path (to be
/// dropped — markdown would render its label occurrences as dead links).
pub fn is_rust_path_definition(line: &str) -> bool {
    split_definition(line).is_some_and(|(_, target)| is_rust_path(target))
}

/// Reduces intra-doc link syntax to plain code spans, for doc lines placed
/// inside a declaration fence where markdown links would render literally.
pub fn strip_intra_doc_links(line: &str) -> String {
    let line = rewrite_inline_links(line, |text, target| {
        is_rust_path(target).then(|| display_text(text).into_owned())
    });
    // module-relative prefixes mean nothing on the page, in code spans included
    let mut line = strip_unresolved_shorthand(&line.replace("`][]", "`]"));
    for prefix in RELATIVE_PREFIXES {
        line = line.replace(&format!("`{prefix}"), "`");
    }
    line
}

/// Scans `line` for inline `[text](target)` links, replacing each with
/// `rewrite(text, target)` where that returns `Some`; other links pass
/// through unchanged. The closing `)` is found by paren balancing so link
/// text and targets containing `()` (`` [`run()`](Self::run()) ``) parse.
fn rewrite_inline_links(line: &str, mut rewrite: impl FnMut(&str, &str) -> Option<String>) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(mid) = rest.find("](") {
        // the link text starts at the last `[` before `](`
        let (Some(open), Some(end)) = (rest[..mid].rfind('['), balanced_close(rest, mid + 2)) else {
            out.push_str(&rest[..mid + 2]);
            rest = &rest[mid + 2..];
            continue;
        };
        let text = &rest[open + 1..mid];
        let target = &rest[mid + 2..end];
        out.push_str(&rest[..open]);
        match rewrite(text, target) {
            Some(replacement) => out.push_str(&replacement),
            None => out.push_str(&rest[open..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Byte index of the `)` closing a link target that starts at `start`,
/// balancing nested parentheses (targets like `Self::run()`).
fn balanced_close(s: &str, start: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (i, b) in s.bytes().enumerate().skip(start) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits a `[label]: target` reference-definition line, ignoring an
/// optional markdown title (`[label]: target "Title"`).
fn split_definition(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find("]:")?;
    let mut tokens = rest[close + 2..].trim().splitn(2, char::is_whitespace);
    let target = tokens.next().filter(|target| !target.is_empty())?;
    let title = tokens.next().map(str::trim_start).unwrap_or_default();
    let is_title = title.is_empty() || title.starts_with(['"', '\'', '(']);
    is_title.then_some((&rest[..close], target))
}

/// Whether a link target is a Rust item path rather than a real URL or a
/// relative file link.
fn is_rust_path(target: &str) -> bool {
    target.contains("::")
        || (!target.contains("://")
            && !target.contains('/')
            && !target.contains('.')
            && !target.contains('#')
            && target.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '`'))
}

/// Link text shown to the reader: `crate::`, `super::` and `self::`
/// prefixes mean nothing outside the defining module, so they are stripped
/// (inside backticks too).
fn display_text(text: &str) -> Cow<'_, str> {
    let (tick, body) = match text.strip_prefix('`') {
        Some(body) => ("`", body),
        None => ("", text),
    };
    let stripped = RELATIVE_PREFIXES.iter().find_map(|prefix| body.strip_prefix(prefix));
    match stripped {
        Some(rest) => Cow::Owned(format!("{tick}{rest}")),
        None => Cow::Borrowed(text),
    }
}

/// Shifts `# Errors`-style doc headings below the item's own heading so a
/// page keeps a single H1 and a sane outline (capped at H6).
fn demote_heading(line: &str, heading_level: usize) -> String {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if hashes == 0 || !line[hashes..].starts_with(' ') {
        line.to_owned()
    } else {
        format!(
            "{} {}",
            "#".repeat((hashes + heading_level).min(6)),
            &line[hashes + 1..]
        )
    }
}

/// Replaces `pattern[]` and bare `pattern` occurrences, leaving alone forms
/// that already carry their own target — `pattern(...)` and `pattern[label]`.
fn replace_shorthand(line: &str, pattern: &str, replacement: &str) -> String {
    let collapsed = line.replace(&format!("{pattern}[]"), pattern);
    let mut out = String::with_capacity(collapsed.len());
    let mut rest = collapsed.as_str();
    while let Some(pos) = rest.find(pattern) {
        let after = &rest[pos + pattern.len()..];
        out.push_str(&rest[..pos]);
        if after.starts_with('(') || after.starts_with('[') {
            out.push_str(pattern);
        } else {
            out.push_str(replacement);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Strips brackets from leftover `` [`X`] `` shorthand rustdoc did not
/// resolve, which plain markdown would otherwise render literally.
fn strip_unresolved_shorthand(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(pos) = rest.find("[`") {
        out.push_str(&rest[..pos]);
        let candidate = &rest[pos..];
        match candidate.find("`]") {
            // keep real links (`](...)` / `][...]`)
            Some(end)
                if !candidate[end + 2..].starts_with('(')
                    && !candidate[end + 2..].starts_with('[')
                    && !candidate[1..end].contains(']') =>
            {
                out.push_str(&candidate[1..=end]);
                rest = &candidate[end + 2..];
            }
            _ => {
                out.push_str("[`");
                rest = &candidate[2..];
            }
        }
    }
    out.push_str(rest);
    out
}
