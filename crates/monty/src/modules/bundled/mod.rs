//! CPython stdlib modules bundled as CPython's own Python source.
//!
//! `scripts/vendor_stdlib.py` copies the files listed in `crates/monty/stdlib/vendor.toml` from a pinned CPython
//! release, applies `crates/monty/stdlib/patches/`, and generates [`sources`]. An import finds these after the
//! native modules and before the host, and runs them like a module from a file: compiled per session, cached by
//! name, with no OS call.

mod sources;

/// A vendored module's source, embedded in the binary.
pub(crate) struct BundledSource {
    /// The dotted module name, e.g. `colorsys` or `pkg.sub`.
    name: &'static str,
    /// The path under CPython's `Lib/`, e.g. `colorsys.py` or `pkg/__init__.py`.
    file: &'static str,
    source: &'static str,
}

impl BundledSource {
    /// The bundled module called `name`, if there is one.
    pub(crate) fn find(name: &str) -> Option<&'static Self> {
        sources::SOURCES.iter().find(|bundled| bundled.name == name)
    }

    /// The module's `__file__`, which also names it in tracebacks; it is not a path in the sandbox's filesystem.
    pub(crate) fn path(&self) -> String {
        format!("{BUNDLED_PREFIX}{}", self.file)
    }

    pub(crate) fn source(&self) -> &'static str {
        self.source
    }
}

/// The prefix of every bundled module's `__file__`, chosen so it cannot collide with a sandbox path.
pub(crate) const BUNDLED_PREFIX: &str = "<stdlib>/";
