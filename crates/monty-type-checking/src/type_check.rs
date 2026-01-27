use std::{
    fmt::{self, Display},
    sync::{Arc, Mutex},
};

use ruff_db::{
    Db as SourceDb,
    diagnostic::{Diagnostic, DiagnosticFormat, DiagnosticId, DisplayDiagnosticConfig, DisplayDiagnostics},
    files::system_path_to_file,
    system::DbWithWritableSystem as _,
};
use ty_module_resolver::SearchPathSettings;
use ty_python_semantic::{
    Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource, types::check_types,
};

use crate::db::MemoryDb;

/// Type check some python source code, checking if it's valid to run with monty.
///
/// # Arguments
/// * `python_source` - The python source code to type check.
/// * `python_file_path` - The path for the python file used in the output.
///
/// # Returns
/// * `Ok(Some(TypeCheckingFailure))` - If there are typing errors.
/// * `Ok(None)` - If there are no typing errors.
/// * `Err(String)` - If there was an unexpected/internal error during type checking.
pub fn type_check(python_source: &str, python_file_path: &str) -> Result<Option<TypeCheckingDiagnostics>, String> {
    let mut db = MemoryDb::new();

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
            search_paths: SearchPathSettings::new(vec![])
                .to_search_paths(db.system(), db.vendored())
                .map_err(to_string)?,
        },
    );

    db.write_file(python_file_path, python_source).map_err(to_string)?;
    let file = system_path_to_file(&db, python_file_path).map_err(to_string)?;
    let mut diagnostics = check_types(&db, file);
    diagnostics.retain(filter_diagnostics);

    if diagnostics.is_empty() {
        Ok(None)
    } else {
        Ok(Some(TypeCheckingDiagnostics::new(diagnostics, db)))
    }
}

fn to_string(err: impl Display) -> String {
    err.to_string()
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

impl fmt::Display for TypeCheckingDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let db = self.db.lock().unwrap();
        DisplayDiagnostics::new(&*db, &self.config(), &self.diagnostics).fmt(f)
    }
}

impl TypeCheckingDiagnostics {
    fn new(mut diagnostics: Vec<Diagnostic>, db: MemoryDb) -> Self {
        // Sort diagnostics by line number
        diagnostics.sort_by(|a, b| a.rendering_sort_key(&db).cmp(&b.rendering_sort_key(&db)));
        Self {
            diagnostics,
            db: Arc::new(Mutex::new(db)),
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
