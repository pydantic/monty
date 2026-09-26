use std::{
    error::Error,
    fmt::{self, Display},
    str::FromStr,
};

use ruff_python_stdlib::identifiers::is_identifier;
use serde::{Deserialize, Serialize};
use strum::VariantNames;

/// How type-check diagnostics are rendered into text.
///
/// Mirrors ty's `DiagnosticFormat`. Rendering happens wherever the type checker
/// runs (inside the worker for pool sessions), because ty's structured
/// diagnostics borrow the salsa database and cannot cross a process boundary —
/// so the format has to be chosen before the check, not after it.
///
/// Serialized into session dumps by variant name, so a rename needs
/// `#[serde(alias)]` to keep older dumps loading (see `DUMP_VERSION` in `monty`).
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::VariantNames,
)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum TypeCheckingFormat {
    /// Human-readable diagnostics with a source snippet and carets.
    #[default]
    Full,
    /// One `path:line:col: severity[rule] message` line per diagnostic.
    Concise,
    /// Azure Pipelines logging commands.
    Azure,
    /// A JSON array of diagnostic objects.
    Json,
    /// One JSON diagnostic object per line.
    #[strum(to_string = "jsonlines", serialize = "json-lines")]
    JsonLines,
    /// Reviewdog diagnostic JSON.
    Rdjson,
    /// Pylint-compatible output.
    Pylint,
    /// GitLab Code Quality report JSON.
    Gitlab,
    /// GitHub Actions workflow commands.
    Github,
}

impl TypeCheckingFormat {
    /// Parses a format name, reporting the valid names on failure.
    ///
    /// Bindings take the format as a string, so the error has to be good
    /// enough to show a user who guessed wrong.
    pub fn from_name(name: &str) -> Result<Self, String> {
        Self::from_str(name)
            .map_err(|_| format!("unknown type check format '{name}', expected one of: {}", Self::names()))
    }

    /// Comma-separated list of the accepted format names.
    #[must_use]
    pub fn names() -> String {
        Self::VARIANTS.join(", ")
    }
}

/// How a type check renders whatever diagnostics it finds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeCheckingConfig {
    /// Output format.
    pub format: TypeCheckingFormat,
    /// Whether to include ANSI colour escapes. Only `Full` and `Concise`
    /// render any colour; the machine-readable formats ignore it.
    pub color: bool,
}

/// Per-session type-check state: successfully committed snippets accumulate as
/// stubs so later snippets can reference names defined by earlier ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeCheckState {
    /// User-provided stubs plus every snippet that has completed successfully.
    pub committed_stubs: String,
    /// The in-flight snippet; committed on success, discarded on error.
    pub pending_snippet: Option<String>,
    /// How diagnostics are rendered by whoever runs the type checker.
    pub config: TypeCheckingConfig,
    /// Stubs for host-provided modules, one file each, kept for the session.
    #[serde(default)]
    pub module_stubs: Vec<ModuleStub>,
    /// The import statements of every committed snippet, re-injected ahead of
    /// the stubs' star import, which does not re-export a `.pyi`'s imports.
    #[serde(default)]
    pub committed_imports: String,
}

/// A `.pyi` for one host-provided module, written as `/<module>.pyi` for the
/// type checker so `import <module>` resolves. The name is validated on
/// construction: an identifier the sandbox's own stdlib does not use, or the
/// runtime and the checker would disagree about what the import gives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleStub {
    module: String,
    source: String,
}

impl ModuleStub {
    /// A stub for `module`, refusing a name that is not an identifier or is
    /// one of [`RESERVED_MODULE_NAMES`].
    pub fn new(module: impl Into<String>, source: impl Into<String>) -> Result<Self, ModuleStubError> {
        let module = module.into();
        if !is_identifier(&module) {
            Err(ModuleStubError::InvalidName(module))
        } else if RESERVED_MODULE_NAMES.contains(&module.as_str()) {
            Err(ModuleStubError::ReservedName(module))
        } else {
            Ok(Self {
                module,
                source: source.into(),
            })
        }
    }

    /// The module the stub describes.
    #[must_use]
    pub fn module(&self) -> &str {
        &self.module
    }

    /// The `.pyi` source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Why a [`ModuleStub`] name was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleStubError {
    /// Not a Python identifier, or a keyword.
    InvalidName(String),
    /// A module the sandbox provides itself, or one its type stubs rely on.
    ReservedName(String),
}

impl Display for ModuleStubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "module stub name {name:?} is not a valid identifier"),
            Self::ReservedName(name) => write!(f, "module {name:?} is provided by the sandbox and cannot take a stub"),
        }
    }
}

impl Error for ModuleStubError {}

/// Module names a [`ModuleStub`] may not use: every module of the vendored
/// typeshed (the sandbox's own stdlib and the private modules its stubs
/// import), plus the names an `import` never asks the host for. A stub under
/// one of these would shadow that module for the checker alone.
pub const RESERVED_MODULE_NAMES: &[&str] = &[
    "__future__",
    "__main__",
    "_collections_abc",
    "_typeshed",
    "abc",
    "asyncio",
    "base64",
    "binascii",
    "builtins",
    "collections",
    "copy",
    "dataclasses",
    "datetime",
    "enum",
    "functools",
    "gc",
    "itertools",
    "json",
    "math",
    "os",
    "pathlib",
    "random",
    "re",
    "sys",
    "time",
    "ty_extensions",
    "types",
    "typing",
    "typing_extensions",
    "unicodedata",
];
