//! Experimental Pyrefly type-checking backend.
//!
//! Pyrefly needs its module set declared up front, so a run gets exactly two:
//! the snippet and its stubs. Arbitrary module paths are not importable here.

use std::fmt;

use monty_types::{TypeCheckingConfig, TypeCheckingFormat};
use pyrefly::embed::{Checker, Diagnostic, Severity};

use crate::source_file::SourceFile;

const PYTHON_VERSION: &str = "3.14";
const MAIN_MODULE: &str = "__monty_main__";
const STUB_MODULE: &str = "__monty_stubs__";

/// A reusable Pyrefly checker. Built lazily: constructing one loads the typeshed.
#[derive(Default)]
pub struct PyreflyChecker {
    checker: Option<Checker>,
}

impl PyreflyChecker {
    /// Type check `python_source`, with `stubs_file` (if any) importable from it.
    /// `Ok(None)` means it checks clean.
    pub fn run(
        &mut self,
        python_source: &SourceFile<'_>,
        stubs_file: Option<&SourceFile<'_>>,
        config: TypeCheckingConfig,
    ) -> Result<Option<PyreflyDiagnostics>, String> {
        // Only the two renderers below are implemented; anything else would be
        // silently wrong for a host that parses it.
        if !matches!(config.format, TypeCheckingFormat::Full | TypeCheckingFormat::Concise) {
            return Err(format!("the pyrefly backend cannot render '{}'", config.format));
        }

        // The injected import shifts reported lines down by one; `line_offset`
        // undoes that when rendering.
        let (main_source, line_offset) = match stubs_file {
            Some(_) => (format!("from {STUB_MODULE} import *\n{}", python_source.source_code), 1),
            None => (python_source.source_code.to_owned(), 0),
        };
        let stub_source = stubs_file.map_or("", |stubs| stubs.source_code);

        if self.checker.is_none() {
            self.checker = Some(Checker::new(Some(PYTHON_VERSION), &[MAIN_MODULE, STUB_MODULE])?);
        }
        let Some(checker) = &self.checker else {
            return Err("pyrefly checker was not initialised".to_owned());
        };

        let diagnostics: Vec<Diagnostic> = checker
            .check(MAIN_MODULE, &[(STUB_MODULE, stub_source), (MAIN_MODULE, &main_source)])
            .into_iter()
            .filter(|d| matches!(d.severity, Severity::Error | Severity::Warn))
            .collect();

        Ok((!diagnostics.is_empty()).then(|| PyreflyDiagnostics {
            diagnostics,
            path: python_source.path.to_owned(),
            line_offset,
            full: config.format == TypeCheckingFormat::Full,
        }))
    }

    /// Blank both modules so the checker can be reused for unrelated code.
    ///
    /// Security-critical: a checker reset and handed to another session without a
    /// run in between must not still hold the previous session's source.
    pub fn reset(&mut self) -> Result<(), String> {
        if let Some(checker) = &self.checker {
            checker.check(MAIN_MODULE, &[(STUB_MODULE, ""), (MAIN_MODULE, "")]);
        }
        Ok(())
    }
}

/// The diagnostics of one failed Pyrefly check.
pub struct PyreflyDiagnostics {
    diagnostics: Vec<Diagnostic>,
    /// The caller's path, reported instead of `__monty_main__`.
    path: String,
    line_offset: u32,
    full: bool,
}

impl fmt::Display for PyreflyDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.diagnostics {
            // Clamped at 1: the injected import line would underflow to 0.
            let line = d.start_line.saturating_sub(self.line_offset).max(1);
            let severity = if d.severity == Severity::Error {
                "error"
            } else {
                "warning"
            };
            writeln!(
                f,
                "{}:{line}:{}: {severity}[{}] {}",
                self.path, d.start_col, d.kind, d.message
            )?;
            if self.full && !d.details.is_empty() {
                writeln!(f, "{}", d.details)?;
            }
        }
        Ok(())
    }
}
