use std::fmt::{self, Display};

use ruff_db::{
    diagnostic::{
        Annotation, Diagnostic, DiagnosticFormat, DiagnosticId, DisplayDiagnosticConfig, DisplayDiagnostics,
        UnifiedFile,
    },
    files::{File, system_path_to_file},
    system::{DbWithWritableSystem as _, SystemPathBuf},
};
use ruff_text_size::{TextRange, TextSize};
use ty_python_semantic::types::check_types;

use crate::db::{SRC_ROOT, global_db, next_call_prefix, strip_unique_prefix};

/// Definition of a source file.
pub struct SourceFile<'a> {
    /// source code
    pub source_code: &'a str,
    /// file path
    pub path: &'a str,
}

impl<'a> SourceFile<'a> {
    /// Create a new source file.
    #[must_use]
    pub fn new(source_code: &'a str, path: &'a str) -> Self {
        Self { source_code, path }
    }
}

/// Type check some python source code, checking if it's valid to run with monty.
///
/// All `type_check` calls share the process-wide warm database (see [`global_db`])
/// to amortize typeshed parsing across calls. Calls are serialized on a mutex so
/// salsa's single-writer invariant holds, and each call writes to a unique internal
/// path (prefixed with `__mc<N>__`) so its salsa `File` ingredient is independent
/// of every other call's — diagnostics remain correct across concurrent holders.
///
/// # Arguments
/// * `python_source` - The python source code to type check.
/// * `stubs_file` - Optional stubs file to use for type checking.
///
/// # Returns
/// * `Ok(Some(TypeCheckingFailure))` - If there are typing errors.
/// * `Ok(None)` - If there are no typing errors.
/// * `Err(String)` - If there was an unexpected/internal error during type checking.
pub fn type_check(
    python_source: &SourceFile<'_>,
    stubs_file: Option<&SourceFile<'_>>,
) -> Result<Option<TypeCheckingDiagnostics>, String> {
    let mut db = global_db().lock().map_err(|e| e.to_string())?;
    let src_root = SystemPathBuf::from(SRC_ROOT);
    let call_prefix = next_call_prefix();

    // Per-call unique path — `__mc<N>__<user_path>` — prevents the call's `File`
    // ingredient from colliding with any other call's and keeps earlier diagnostics
    // valid after later calls have run.
    let main_path = unique_path(&src_root, &call_prefix, python_source.path);
    let main_source = python_source.source_code;

    let code_offset: u32 = if let Some(stubs_file) = stubs_file {
        let stubs_path = unique_path(&src_root, &call_prefix, stubs_file.path);

        // write the stub file
        db.write_file(&stubs_path, stubs_file.source_code).map_err(to_string)?;

        // prepend the stub import to the main source code. Use the uniquified stub
        // module name so the import resolves against this call's stub file only.
        let stub_basename = stubs_file.path.rsplit('/').next().unwrap_or(stubs_file.path);
        let stub_stem = stub_basename
            .split_once('.')
            .map_or(stub_basename, |(before, _)| before);
        let unique_stub_module = format!("{call_prefix}{stub_stem}");
        let mut new_source = format!("from {unique_stub_module} import *\n");
        let offset = u32::try_from(new_source.len()).map_err(to_string)?;
        new_source.push_str(main_source);

        // write the main source code
        db.write_file(&main_path, &new_source).map_err(to_string)?;
        // one line offset for errors vs. the original source code since we injected the stub import
        offset
    } else {
        // write just the main source code
        db.write_file(&main_path, main_source).map_err(to_string)?;
        0
    };

    let main_file = system_path_to_file(&*db, &main_path).map_err(to_string)?;
    let mut diagnostics = check_types(&*db, main_file);
    diagnostics.retain(filter_diagnostics);

    if diagnostics.is_empty() {
        Ok(None)
    } else {
        // without all this errors would appear on the wrong line because we injected `from <stub> import *`

        // if we injected the stubs import, we need to write the actual source back to the file in the database
        db.write_file(&main_path, main_source).map_err(to_string)?;
        // and then adjust each span in the error message to account for the injected stubs import
        if code_offset > 0 {
            let offset = TextSize::new(code_offset);
            for diagnostic in &mut diagnostics {
                // Adjust spans in main diagnostic annotations (only for spans in the main file)
                for ann in diagnostic.annotations_mut() {
                    adjust_annotation_span(ann, main_file, offset);
                }
                // Adjust spans in sub-diagnostic annotations (e.g., "info: Function defined here")
                for sub in diagnostic.sub_diagnostics_mut() {
                    for ann in sub.annotations_mut() {
                        adjust_annotation_span(ann, main_file, offset);
                    }
                }
            }
        }
        // Sort diagnostics by line number
        diagnostics.sort_by(|a, b| a.rendering_sort_key(&*db).cmp(&b.rendering_sort_key(&*db)));

        Ok(Some(TypeCheckingDiagnostics::new(diagnostics)))
    }
}

/// Compose the per-call unique path for a user-supplied script name.
///
/// The prefix is applied to the basename so that `src/app.py` becomes
/// `/__mc42__src/app.py` (not `/src/__mc42__app.py`) — simpler to strip and keeps
/// rendered output close to the user's original path.
fn unique_path(src_root: &SystemPathBuf, call_prefix: &str, user_path: &str) -> SystemPathBuf {
    // Trim leading slashes so joining under src_root stays within the project.
    let trimmed = user_path.trim_start_matches('/');
    src_root.join(format!("{call_prefix}{trimmed}"))
}

fn to_string(err: impl Display) -> String {
    err.to_string()
}

/// Adjust the span of an annotation by subtracting the given offset.
///
/// This is used when we inject a stub import at the beginning of the source code,
/// and need to adjust all spans to account for the injected code.
/// Only adjusts spans that belong to the main file being type-checked.
fn adjust_annotation_span(ann: &mut Annotation, main_file: File, offset: TextSize) {
    let span = ann.get_span();
    // Only adjust spans for the main file (not stubs or other files)
    if let UnifiedFile::Ty(span_file) = span.file()
        && *span_file == main_file
        && let Some(range) = span.range()
    {
        let new_range = TextRange::new(range.start() - offset, range.end() - offset);
        let new_span = span.clone().with_range(new_range);
        ann.set_span(new_span);
    }
}

/// Represents diagnostic details when type checking fails.
///
/// Doesn't hold a database clone: rendering acquires the process-wide [`global_db`]
/// mutex on demand. This matters for correctness — an `Arc<MemoryDb>` captured here
/// would pin salsa's `Arc<Zalsa>` refcount above 1 and deadlock the next `write_file`
/// setter (salsa's `Arc::get_mut` spins until refcount drops to 1). The diagnostic's
/// `File` ingredients are unique per call, so re-querying the db at display time
/// still returns the original source text produced during this diagnostic's call.
#[derive(Clone)]
pub struct TypeCheckingDiagnostics {
    /// The actual diagnostic message
    diagnostics: Vec<Diagnostic>,
    /// How to format the output
    format: DiagnosticFormat,
    /// Whether to highlight the output with ansi colors
    color: bool,
}

/// Debug output for TypeCheckingDiagnostics shows the pretty typing output, and no other values since
/// this will be displayed when users are printing `Result<..., TypeCheckingDiagnostics>` etc. and the
/// raw errors are not useful to end users.
impl fmt::Debug for TypeCheckingDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeCheckingDiagnostics:\n{self}")
    }
}

/// To display true debugs details about the TypeCheckingDiagnostics
#[derive(Debug)]
#[expect(dead_code)]
pub struct DebugTypeCheckingDiagnostics<'a> {
    diagnostics: &'a [Diagnostic],
    format: DiagnosticFormat,
    color: bool,
}

impl fmt::Display for TypeCheckingDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rendered = {
            let db = global_db().lock().expect("global db mutex poisoned");
            DisplayDiagnostics::new(&*db, &self.config(), &self.diagnostics).to_string()
        };
        // Hide the internal `__mc<N>__` uniquifying prefix from user-visible output so
        // file paths in error messages read `main.py`, not `__mc42__main.py`.
        f.write_str(&strip_unique_prefix(&rendered))
    }
}

impl TypeCheckingDiagnostics {
    fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            diagnostics,
            format: DiagnosticFormat::Full,
            color: false,
        }
    }

    fn config(&self) -> DisplayDiagnosticConfig {
        DisplayDiagnosticConfig::new("monty")
            .format(self.format)
            .color(self.color)
    }

    /// To display debug details for the TypeCheckingDiagnostics since debug is the pretty output
    #[must_use]
    pub fn debug_details(&self) -> DebugTypeCheckingDiagnostics<'_> {
        DebugTypeCheckingDiagnostics {
            diagnostics: &self.diagnostics,
            format: self.format,
            color: self.color,
        }
    }

    /// Set the format of the diagnostics.
    #[must_use]
    pub fn format(self, format: DiagnosticFormat) -> Self {
        Self { format, ..self }
    }

    /// Set the format of the diagnostics from a string.
    /// Valid formats: "full", "concise", "azure", "json", "jsonlines", "rdjson",
    /// "pylint", "gitlab", "github".
    pub fn format_from_str(self, format: &str) -> Result<Self, String> {
        let format = match format.to_ascii_lowercase().as_str() {
            "full" => DiagnosticFormat::Full,
            "concise" => DiagnosticFormat::Concise,
            "azure" => DiagnosticFormat::Azure,
            "json" => DiagnosticFormat::Json,
            "jsonlines" | "json-lines" => DiagnosticFormat::JsonLines,
            "rdjson" => DiagnosticFormat::Rdjson,
            "pylint" => DiagnosticFormat::Pylint,
            // don't bother with the "junit" feature, please check the binary size and add it if you need this format
            // "junit" => DiagnosticFormat::Junit,
            "gitlab" => DiagnosticFormat::Gitlab,
            "github" => DiagnosticFormat::Github,
            _ => return Err(format!("Unknown format: {format}")),
        };
        Ok(Self { format, ..self })
    }

    /// Set whether to highlight the output with ansi colors
    #[must_use]
    pub fn color(self, color: bool) -> Self {
        Self { color, ..self }
    }
}

/// Filter out diagnostics we want to ignore.
///
/// Should only be necessary until <https://github.com/astral-sh/ty/issues/2599> is fixed.
fn filter_diagnostics(d: &Diagnostic) -> bool {
    !(matches!(d.id(), DiagnosticId::InvalidSyntax)
        && matches!(
            d.primary_message(),
            "`await` statement outside of a function" | "`await` outside of an asynchronous function"
        ))
}
