use std::{
    error::Error,
    fmt::{self, Display},
    str::FromStr,
};

pub use ruff_python_stdlib::identifiers::is_identifier;
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
    /// Committed imports, injected before the stubs' star import to preserve bindings `.pyi` files omit.
    #[serde(default)]
    pub committed_imports: String,
    /// Imports from `pending_snippet`, appended to `committed_imports` on commit.
    #[serde(default)]
    pub pending_imports: String,
    /// Modules already requested from the host, including modules without stubs, to avoid asking twice.
    #[serde(default)]
    pub asked_modules: Vec<String>,
}

/// A host module's `.pyi`, written as a module or package tree for the type checker.
/// Construction validates each path component and rejects top-level names reserved by the
/// sandbox or checker, preventing stubs from shadowing their modules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleStub {
    module: String,
    source: String,
}

impl ModuleStub {
    /// Validates each dotted component as an identifier and rejects top-level names reserved
    /// by [`validate_module_name`] or [`CHECKER_MODULE_NAMES`].
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

    /// The dotted module path.
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

/// Rejects non-identifiers and [`SANDBOX_MODULE_NAMES`], whose imports never reach the host.
/// [`CHECKER_MODULE_NAMES`] are valid runtime module names; only [`ModuleStub::new`] rejects them.
pub fn validate_module_name(name: &str) -> Result<(), ModuleStubError> {
    if !is_identifier(name) {
        Err(ModuleStubError::InvalidName(name.to_owned()))
    } else if SANDBOX_MODULE_NAMES.contains(&name) {
        Err(ModuleStubError::ReservedName(name.to_owned()))
    } else {
        Ok(())
    }
}

/// Preserves reserved-name errors; other invalid components report the full dotted path.
fn invalid_path(path: &str, part: &str) -> ModuleStubError {
    if SANDBOX_MODULE_NAMES.contains(&part) {
        ModuleStubError::ReservedName(part.to_owned())
    } else {
        ModuleStubError::InvalidName(path.to_owned())
    }
}

/// An invalid or reserved host module name; see [`validate_module_name`].
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

/// Sandbox module names, excluding the dotted name `os.path`.
/// Imports of these modules never reach the host, so host modules cannot use these names.
/// Keep in sync with the runtime's `StandardLib`.
pub const SANDBOX_MODULE_NAMES: &[&str] = &[
    "asyncio",
    "base64",
    "binascii",
    "bisect",
    "collections",
    "copy",
    "csv",
    "dataclasses",
    "datetime",
    "decimal",
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
    "tomllib",
    "typing",
    "unicodedata",
];

/// Vendored typeshed modules outside [`SANDBOX_MODULE_NAMES`], including private dependencies,
/// plus ty's `ty_extensions`. Hosts may provide these modules at runtime, but
/// [`ModuleStub::new`] rejects their stubs to prevent shadowing the checker's definitions.
pub const CHECKER_MODULE_NAMES: &[&str] = &[
    "__future__",
    "__main__",
    "_collections_abc",
    "_io",
    "_typeshed",
    "abc",
    "builtins",
    "enum",
    "genericpath",
    "io",
    "ty_extensions",
    "types",
    "typing_extensions",
];
