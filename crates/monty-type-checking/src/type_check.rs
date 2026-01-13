use std::fmt;

use ruff_db::{
    diagnostic::{Diagnostic, DiagnosticFormat, DisplayDiagnosticConfig, DisplayDiagnostics},
    files::system_path_to_file,
    system::DbWithWritableSystem as _,
    Db as SourceDb,
};
use ty_module_resolver::SearchPathSettings;
use ty_python_semantic::{
    types::check_types, Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource,
};

use crate::db::MemoryDb;

/// Type check some python source code, checking if it's valid to run with monty.
///
/// # Arguments
/// * `python_source` - The python source code to type check.
/// * `python_file_path` - The path for the python file used in the output, defaults to `main.py`
///
/// # Returns
/// * `Ok(Some(TypeCheckingFailure))` - If there are typing errors, returns a string with the error diagnostics.
/// * `Ok(None)` - If there are no typing errors.
/// * `Err(String)` - If there was an unexpected/internal error during type checking.
pub fn type_check(python_source: &str, python_file_path: Option<&str>) -> Result<Option<TypeCheckingFailure>, String> {
    let mut db = MemoryDb::new();

    Program::from_settings(
        &db,
        ProgramSettings {
            python_version: PythonVersionWithSource {
                version: db.python_version(),
                source: PythonVersionSource::default(),
            },
            python_platform: PythonPlatform::default(),
            search_paths: SearchPathSettings::new(vec![])
                .to_search_paths(db.system(), db.vendored())
                .map_err(|e| e.to_string())?,
        },
    );

    let path = python_file_path.unwrap_or("main.py");

    db.write_file(path, python_source).map_err(|e| e.to_string())?;
    let file = system_path_to_file(&db, path).map_err(|e| e.to_string())?;
    let diagnostics = check_types(&db, file);

    if diagnostics.is_empty() {
        Ok(None)
    } else {
        Ok(Some(TypeCheckingFailure::new(diagnostics, db)))
    }
}

/// Represents diagnostic details when type checking fails.
#[derive(Clone)]
pub struct TypeCheckingFailure {
    /// The actual diagnostic message
    diagnostics: Vec<Diagnostic>,
    /// db used to display diagnostics
    db: MemoryDb,
    /// How to format the output
    format: DiagnosticFormat,
    /// Whether to highlight the output with ansi colors
    color: bool,
}

impl fmt::Debug for TypeCheckingFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let config = self.config();
        let d = DisplayDiagnostics::new(&self.db, &config, &self.diagnostics);
        f.debug_struct("TypeCheckingFailure")
            .field("format", &self.format)
            .field("color", &self.color)
            .field("diagnostics", &d.to_string())
            .finish()
    }
}

impl fmt::Display for TypeCheckingFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        DisplayDiagnostics::new(&self.db, &self.config(), &self.diagnostics).fmt(f)
    }
}

impl TypeCheckingFailure {
    fn new(diagnostics: Vec<Diagnostic>, db: MemoryDb) -> Self {
        Self {
            diagnostics,
            db,
            format: DiagnosticFormat::Full,
            color: false,
        }
    }

    fn config(&self) -> DisplayDiagnosticConfig {
        DisplayDiagnosticConfig::default().format(self.format).color(self.color)
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
