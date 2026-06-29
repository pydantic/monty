//! Extracting PEP 723 inline script-metadata dependencies from fed code.
//!
//! Before running a feed, the session scans the snippet for a PEP 723
//! `# /// script` … `# ///` metadata block and installs the `dependencies` it
//! declares (via the same `uv` path as an explicit `InstallDependencies`
//! request) so the snippet's imports resolve. This mirrors how `uv run`
//! provisions a script's inline dependencies, but stays entirely inside the
//! worker — the protocol carries no PEP 723 awareness.
//!
//! Extraction is pure Rust: [`BLOCK`] is the block matcher from the PEP 723
//! specification, and the metadata body is parsed with the `toml` crate.

use std::{error::Error, fmt, sync::LazyLock};

use regex::Regex;
use toml::{Table, Value, de::Error as TomlError};

/// The block matcher from the PEP 723 specification: a `# /// <type>` opener,
/// one or more `#`-prefixed content lines, then a closing `# ///`. Compiled
/// once. Note `# ///` itself matches a content line, so the greedy `content`
/// group merges two *adjacent* blocks into one (they must be separated by a
/// non-comment line to be seen as two) — this matches the reference regex.
static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^# /// (?P<type>[a-zA-Z0-9-]+)$\s(?P<content>(^#(| .*)$\s)+)^# ///$")
        .expect("PEP 723 block regex is valid")
});

/// Why PEP 723 metadata could not be turned into a dependency list. Surfaced to
/// the parent as a `ValueError`, matching what CPython tooling would raise.
#[derive(Debug)]
pub enum Pep723Error {
    /// More than one `# /// script` block; PEP 723 permits at most one.
    MultipleBlocks,
    /// `dependencies` is present but is not an array of strings.
    InvalidDependencies,
    /// The metadata block body is not valid TOML.
    Toml(TomlError),
}

impl fmt::Display for Pep723Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleBlocks => f.write_str("multiple PEP 723 script blocks found"),
            Self::InvalidDependencies => f.write_str("PEP 723 dependencies must be an array of strings"),
            Self::Toml(err) => write!(f, "invalid PEP 723 metadata: {err}"),
        }
    }
}

impl Error for Pep723Error {}

/// Extracts the PEP 723 `dependencies` declared in `source`.
///
/// Returns an empty vec when the snippet has no `script` metadata block (the
/// common, fast path). Errors on a duplicated block, a malformed TOML body, or
/// a `dependencies` value that is not an array of strings.
pub fn dependencies(source: &str) -> Result<Vec<String>, Pep723Error> {
    let mut scripts = BLOCK.captures_iter(source).filter(|caps| &caps["type"] == "script");
    let Some(block) = scripts.next() else {
        return Ok(Vec::new());
    };
    if scripts.next().is_some() {
        return Err(Pep723Error::MultipleBlocks);
    }

    let body = strip_comment_prefix(&block["content"]);
    let metadata = body.parse::<Table>().map_err(Pep723Error::Toml)?;
    match metadata.get("dependencies") {
        None => Ok(Vec::new()),
        Some(value) => requirement_strings(value),
    }
}

/// Removes the `# ` (or bare `#`) comment prefix from each content line,
/// reconstructing the raw TOML body. Mirrors the PEP 723 reference: lines are
/// either `# …` or a bare `#`, so stripping `# ` then `#` recovers the content.
fn strip_comment_prefix(content: &str) -> String {
    let mut body = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        body.push_str(
            line.strip_prefix("# ")
                .or_else(|| line.strip_prefix('#'))
                .unwrap_or(line),
        );
    }
    body
}

/// Interprets a TOML `dependencies` value as a list of requirement strings,
/// erroring unless it is an array whose every element is a string.
fn requirement_strings(value: &Value) -> Result<Vec<String>, Pep723Error> {
    value
        .as_array()
        .ok_or(Pep723Error::InvalidDependencies)?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or(Pep723Error::InvalidDependencies))
        .collect()
}
