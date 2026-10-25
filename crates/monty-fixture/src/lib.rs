//! The `# /// monty` header that configures a test case (`crates/monty/test_cases/`) or a
//! benchmark script (`crates/monty-bench/benches/python/`).
//!
//! The header is a [PEP 723](https://peps.python.org/pep-0723/) comment block of TOML, so
//! both the datatest harness and the benchmarks read it through one strict parser:
//!
//! ```python
//! # /// monty
//! # call-external = true
//! # xfail = ['cpython']
//! # [bench]
//! # limits = true
//! # ///
//! ```

use std::{error::Error, fmt};

use serde::Deserialize;

/// The line that opens a monty header block.
const BLOCK_START: &str = "# /// monty";
/// The line that closes any PEP 723 block.
const BLOCK_END: &str = "# ///";

/// A fixture's settings; every field defaults to off, so a file without a header gets
/// `Header::default()`. Unknown keys are errors, so a typo can't silently drop a setting.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
#[expect(clippy::struct_excessive_bools, reason = "one bool per TOML flag")]
pub struct Header {
    /// Run with external-function support, driving `RunProgress` suspensions.
    pub call_external: bool,
    /// Wrap the code in an async context for CPython, allowing top-level `await`.
    pub run_async: bool,
    /// Mount a fixture directory: `/mnt` for Monty, a real path for CPython.
    pub mount_fs: bool,
    /// Skip the CPython side on Windows, for POSIX path semantics only Monty provides there.
    pub skip_cpython_windows: bool,
    /// Set `__name__ = '__main__'` for CPython only.
    pub cpython_main_module: bool,
    /// Interpreters the case must fail on (strict: passing there is an error).
    pub xfail: Vec<Interpreter>,
    /// Overrides the GC interval of the case's resource limits.
    pub gc_interval: Option<usize>,
    /// IANA zone both sides run in, instead of UTC.
    pub timezone: Option<String>,
    /// Benchmark options, read only by `monty-bench`.
    pub bench: Bench,
}

impl Header {
    /// Reads the `# /// monty` block from `source`, or returns the default header if there is none.
    ///
    /// Errors on malformed TOML, unknown keys, an unterminated, repeated or misplaced block, and
    /// any other `# ///` line, such as a misspelt block type.
    pub fn parse(source: &str) -> Result<Self, HeaderError> {
        match extract_block(source)? {
            Some((start, toml)) => toml::from_str(&toml)
                .map_err(|err| HeaderError(format!("in the header opening on line {}: {err}", start + 1))),
            None => Ok(Self::default()),
        }
    }

    /// Whether the case is expected to fail on `interpreter`.
    #[must_use]
    pub fn xfails(&self, interpreter: Interpreter) -> bool {
        self.xfail.contains(&interpreter)
    }
}

/// An interpreter a test case runs on, as named by `xfail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interpreter {
    Monty,
    Cpython,
}

/// The `[bench]` table: how `monty-bench` runs a script.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bench {
    /// Bind `DATA` to the contents of this file, relative to the script.
    pub data: Option<String>,
    /// Bind `DATA` to the root of the benchmarks' mounted scan tree.
    pub mount: bool,
    /// With `mount`, also benchmark over an overlay mount.
    pub overlay: bool,
    /// Also benchmark with resource limits armed.
    pub limits: bool,
}

/// Why a header couldn't be read.
#[derive(Debug)]
pub struct HeaderError(String);

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for HeaderError {}

/// Returns the index of the `# /// monty` line and the TOML inside its block, each line's `#`
/// prefix removed.
///
/// The block must sit in the file's leading comment and blank lines, so code and strings can't
/// configure a fixture; a block marker anywhere after them is an error rather than ignored.
fn extract_block(source: &str) -> Result<Option<(usize, String)>, HeaderError> {
    let mut block: Option<(usize, String)> = None;
    let mut before_code = true;
    let mut lines = source.lines().enumerate();
    while let Some((index, line)) = lines.next() {
        before_code &= line.starts_with('#') || line.trim().is_empty();
        if !is_block_marker(line) {
            continue;
        }
        let problem = if line != BLOCK_START {
            format!("`{line}` is not `{BLOCK_START}` or a block's closing `{BLOCK_END}`")
        } else if !before_code {
            format!("`{BLOCK_START}` must come before any code")
        } else if block.is_some() {
            format!("second `{BLOCK_START}` block")
        } else {
            block = Some((index, read_block_body(&mut lines, index)?));
            continue;
        };
        return Err(HeaderError(format!("line {}: {problem}", index + 1)));
    }
    Ok(block)
}

/// Reads the comment lines after the opening line at `start` up to the closing `# ///`.
fn read_block_body<'a>(
    lines: &mut impl Iterator<Item = (usize, &'a str)>,
    start: usize,
) -> Result<String, HeaderError> {
    let mut toml = String::new();
    for (index, line) in lines {
        if line == BLOCK_END {
            return Ok(toml);
        }
        // PEP 723: every body line is `#` alone or `# ` followed by content.
        let content = if line == "#" {
            ""
        } else {
            line.strip_prefix("# ").ok_or_else(|| {
                HeaderError(format!(
                    "line {}: `{line}` inside the header is not a `# ` comment",
                    index + 1
                ))
            })?
        };
        toml.push_str(content);
        toml.push('\n');
    }
    Err(HeaderError(format!(
        "line {}: `{BLOCK_START}` is never closed",
        start + 1
    )))
}

/// Whether a comment looks like a PEP 723 block delimiter, such as `#/// monty` or `# ///monty`.
fn is_block_marker(line: &str) -> bool {
    line.starts_with('#') && line.trim_start_matches('#').trim_start().starts_with("///")
}
