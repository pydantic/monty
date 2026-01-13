use ruff_db::{
    diagnostic::{DiagnosticFormat, DisplayDiagnosticConfig, DisplayDiagnostics},
    files::system_path_to_file,
    system::DbWithWritableSystem as _,
    Db as SourceDb,
};
use ty_module_resolver::SearchPathSettings;
use ty_python_semantic::{
    types::check_types, Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource,
};

use crate::db::MemoryDb;

#[derive(Debug, Default)]
pub struct TypeCheckingConfig {
    /// How to format the output
    pub format: DiagnosticFormat,
    /// Whether to highlight the output with ansi colors
    pub color: bool,
    /// Path for the python file used in the output, defaults to `main.py`
    pub python_file_path: Option<String>,
}

impl TypeCheckingConfig {
    #[must_use]
    pub fn format(self, format: DiagnosticFormat) -> Self {
        Self { format, ..self }
    }

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

    #[must_use]
    pub fn color(self, color: bool) -> Self {
        Self { color, ..self }
    }

    #[must_use]
    pub fn python_file_path(self, python_file_path: Option<String>) -> Self {
        Self {
            python_file_path,
            ..self
        }
    }
}

/// Type check some python source code, checking if it's valid to run with monty.
///
/// # Arguments
/// * `python_source` - The python source code to type check.
/// * `config` - The configuration for type checking.
///
/// # Returns
/// * `Ok(Some(String))` - If there are typing errors, returns a string with the error diagnostics.
/// * `Ok(None)` - If there are no typing errors.
/// * `Err(String)` - If there was an unexpected/internal error during type checking.
pub fn type_check(python_source: &str, config: Option<TypeCheckingConfig>) -> Result<Option<String>, String> {
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

    let config = config.unwrap_or_default();
    let path = config.python_file_path.as_deref().unwrap_or("main.py");

    db.write_file(path, python_source).map_err(|e| e.to_string())?;
    let file = system_path_to_file(&db, path).map_err(|e| e.to_string())?;
    let diagnostics = check_types(&db, file);

    if diagnostics.is_empty() {
        Ok(None)
    } else {
        let display_config = DisplayDiagnosticConfig::default()
            .format(config.format)
            .color(config.color);

        let s = DisplayDiagnostics::new(&db, &display_config, &diagnostics).to_string();
        Ok(Some(s))
    }
}
