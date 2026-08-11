use std::{fmt, io::ErrorKind, mem};

use monty_types::{TypeCheckingConfig, TypeCheckingFormat};
use ruff_db::{
    diagnostic::{
        Annotation, Diagnostic, DiagnosticFormat, DiagnosticId, DisplayDiagnosticConfig, DisplayDiagnostics,
        UnifiedFile,
    },
    files::{File, system_path_to_file},
    system::{DbWithTestSystem, DbWithWritableSystem as _, SystemPathBuf},
};
use ruff_text_size::{TextRange, TextSize};
use ty_python_semantic::check_file_unwrap;

use crate::db::{MemoryDb, SRC_ROOT};

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

#[derive(Debug, Default)]
pub struct TypeChecker {
    db: MemoryDb,
    touched_files: Vec<TouchedRootFile>,
}

impl TypeChecker {
    /// Type check some python source code, checking if it's valid to run with monty.
    ///
    /// # Arguments
    /// * `python_source` - The python source code to type check.
    /// * `stubs_file` - Optional stubs file to use for type checking.
    /// * `config` - Configuration for the type checking diagnostics.
    ///
    /// # Returns
    /// * `Ok(None)` - If there are no typing errors.
    /// * `Ok(Some(string))` - If there are typing errors.
    /// * `Err(String)` - If there was an unexpected/internal error during type checking.
    pub fn run<'a>(
        &'a mut self,
        python_source: &SourceFile<'_>,
        stubs_file: Option<&SourceFile<'_>>,
        config: TypeCheckingConfig,
    ) -> Result<Option<TypeCheckingDiagnostics<'a>>, String> {
        let src_root = SystemPathBuf::from(SRC_ROOT);
        let main_path = src_root.join(python_source.path);
        let main_source = python_source.source_code;

        let (main_file, code_offset): (File, u32) = if let Some(stubs_file) = stubs_file {
            let stubs_path = src_root.join(stubs_file.path);
            self.write_root_file(&stubs_path, stubs_file.source_code)?;

            // prepend the stub import to the main source code
            let stub_stem = stubs_file
                .path
                .split_once('.')
                .map_or(stubs_file.path, |(before, _)| before);
            let mut new_source = format!("from {stub_stem} import *\n");
            let offset = u32::try_from(new_source.len()).map_err(to_string)?;
            new_source.push_str(main_source);

            let main_file = self.write_root_file(&main_path, &new_source)?;
            // one line offset for errors vs. the original source code since we injected the stub import
            (main_file, offset)
        } else {
            let main_file = self.write_root_file(&main_path, main_source)?;
            (main_file, 0)
        };

        // Use `check_file_unwrap` (not `check_types` alone) so that parser errors
        // and unsupported-syntax errors are included — otherwise malformed input
        // (e.g. deeply nested parentheses that ruff's parser rejects) would silently
        // type-check clean.
        let mut diagnostics = check_file_unwrap(&self.db, main_file);
        diagnostics.retain(filter_diagnostics);

        if diagnostics.is_empty() {
            Ok(None)
        } else {
            // without all this errors would appear on the wrong line because we injected `from type_stubs import *`

            // if we injected the stubs import, we need to write the actual source back to the file in the database
            self.rewrite_root_file(&main_path, main_source)?;
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
            diagnostics.sort_by(|a, b| a.rendering_sort_key(&self.db).cmp(&b.rendering_sort_key(&self.db)));

            Ok(Some(TypeCheckingDiagnostics {
                diagnostics,
                type_checker: self,
                config,
            }))
        }
    }

    /// Write one root file into the db and remember it for mandatory cleanup.
    ///
    /// A path already tracked from an earlier `run` is overwritten rather than
    /// tracked twice: a session rewrites the same script path on every feed,
    /// so the tracking list must not grow with the number of feeds.
    fn write_root_file(&mut self, path: &SystemPathBuf, source: &str) -> Result<File, String> {
        self.db.write_file(path, source).map_err(to_string)?;

        // The write above succeeded, so interning the path must succeed — otherwise the
        // file would live in the db but be untracked, poisoning cleanup, hence the panic.
        let file = system_path_to_file(&self.db, path)
            .unwrap_or_else(|e| panic!("interning a just-written root file must succeed, DB in an unsafe state: {e}"));

        if !self.touched_files.iter().any(|t| &t.path == path) {
            self.touched_files.push(TouchedRootFile::new(path.clone(), file));
        }
        Ok(file)
    }

    /// Overwrite the contents of a root file that was previously written via
    /// [`Self::write_root_file`].
    ///
    /// Panics if `path` is not already tracked — the caller would otherwise be
    /// leaving an untracked write behind that cleanup would miss, so a later
    /// [`Self::reset`] would leave the file visible to the next session.
    fn rewrite_root_file(&mut self, path: &SystemPathBuf, source: &str) -> Result<(), String> {
        assert!(
            self.touched_files.iter().any(|t| &t.path == path),
            "rewrite_root_file called for untracked path '{path}' — must call write_root_file first",
        );
        self.db.write_file(path, source).map_err(to_string)
    }

    /// Remove every file written since the last reset and sync the filesystem
    /// changes, so the checker can be reused for unrelated code.
    ///
    /// Security-critical: without this a recycled worker would let one
    /// session's modules and stubs resolve while checking the next session's
    /// code. Each `TouchedRootFile::cleanup` removes its own file and walks its
    /// ancestor chain up to `SRC_ROOT`, removing any directory that has become
    /// empty. Shared parent directories collapse naturally once the last file
    /// inside them is gone. We sync `SRC_ROOT` once at the end so the next
    /// session cannot observe the previous root directory listing.
    pub fn reset(&mut self) -> Result<(), String> {
        let touched_files = mem::take(&mut self.touched_files);

        for touched in touched_files.iter().rev() {
            touched.cleanup(&mut self.db)?;
        }

        File::sync_path(&mut self.db, &SystemPathBuf::from(SRC_ROOT));
        Ok(())
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

/// The diagnostics of one failed type check, rendered on `Display`.
///
/// Borrows the checker because ty's diagnostics only resolve their spans (file
/// paths, source snippets) against the database that produced them — which is
/// why the format is chosen up front, in [`TypeChecker::run`], and why callers
/// that outlive the checker (anything across a process boundary) must keep the
/// rendered string rather than this.
pub struct TypeCheckingDiagnostics<'a> {
    /// The actual diagnostic message
    diagnostics: Vec<Diagnostic>,
    /// The type checker, the db is needed to display diagnostics.
    type_checker: &'a TypeChecker,
    /// How to format the output and whether to use color.
    config: TypeCheckingConfig,
}

impl fmt::Display for TypeCheckingDiagnostics<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let format = match self.config.format {
            TypeCheckingFormat::Full => DiagnosticFormat::Full,
            TypeCheckingFormat::Concise => DiagnosticFormat::Concise,
            TypeCheckingFormat::Azure => DiagnosticFormat::Azure,
            TypeCheckingFormat::Json => DiagnosticFormat::Json,
            TypeCheckingFormat::JsonLines => DiagnosticFormat::JsonLines,
            TypeCheckingFormat::Rdjson => DiagnosticFormat::Rdjson,
            TypeCheckingFormat::Pylint => DiagnosticFormat::Pylint,
            TypeCheckingFormat::Gitlab => DiagnosticFormat::Gitlab,
            TypeCheckingFormat::Github => DiagnosticFormat::Github,
        };

        let config = DisplayDiagnosticConfig::new("monty")
            .format(format)
            .color(self.config.color);
        DisplayDiagnostics::new(&self.type_checker.db, &config, &self.diagnostics).fmt(f)
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

/// File written into a pooled database during one type-check run.
///
/// The path is used to remove the file from the in-memory filesystem, and the
/// interned `File` handle is then synced so Salsa observes the deletion before
/// the db is returned to the pool.
#[derive(Debug)]
struct TouchedRootFile {
    path: SystemPathBuf,
    file: File,
}

impl TouchedRootFile {
    /// Track a root file and its interned handle for mandatory cleanup.
    fn new(path: SystemPathBuf, file: File) -> Self {
        Self { path, file }
    }

    /// Remove the file from the in-memory filesystem, then walk its ancestor
    /// chain and remove every directory under `SRC_ROOT` that has become empty.
    ///
    /// `remove_directory` requires the directory to be empty, so if two touched
    /// files live in the same directory the first cleanup hits
    /// `DirectoryNotEmpty` (silently swallowed) and the second succeeds once its
    /// file is gone. This gives us correct cleanup without needing to sort paths
    /// or coordinate across `TouchedRootFile`s.
    fn cleanup(&self, db: &mut MemoryDb) -> Result<(), String> {
        match db.memory_file_system().remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "Failed to remove pooled type-check file '{}': {err}",
                    self.path
                ));
            }
        }
        self.file.sync(db);

        // Walk parents up to but not including SRC_ROOT, removing each empty directory
        // and syncing its path so Salsa invalidates any cached directory listing.
        let src_root = SystemPathBuf::from(SRC_ROOT);
        let mut ancestor = self.path.parent();
        while let Some(dir) = ancestor
            && dir != src_root.as_path()
        {
            match db.memory_file_system().remove_directory(dir) {
                Ok(()) => {}
                // Another touched file still lives in this directory; it will be
                // removed by a later `cleanup` call. Every ancestor above this
                // one is necessarily also non-empty (they contain this directory),
                // so there is no point walking further up.
                //
                // `MemoryFileSystem::remove_directory` reports "directory not
                // empty" as `io::Error::other(...)` (kind `Other`), so we match on
                // the message rather than on `ErrorKind::DirectoryNotEmpty`.
                Err(err) if err.to_string().contains("directory not empty") => break,
                // `NotFound` at this point would mean the directory never existed
                // or was already removed, both of which indicate a logic bug
                // (e.g. the same path tracked twice) — fail loudly.
                Err(err) => {
                    return Err(format!("Failed to remove pooled type-check directory '{dir}': {err}"));
                }
            }
            File::sync_path(db, dir);
            ancestor = dir.parent();
        }
        Ok(())
    }
}

/// Convert a displayable error into the string type used throughout type checking.
pub(crate) fn to_string(err: impl fmt::Display) -> String {
    err.to_string()
}
