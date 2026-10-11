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
    /// The import statements of every committed snippet, re-injected ahead of
    /// the stubs' star import, which does not re-export a `.pyi`'s imports.
    #[serde(default)]
    pub committed_imports: String,
    /// The import statements of `pending_snippet`, joining `committed_imports`
    /// when it commits.
    #[serde(default)]
    pub pending_imports: String,
    /// Every top-level module the host has been asked stubs for, with or
    /// without an answer, so each is asked about once per session.
    #[serde(default)]
    pub asked_modules: Vec<String>,
}

/// A `.pyi` for one host-provided module, written for the type checker so
/// `import <module>` resolves: `/<module>.pyi`, or inside a package tree for a
/// dotted path (`a.b` is `/a/b.pyi` beside `/a/__init__.pyi`). The path is
/// validated on construction: identifiers, the first not one the vendored
/// typeshed has, or the stub would shadow it for the checker alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleStub {
    module: String,
    source: String,
}

impl ModuleStub {
    /// A stub for `module`, a dotted path whose top-level name
    /// [`validate_module_name`] accepts, is not in [`CHECKER_MODULE_NAMES`]
    /// either, and whose other components are identifiers.
    pub fn new(module: impl Into<String>, source: impl Into<String>) -> Result<Self, ModuleStubError> {
        let module = module.into();
        let mut parts = module.split('.');
        let top = parts.next().unwrap_or_default();
        validate_module_name(top).map_err(|_| invalid_path(&module, top))?;
        if CHECKER_MODULE_NAMES.contains(&top) {
            return Err(ModuleStubError::ReservedName(top.to_owned()));
        }
        if let Some(part) = parts.find(|part| !is_identifier(part)) {
            return Err(invalid_path(&module, part));
        }
        Ok(Self {
            module,
            source: source.into(),
        })
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

/// Refuses a name a host-provided module may not have: not an identifier, or
/// one of [`SANDBOX_MODULE_NAMES`], whose import never reaches the host. A
/// name in [`CHECKER_MODULE_NAMES`] passes, since the runtime does ask the
/// host for it; only a stub under it is refused, by [`ModuleStub::new`].
pub fn validate_module_name(name: &str) -> Result<(), ModuleStubError> {
    if !is_identifier(name) {
        Err(ModuleStubError::InvalidName(name.to_owned()))
    } else if SANDBOX_MODULE_NAMES.contains(&name) {
        Err(ModuleStubError::ReservedName(name.to_owned()))
    } else {
        Ok(())
    }
}

/// The error for a dotted stub path: a reserved top-level name keeps its own
/// error, any other bad component makes the whole path invalid.
fn invalid_path(path: &str, part: &str) -> ModuleStubError {
    if SANDBOX_MODULE_NAMES.contains(&part) {
        ModuleStubError::ReservedName(part.to_owned())
    } else {
        ModuleStubError::InvalidName(path.to_owned())
    }
}

/// Why a host-provided module's name was refused (see [`validate_module_name`]).
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
            Self::InvalidName(name) => write!(f, "module name {name:?} is not a valid identifier"),
            Self::ReservedName(name) => write!(
                f,
                "module {name:?} is provided by the sandbox or its type checker and cannot be replaced"
            ),
        }
    }
}

impl Error for ModuleStubError {}

/// The import names of every module the sandbox provides itself, `os.path`
/// aside (a dotted name is never an identifier). An `external_modules` entry
/// under one would never be asked for, so a host module may not take one; keep
/// in step with the runtime's `StandardLib`.
pub const SANDBOX_MODULE_NAMES: &[&str] = &[
    "asyncio",
    "base64",
    "binascii",
    "collections",
    "copy",
    "dataclasses",
    "datetime",
    "functools",
    "gc",
    "hashlib",
    "itertools",
    "json",
    "math",
    "os",
    "pathlib",
    "posixpath",
    "random",
    "re",
    "sys",
    "time",
    "typing",
    "unicodedata",
];

/// The modules of the vendored typeshed beyond [`SANDBOX_MODULE_NAMES`] (the
/// private modules its stubs import included), plus `ty_extensions`, which ty
/// provides without a typeshed entry. The runtime asks the host for these, so
/// a host module may take one, but a stub under it would shadow typeshed's for
/// the checker alone, so [`ModuleStub::new`] refuses it.
pub const CHECKER_MODULE_NAMES: &[&str] = &[
    "__future__",
    "__main__",
    "_collections_abc",
    "_typeshed",
    "abc",
    "builtins",
    "enum",
    "genericpath",
    "ty_extensions",
    "types",
    "typing_extensions",
];
