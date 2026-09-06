//! Experimental Pyrefly type-checking backend.
//!
//! Pyrefly needs its module set declared up front, so a run gets exactly two:
//! the snippet and its stubs. Arbitrary module paths are not importable here.

use std::{fmt, marker::PhantomData};

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

impl fmt::Debug for PyreflyChecker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PyreflyChecker")
            .field("warm", &self.checker.is_some())
            .finish()
    }
}

impl PyreflyChecker {
    /// Type check `python_source`, with `stubs_file` (if any) importable from it.
    /// `Ok(None)` means it checks clean.
    pub fn run<'a>(
        &'a mut self,
        python_source: &SourceFile<'_>,
        stubs_file: Option<&SourceFile<'_>>,
        config: TypeCheckingConfig,
    ) -> Result<Option<PyreflyDiagnostics<'a>>, String> {
        let format = supported_format(config.format)?;

        // The injected import shifts reported lines down by one; `line_offset`
        // undoes that when rendering.
        let (main_source, line_offset) = match stubs_file {
            Some(_) => (format!("from {STUB_MODULE} import *\n{}", python_source.source_code), 1),
            None => (python_source.source_code.to_owned(), 0),
        };
        let stub_source = stubs_file.map_or("", |stubs| stubs.source_code);

        let diagnostics = self
            .checker()?
            .check(MAIN_MODULE, &[(STUB_MODULE, stub_source), (MAIN_MODULE, &main_source)]);

        let diagnostics: Vec<Diagnostic> = diagnostics
            .into_iter()
            .filter(|d| matches!(d.severity, Severity::Error | Severity::Warn))
            .collect();

        if diagnostics.is_empty() {
            Ok(None)
        } else {
            Ok(Some(PyreflyDiagnostics {
                diagnostics,
                path: python_source.path.to_owned(),
                line_offset,
                format,
                checker: PhantomData,
            }))
        }
    }

    /// Blank both modules so the checker can be reused for unrelated code.
    ///
    /// Security-critical: a checker reset and handed to another session without a
    /// run in between must not still hold the previous session's source. The warm
    /// typeshed is kept — it carries no session state.
    pub fn reset(&mut self) -> Result<(), String> {
        if let Some(checker) = &self.checker {
            checker.check(MAIN_MODULE, &[(STUB_MODULE, ""), (MAIN_MODULE, "")]);
        }
        Ok(())
    }

    fn checker(&mut self) -> Result<&Checker, String> {
        if self.checker.is_none() {
            self.checker = Some(Checker::new(Some(PYTHON_VERSION), &[MAIN_MODULE, STUB_MODULE])?);
        }
        Ok(self.checker.as_ref().expect("checker built above"))
    }
}

/// Pyrefly returns structured diagnostics rather than driving ruff's renderer, so
/// the machine-readable formats are rejected rather than silently downgraded.
/// `TypeCheckingConfig::color` is ignored throughout.
fn supported_format(format: TypeCheckingFormat) -> Result<TypeCheckingFormat, String> {
    match format {
        TypeCheckingFormat::Full | TypeCheckingFormat::Concise => Ok(format),
        other => Err(format!(
            "the pyrefly type-check backend renders only 'full' and 'concise', not '{other}'"
        )),
    }
}

/// The diagnostics of one failed Pyrefly check.
#[derive(Debug, Clone)]
pub struct PyreflyDiagnostics<'a> {
    diagnostics: Vec<Diagnostic>,
    /// The caller's path, reported instead of `__monty_main__`.
    path: String,
    line_offset: u32,
    format: TypeCheckingFormat,
    /// Borrows nothing; the lifetime only matches the ty backend's signature.
    checker: PhantomData<&'a ()>,
}

impl fmt::Display for PyreflyDiagnostics<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for diagnostic in &self.diagnostics {
            // Clamped at 1: the injected import line would underflow to 0.
            let line = diagnostic.start_line.saturating_sub(self.line_offset).max(1);
            writeln!(
                f,
                "{}:{}:{}: {}[{}] {}",
                self.path,
                line,
                diagnostic.start_col,
                severity_label(diagnostic.severity),
                diagnostic.kind,
                diagnostic.message,
            )?;
            if self.format == TypeCheckingFormat::Full && !diagnostic.details.is_empty() {
                writeln!(f, "{}", diagnostic.details)?;
            }
        }
        Ok(())
    }
}

/// `ty`-style label, so both backends' output reads the same way.
fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warn => "warning",
        // Filtered out by `run`; matched so a new severity is a compile error.
        Severity::Info | Severity::Ignore => "info",
    }
}
