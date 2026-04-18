use std::{fmt, sync::Arc};

use ruff_db::{
    Db as SourceDb,
    files::{File, FileRootKind, Files},
    system::{DbWithTestSystem, System, SystemPathBuf, TestSystem},
    vendored::VendoredFileSystem,
};
use ruff_python_ast::PythonVersion;
use ty_module_resolver::{Db as ModuleResolverDb, SearchPathSettings, SearchPaths};
use ty_python_semantic::{
    AnalysisSettings, Db, Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource,
    default_lint_registry,
    lint::{LintRegistry, RuleSelection},
};

/// Very simple in-memory salsa/ty database.
///
/// Mostly taken from
/// https://github.com/astral-sh/ruff/blob/7bacca9b625c2a658470afd99a0bf0aa0b4f1dbb/crates/ty_python_semantic/src/db.rs#L51
///
/// ## Lifetime invariant
///
/// Each `MemoryDb` owns a unique Salsa storage. It must never be cloned or shared
/// with another live handle because Salsa setters require exclusive access to the
/// underlying `Arc<Zalsa>`.
#[salsa::db]
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
        f.debug_struct("MemoryDb")
            .field("files", &self.files)
            .field("system", &self.system)
            .field("vendored", &self.vendored)
            .field("rule_selection", &self.rule_selection)
            .field("analysis_settings", &self.analysis_settings)
            .finish_non_exhaustive()
    }
}

/// Virtual source root used for all in-memory type-checking files.
///
/// Every public `SourceFile.path` is mapped under `/`, including nested paths such
/// as `pkg/main.py`. Pool cleanup is responsible for removing any intermediate
/// directories before a reused db is returned to the next caller.
pub(crate) const SRC_ROOT: &str = "/";

impl Default for MemoryDb {
    /// Create a fresh database wired up for type checking under `SRC_ROOT`.
    ///
    /// Registers `SRC_ROOT` as a Salsa-tracked project root and installs the
    /// `Program` settings needed by `check_types`. Returning a db without this
    /// setup would unwrap-panic the first time `check_types` is called, so this
    /// constructor is the only sanctioned way to build a `MemoryDb`.
    fn default() -> Self {
        let src_root = SystemPathBuf::from(SRC_ROOT);
        let db = Self {
            storage: salsa::Storage::new(None),
            system: TestSystem::default(),
            vendored: monty_typeshed::file_system().clone(),
            files: Files::default(),
            rule_selection: Arc::new(RuleSelection::from_registry(default_lint_registry())),
            analysis_settings: AnalysisSettings::default().into(),
        };

        db.files().try_add_root(&db, &src_root, FileRootKind::Project);

        let search_paths = SearchPathSettings::new(vec![src_root.to_path_buf()])
            .to_search_paths(db.system(), db.vendored())
            .expect("vendored typeshed search paths always resolve");

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

        db
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
