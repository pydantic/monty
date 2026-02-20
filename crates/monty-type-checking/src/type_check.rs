use std::{
    fmt::{self, Display},
    sync::{Arc, Mutex},
};

use ruff_db::{
    Db as SourceDb,
    diagnostic::{
        Annotation, Diagnostic, DiagnosticFormat, DiagnosticId, DisplayDiagnosticConfig, DisplayDiagnostics,
        UnifiedFile,
    },
    files::{File, FileRootKind, system_path_to_file},
    system::{DbWithWritableSystem as _, SystemPathBuf},
};
use ruff_text_size::{TextRange, TextSize};
use ty_module_resolver::SearchPathSettings;
use ty_python_semantic::{
    Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource, types::check_types,
};

use crate::db::MemoryDb;

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
    let mut db = MemoryDb::new();

    // Files must be written under a directory that's registered as a search path for module
    // resolution to work. We use "/" as the root directory so paths appear without a prefix.
    let src_root = SystemPathBuf::from("/");

    // Register the source root for Salsa tracking - required for module resolution
    db.files().try_add_root(&db, &src_root, FileRootKind::Project);

    let search_paths = SearchPathSettings::new(vec![src_root.clone()])
        .to_search_paths(db.system(), db.vendored())
        .map_err(to_string)?;

    // The API is confusing here - we have to load the "program" here like this, otherwise we get unwrap
    // panics when calling `check_types`
    Program::from_settings(
        &db,
        ProgramSettings {
            python_version: PythonVersionWithSource {
                version: db.python_version(),
                source: PythonVersionSource::default(),
            },
            python_platform: PythonPlatform::default(),
            search_paths,
        },
    );

    // Build absolute paths for files under /
    let main_path = src_root.join(python_source.path);
    let main_source = python_source.source_code;

    let code_offset: u32 = if let Some(stubs_file) = stubs_file {
        let stubs_path = src_root.join(stubs_file.path);

        // write the stub file
        db.write_file(&stubs_path, stubs_file.source_code).map_err(to_string)?;

        // prepend the stub import to the main source code
        let stub_stem = stubs_file
            .path
            .split_once('.')
            .map_or(stubs_file.path, |(before, _)| before);
        let mut new_source = format!("from {stub_stem} import *\n");
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

    let main_file = system_path_to_file(&db, &main_path).map_err(to_string)?;
    let mut diagnostics = check_types(&db, main_file);
    diagnostics.retain(filter_diagnostics);

    if diagnostics.is_empty() {
        Ok(None)
    } else {
        // without all this errors would appear on the wrong line because we injected `from type_stubs import *`

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
        diagnostics.sort_by(|a, b| a.rendering_sort_key(&db).cmp(&b.rendering_sort_key(&db)));

        Ok(Some(TypeCheckingDiagnostics::new(diagnostics, db)))
    }
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
#[derive(Clone)]
pub struct TypeCheckingDiagnostics {
    /// The actual diagnostic message
    diagnostics: Vec<Diagnostic>,
    /// db used to display diagnostics, wrapped in Mutex for Sync so MontyTypingError is sendable
    db: Arc<Mutex<MemoryDb>>,
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
        let db = self.db.lock().unwrap();
        write!(
            f,
            "TypeCheckingDiagnostics:\n{}",
            DisplayDiagnostics::new(&*db, &config, &self.diagnostics)
        )
    }
}

/// To display true debugs details about the TypeCheckingDiagnostics
#[derive(Debug)]
#[expect(dead_code)]
pub struct DebugTypeCheckingDiagnostics<'a> {
    diagnostics: &'a [Diagnostic],
    db: Arc<Mutex<MemoryDb>>,
    format: DiagnosticFormat,
    color: bool,
}

impl fmt::Display for TypeCheckingDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let db = self.db.lock().unwrap();
        DisplayDiagnostics::new(&*db, &self.config(), &self.diagnostics).fmt(f)
    }
}

impl TypeCheckingDiagnostics {
    fn new(diagnostics: Vec<Diagnostic>, db: MemoryDb) -> Self {
        Self {
            diagnostics,
            db: Arc::new(Mutex::new(db)),
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
            db: self.db.clone(),
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
