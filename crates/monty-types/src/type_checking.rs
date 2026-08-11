use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::VariantNames;

/// How type-check diagnostics are rendered into text.
///
/// Mirrors ty's `DiagnosticFormat`. Rendering happens wherever the type checker
/// runs (inside the worker for pool sessions), because ty's structured
/// diagnostics borrow the salsa database and cannot cross a process boundary —
/// so the format has to be chosen before the check, not after it.
///
/// Serialized into session dumps by discriminant, so new variants must be
/// appended — inserting one shifts every later variant and silently rewrites
/// older dumps' format (see `DUMP_VERSION` in `monty`).
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
}
