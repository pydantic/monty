use std::{fmt, sync::Arc};

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
    db::MemoryDb,
    pool::{PooledMemoryDb, SRC_ROOT, to_string},
};

/// All diagnostic formats supported by `TypeCheckingDiagnostics`.
const SUPPORTED_FORMATS: [DiagnosticFormat; 9] = [
    DiagnosticFormat::Full,
    DiagnosticFormat::Concise,
    DiagnosticFormat::Azure,
    DiagnosticFormat::Json,
    DiagnosticFormat::JsonLines,
    DiagnosticFormat::Rdjson,
    DiagnosticFormat::Pylint,
    DiagnosticFormat::Gitlab,
    DiagnosticFormat::Github,
];

/// Definition of a source file used by the Monty type-checking wrapper.
pub struct SourceFile<'a> {
    /// Python source code to type check.
    pub source_code: &'a str,
    /// User-visible file name used for diagnostics and module resolution.
    pub path: &'a str,
}

impl<'a> SourceFile<'a> {
    /// Create a new source file value.
    #[must_use]
    pub fn new(source_code: &'a str, path: &'a str) -> Self {
        Self { source_code, path }
    }
}

/// Pre-rendered diagnostic strings detached from the live Salsa database.
///
/// Every supported `(format, color)` pair is rendered while the database is still
/// alive, which lets pooled databases be scrubbed immediately after type checking.
struct RenderedDiagnostics {
    plain: [String; SUPPORTED_FORMATS.len()],
    color: [String; SUPPORTED_FORMATS.len()],
}

impl RenderedDiagnostics {
    /// Render all supported format/color combinations against the current db state.
    fn new(db: &MemoryDb, diagnostics: &[Diagnostic]) -> Self {
        Self {
            plain: SUPPORTED_FORMATS.map(|format| render_diagnostics(db, diagnostics, format, false)),
            color: SUPPORTED_FORMATS.map(|format| render_diagnostics(db, diagnostics, format, true)),
        }
    }

    /// Return the pre-rendered output for the requested format and color mode.
    fn get(&self, format: DiagnosticFormat, color: bool) -> &str {
        let index = diagnostic_format_index(format);
        if color { &self.color[index] } else { &self.plain[index] }
    }
}

/// Type check some Python source code, checking if it's valid to run with Monty.
///
/// Every call checks out one database from the process-wide pool, writes the
/// root-level source files into that db, renders any diagnostics eagerly, deletes
/// the temporary files again, and finally returns the scrubbed db to the pool.
///
/// # Arguments
/// * `python_source` - The Python source code to type check.
/// * `stubs_file` - Optional stubs file to import during type checking.
///
/// # Returns
/// * `Ok(Some(TypeCheckingDiagnostics))` - The code contains typing errors.
/// * `Ok(None)` - The code type-checks cleanly.
/// * `Err(String)` - An unexpected/internal error occurred while type checking.
pub fn type_check(
    python_source: &SourceFile<'_>,
    stubs_file: Option<&SourceFile<'_>>,
) -> Result<Option<TypeCheckingDiagnostics>, String> {
    let main_path = validate_root_file_name(python_source.path, "source")?;
    let stubs_path = stubs_file
        .map(|stubs_file| validate_root_file_name(stubs_file.path, "stub"))
        .transpose()?;

    if stubs_path.as_ref().is_some_and(|path| path == &main_path) {
        return Err(format!(
            "Type checking source and stubs must use different root file names: '{}'",
            python_source.path
        ));
    }

    let mut pooled_db = PooledMemoryDb::checkout()?;
    let result = type_check_with_db(
        &mut pooled_db,
        python_source,
        &main_path,
        stubs_file.zip(stubs_path.as_ref()),
    );
    pooled_db.finish(result)
}

/// Run one type-check operation against a checked-out pooled database.
///
/// The caller provides already-validated root file paths, while the pooled wrapper
/// owns the temporary-file bookkeeping needed for cleanup.
fn type_check_with_db(
    pooled_db: &mut PooledMemoryDb,
    python_source: &SourceFile<'_>,
    main_path: &SystemPathBuf,
    stubs_file: Option<(&SourceFile<'_>, &SystemPathBuf)>,
) -> Result<Option<TypeCheckingDiagnostics>, String> {
    let main_source = python_source.source_code;

    let (main_file, code_offset) = if let Some((stubs_file, stubs_path)) = stubs_file {
        pooled_db.write_root_file(stubs_path, stubs_file.source_code)?;

        // Import the stub module into the user's source so ty sees those definitions
        // while keeping user-visible spans anchored to the original file later.
        let stub_stem = module_stem(stubs_file.path);
        let mut new_source = format!("from {stub_stem} import *\n");
        let offset = u32::try_from(new_source.len()).map_err(to_string)?;
        new_source.push_str(main_source);

        let main_file = pooled_db.write_root_file(main_path, &new_source)?;
        (main_file, offset)
    } else {
        let main_file = pooled_db.write_root_file(main_path, main_source)?;
        (main_file, 0)
    };

    let mut diagnostics = check_types(pooled_db.db(), main_file);
    diagnostics.retain(filter_diagnostics);

    if diagnostics.is_empty() {
        return Ok(None);
    }

    // The stub import only exists to seed names into the semantic model. Restore the
    // original source text before rendering so detached diagnostics show user code.
    // Route via `rewrite_root_file` so the tracking invariant is checked at runtime
    // rather than relying on a hand-maintained comment.
    if code_offset > 0 {
        pooled_db.rewrite_root_file(main_path, main_source)?;
        let offset = TextSize::new(code_offset);
        for diagnostic in &mut diagnostics {
            for ann in diagnostic.annotations_mut() {
                adjust_annotation_span(ann, main_file, offset);
            }
            for sub in diagnostic.sub_diagnostics_mut() {
                for ann in sub.annotations_mut() {
                    adjust_annotation_span(ann, main_file, offset);
                }
            }
        }
    }

    let db = pooled_db.db();
    diagnostics.sort_by(|a, b| a.rendering_sort_key(db).cmp(&b.rendering_sort_key(db)));
    let rendered = RenderedDiagnostics::new(db, &diagnostics);

    Ok(Some(TypeCheckingDiagnostics::new(rendered)))
}

/// Validate that `path` names a single root-level file suitable for pooled reuse.
fn validate_root_file_name(path: &str, role: &str) -> Result<SystemPathBuf, String> {
    if path.is_empty() {
        return Err(format!("Type checking {role} file name cannot be empty"));
    }
    if path.contains('\0') {
        return Err(format!(
            "Type checking {role} file name must not contain NUL bytes, got '{}'",
            path.escape_debug()
        ));
    }
    if path == "." || path == ".." || path.contains('/') || path.contains('\\') {
        return Err(format!(
            "Type checking only supports root-level {role} file names, got '{path}'"
        ));
    }

    Ok(SystemPathBuf::from(SRC_ROOT).join(path))
}

/// Return the importable module stem for a root-level stub file name.
///
/// Uses the first `.` as the split so `foo.stubs.pyi` becomes `foo` (matches
/// Python's package-import semantics for root-level files — there is no
/// `foo.stubs` module on disk).
fn module_stem(file_name: &str) -> &str {
    file_name.split_once('.').map_or(file_name, |(before, _)| before)
}

/// Render diagnostics with a specific format/color pair while the database is alive.
fn render_diagnostics(db: &MemoryDb, diagnostics: &[Diagnostic], format: DiagnosticFormat, color: bool) -> String {
    let config = DisplayDiagnosticConfig::new("monty").format(format).color(color);
    DisplayDiagnostics::new(db, &config, diagnostics).to_string()
}

/// Convert a `DiagnosticFormat` to its slot in [`SUPPORTED_FORMATS`].
fn diagnostic_format_index(format: DiagnosticFormat) -> usize {
    match format {
        DiagnosticFormat::Full => 0,
        DiagnosticFormat::Concise => 1,
        DiagnosticFormat::Azure => 2,
        DiagnosticFormat::Json => 3,
        DiagnosticFormat::JsonLines => 4,
        DiagnosticFormat::Rdjson => 5,
        DiagnosticFormat::Pylint => 6,
        DiagnosticFormat::Gitlab => 7,
        DiagnosticFormat::Github => 8,
    }
}

/// Adjust the span of an annotation by subtracting the given offset.
///
/// This compensates for the injected `from <stub> import *` line so diagnostics still
/// point at the user's original source.
fn adjust_annotation_span(ann: &mut Annotation, main_file: File, offset: TextSize) {
    let span = ann.get_span();
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
/// The diagnostics are fully detached from the live database. Every supported output
/// format is rendered eagerly while the db is still checked out, which lets the db be
/// scrubbed and returned to the pool immediately afterwards.
#[derive(Clone)]
pub struct TypeCheckingDiagnostics {
    rendered: Arc<RenderedDiagnostics>,
    format: DiagnosticFormat,
    color: bool,
}

/// True debug details for detached type-checking diagnostics.
#[derive(Debug)]
#[expect(dead_code)]
pub struct DebugTypeCheckingDiagnostics<'a> {
    current_output: &'a str,
    format: DiagnosticFormat,
    color: bool,
}

impl fmt::Debug for TypeCheckingDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeCheckingDiagnostics:\n{self}")
    }
}

impl fmt::Display for TypeCheckingDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.rendered.get(self.format, self.color))
    }
}

impl TypeCheckingDiagnostics {
    /// Create detached diagnostics with the default full, non-colored output mode.
    fn new(rendered: RenderedDiagnostics) -> Self {
        Self {
            rendered: Arc::new(rendered),
            format: DiagnosticFormat::Full,
            color: false,
        }
    }

    /// Return details useful when debugging detached diagnostics behavior.
    #[must_use]
    pub fn debug_details(&self) -> DebugTypeCheckingDiagnostics<'_> {
        DebugTypeCheckingDiagnostics {
            current_output: self.rendered.get(self.format, self.color),
            format: self.format,
            color: self.color,
        }
    }

    /// Set the output format for later display.
    #[must_use]
    pub fn format(self, format: DiagnosticFormat) -> Self {
        Self { format, ..self }
    }

    /// Set the output format from a string accepted by the public bindings.
    pub fn format_from_str(self, format: &str) -> Result<Self, String> {
        let format = match format.to_ascii_lowercase().as_str() {
            "full" => DiagnosticFormat::Full,
            "concise" => DiagnosticFormat::Concise,
            "azure" => DiagnosticFormat::Azure,
            "json" => DiagnosticFormat::Json,
            "jsonlines" | "json-lines" => DiagnosticFormat::JsonLines,
            "rdjson" => DiagnosticFormat::Rdjson,
            "pylint" => DiagnosticFormat::Pylint,
            "gitlab" => DiagnosticFormat::Gitlab,
            "github" => DiagnosticFormat::Github,
            _ => return Err(format!("Unknown format: {format}")),
        };
        Ok(Self { format, ..self })
    }

    /// Set whether future displays should use the colored pre-rendered output.
    #[must_use]
    pub fn color(self, color: bool) -> Self {
        Self { color, ..self }
    }
}

/// Filter out diagnostics we intentionally ignore for now.
///
/// Should only be necessary until <https://github.com/astral-sh/ty/issues/2599> is fixed.
fn filter_diagnostics(d: &Diagnostic) -> bool {
    !(matches!(d.id(), DiagnosticId::InvalidSyntax)
        && matches!(
            d.primary_message(),
            "`await` statement outside of a function" | "`await` outside of an asynchronous function"
        ))
}
