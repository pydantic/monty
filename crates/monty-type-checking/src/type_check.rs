use std::{
    fmt,
    sync::{Arc, Mutex},
};

use ruff_db::{
    diagnostic::{
        Annotation, Diagnostic, DiagnosticFormat, DiagnosticId, DisplayDiagnosticConfig, DisplayDiagnostics,
        UnifiedFile,
    },
    files::File,
    system::SystemPathBuf,
};
use ruff_text_size::{TextRange, TextSize};
use ty_python_semantic::types::check_types;

use crate::{
    db::SRC_ROOT,
    pool::{PooledMemoryDb, to_string},
};

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
    // Check out a pre-configured db from the global pool. The `Drop` impl on
    // `PooledMemoryDb` scrubs every file (and now also every parent directory) we
    // write below and returns the db to the pool when the lease is no longer
    // reachable — either at the end of this function (clean run) or when the
    // returned `TypeCheckingDiagnostics` is dropped.
    let mut pooled_db = PooledMemoryDb::checkout()?;

    let src_root = SystemPathBuf::from(SRC_ROOT);
    let main_path = src_root.join(python_source.path);
    let main_source = python_source.source_code;

    let (main_file, code_offset): (File, u32) = if let Some(stubs_file) = stubs_file {
        let stubs_path = src_root.join(stubs_file.path);
        pooled_db.write_root_file(&stubs_path, stubs_file.source_code)?;

        // prepend the stub import to the main source code
        let stub_stem = stubs_file
            .path
            .split_once('.')
            .map_or(stubs_file.path, |(before, _)| before);
        let mut new_source = format!("from {stub_stem} import *\n");
        let offset = u32::try_from(new_source.len()).map_err(to_string)?;
        new_source.push_str(main_source);

        let main_file = pooled_db.write_root_file(&main_path, &new_source)?;
        // one line offset for errors vs. the original source code since we injected the stub import
        (main_file, offset)
    } else {
        let main_file = pooled_db.write_root_file(&main_path, main_source)?;
        (main_file, 0)
    };

    let mut diagnostics = check_types(pooled_db.db(), main_file);
    diagnostics.retain(filter_diagnostics);

    if diagnostics.is_empty() {
        Ok(None)
    } else {
        // without all this errors would appear on the wrong line because we injected `from type_stubs import *`

        // if we injected the stubs import, we need to write the actual source back to the file in the database
        pooled_db.rewrite_root_file(&main_path, main_source)?;
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
        let db = pooled_db.db();
        diagnostics.sort_by(|a, b| a.rendering_sort_key(db).cmp(&b.rendering_sort_key(db)));

        Ok(Some(TypeCheckingDiagnostics::new(diagnostics, pooled_db)))
    }
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
/// The pooled database is held inside an `Arc<Mutex<...>>` so that:
/// 1. Diagnostic rendering can borrow the db lazily on every `Display`/`Debug` call,
///    avoiding eager pre-rendering of every output format.
/// 2. The `MontyTypingError` Python exception that wraps this type stays `Send + Sync`.
/// 3. The `PooledMemoryDb` is released back to the pool exactly when the last clone
///    of this `Arc` is dropped — RAII via `PooledMemoryDb`'s `Drop` impl.
#[derive(Clone)]
pub struct TypeCheckingDiagnostics {
    /// The actual diagnostic message
    diagnostics: Vec<Diagnostic>,
    /// Pooled db used to display diagnostics. Wrapped in `Mutex` for `Sync` so
    /// `MontyTypingError` is sendable; the inner `Drop` impl releases the db when
    /// the last `Arc` clone is dropped.
    pooled_db: Arc<Mutex<PooledMemoryDb>>,
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
        let config = self.config();
        let pooled_db = self.pooled_db.lock().unwrap();
        write!(
            f,
            "TypeCheckingDiagnostics:\n{}",
            DisplayDiagnostics::new(pooled_db.db(), &config, &self.diagnostics)
        )
    }
}

/// To display true debugs details about the TypeCheckingDiagnostics
#[derive(Debug)]
#[expect(dead_code)]
pub struct DebugTypeCheckingDiagnostics<'a> {
    diagnostics: &'a [Diagnostic],
    pooled_db: Arc<Mutex<PooledMemoryDb>>,
    format: DiagnosticFormat,
    color: bool,
}

impl fmt::Display for TypeCheckingDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pooled_db = self.pooled_db.lock().unwrap();
        DisplayDiagnostics::new(pooled_db.db(), &self.config(), &self.diagnostics).fmt(f)
    }
}

impl TypeCheckingDiagnostics {
    fn new(diagnostics: Vec<Diagnostic>, pooled_db: PooledMemoryDb) -> Self {
        Self {
            diagnostics,
            pooled_db: Arc::new(Mutex::new(pooled_db)),
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
            pooled_db: self.pooled_db.clone(),
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
