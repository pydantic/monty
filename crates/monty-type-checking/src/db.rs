use std::{fmt, sync::Arc};

use ruff_db::{
    Db as SourceDb,
    files::{File, Files},
    system::{DbWithTestSystem, System, TestSystem},
    vendored::VendoredFileSystem,
};
use ruff_python_ast::PythonVersion;
use ty_module_resolver::{Db as ModuleResolverDb, SearchPaths};
use ty_python_semantic::{
    AnalysisSettings, Db, Program, default_lint_registry,
    lint::{LintRegistry, RuleSelection},
};

/// Very simple in-memory salsa/ty database.
///
/// Mostly taken from
/// https://github.com/astral-sh/ruff/blob/7bacca9b625c2a658470afd99a0bf0aa0b4f1dbb/crates/ty_python_semantic/src/db.rs#L51
#[salsa::db]
#[derive(Clone)]
pub(crate) struct MemoryDb {
    storage: salsa::Storage<Self>,
    files: Files,
    system: TestSystem,
    vendored: VendoredFileSystem,
    rule_selection: Arc<RuleSelection>,
    analysis_settings: Arc<AnalysisSettings>,
}

impl fmt::Debug for MemoryDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypeCheckingFailure")
            .field("files", &self.files)
            .field("system", &self.system)
            .field("vendored", &self.vendored)
            .field("rule_selection", &self.rule_selection)
            .field("analysis_settings", &self.analysis_settings)
            .finish_non_exhaustive()
    }
}

impl MemoryDb {
    pub fn new() -> Self {
        Self {
            storage: salsa::Storage::new(None),
            system: TestSystem::default(),
            vendored: monty_typeshed::file_system().clone(),
            files: Files::default(),
            rule_selection: Arc::new(RuleSelection::from_registry(default_lint_registry())),
            analysis_settings: AnalysisSettings::default().into(),
        }
    }
}

impl DbWithTestSystem for MemoryDb {
    fn test_system(&self) -> &TestSystem {
        &self.system
    }

    fn test_system_mut(&mut self) -> &mut TestSystem {
        &mut self.system
    }
}

#[salsa::db]
impl SourceDb for MemoryDb {
    fn vendored(&self) -> &VendoredFileSystem {
        &self.vendored
    }

    fn system(&self) -> &dyn System {
        &self.system
    }

    fn files(&self) -> &Files {
        &self.files
    }

    fn python_version(&self) -> PythonVersion {
        PythonVersion::PY314
    }
}

#[salsa::db]
impl Db for MemoryDb {
    fn should_check_file(&self, file: File) -> bool {
        !file.path(self).is_vendored_path()
    }

    fn rule_selection(&self, _file: File) -> &RuleSelection {
        &self.rule_selection
    }

    fn lint_registry(&self) -> &LintRegistry {
        default_lint_registry()
    }

    fn analysis_settings(&self, _file: File) -> &AnalysisSettings {
        &self.analysis_settings
    }

    fn verbose(&self) -> bool {
        false
    }
}

#[salsa::db]
impl ModuleResolverDb for MemoryDb {
    fn search_paths(&self) -> &SearchPaths {
        Program::get(self).search_paths(self)
    }
}

#[salsa::db]
impl salsa::Database for MemoryDb {}
