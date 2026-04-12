use std::{
    fmt,
    sync::{Arc, Mutex, OnceLock},
};

use ruff_db::{
    Db as SourceDb,
    files::{File, FileRootKind, Files, system_path_to_file},
    system::{DbWithTestSystem, DbWithWritableSystem as _, System, SystemPathBuf, TestSystem},
    vendored::VendoredFileSystem,
};
use ruff_python_ast::PythonVersion;
use ty_module_resolver::{Db as ModuleResolverDb, SearchPathSettings, SearchPaths};
use ty_python_semantic::{
    AnalysisSettings, Db, Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource,
    default_lint_registry,
    lint::{LintRegistry, RuleSelection},
    types::check_types,
};

/// Virtual source root used for all in-memory type-checking files.
///
/// The reusable database pool only supports root-level user files, so every public
/// `SourceFile.path` is mapped directly under `/`.
pub(crate) const SRC_ROOT: &str = "/";

/// Warmup file used to populate stdlib/type-semantic caches in a fresh database.
///
/// The file is removed before the database is returned to the pool so pooled runs
/// always start from an empty visible filesystem.
const WARMUP_PATH: &str = "/__monty_warmup__.py";

/// Maximum number of warm `MemoryDb` instances kept in the process-wide pool.
///
/// The pool is intentionally small because every warm database retains its own
/// Salsa memo graph and typeshed-derived semantic state.
const MAX_POOLED_DBS: usize = 4;

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

impl MemoryDb {
    /// Create a fresh database with its own Salsa storage and Monty's fixed typing config.
    fn new() -> Self {
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

/// Pool of reusable warm databases for root-file type checking.
///
/// Each checked-out db is owned by exactly one caller until it is either returned
/// clean or dropped. This keeps Salsa's single-writer invariant intact while still
/// allowing concurrent type checks to use different warm databases.
struct MemoryDbPool {
    dbs: Mutex<Vec<MemoryDb>>,
}

impl MemoryDbPool {
    /// Access the process-wide database pool.
    fn global() -> &'static Self {
        static GLOBAL_POOL: OnceLock<MemoryDbPool> = OnceLock::new();
        GLOBAL_POOL.get_or_init(|| Self {
            dbs: Mutex::new(Vec::new()),
        })
    }

    /// Check out one warm database from the pool, creating a fresh warmed db if needed.
    fn checkout(&'static self) -> Result<PooledMemoryDb, String> {
        let maybe_db = {
            let mut dbs = self.dbs.lock().map_err(|e| e.to_string())?;
            dbs.pop()
        };

        Ok(PooledMemoryDb {
            db: Some(maybe_db.unwrap_or_else(build_warm_db)),
            pool: self,
        })
    }

    /// Return a fully scrubbed database to the pool if there is capacity left.
    fn release(&self, db: MemoryDb) -> Result<(), String> {
        let mut dbs = self.dbs.lock().map_err(|e| e.to_string())?;
        if dbs.len() < MAX_POOLED_DBS {
            dbs.push(db);
        }
        Ok(())
    }
}

/// Exclusive lease for one pooled database.
///
/// The caller must only return the lease with [`Self::return_clean`] after all user
/// files have been deleted and synced out of the in-memory filesystem. Dropping the
/// lease without returning it discards the db, which is the panic-safe fallback.
pub(crate) struct PooledMemoryDb {
    db: Option<MemoryDb>,
    pool: &'static MemoryDbPool,
}

impl PooledMemoryDb {
    /// Borrow the checked-out database mutably for one type-check run.
    pub(crate) fn db(&mut self) -> &mut MemoryDb {
        self.db
            .as_mut()
            .expect("pooled memory db accessed after being returned")
    }

    /// Return a scrubbed database to the pool.
    pub(crate) fn return_clean(mut self) -> Result<(), String> {
        let db = self.db.take().expect("pooled memory db returned more than once");
        self.pool.release(db)
    }
}

/// Check out a database from the global pool.
pub(crate) fn checkout_db() -> Result<PooledMemoryDb, String> {
    MemoryDbPool::global().checkout()
}

/// Build one warm database ready for pooled reuse.
///
/// The warmup initializes the `Program` singleton, resolves typeshed-backed search
/// paths, runs a trivial check that touches common builtin types, and then removes
/// the warmup file again so the returned db has no visible user files.
fn build_warm_db() -> MemoryDb {
    let mut db = MemoryDb::new();
    let src_root = SystemPathBuf::from(SRC_ROOT);
    db.files().try_add_root(&db, &src_root, FileRootKind::Project);

    let search_paths = SearchPathSettings::new(vec![src_root.clone()])
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

    // Prewarm the db with the stdlib stubs every real call tends to touch.
    let warmup_code = "\
x: int = 0
y: str = ''
z: float = 0.0
b: bool = False
xs: list[int] = []
d: dict[str, int] = {}
";
    let warmup = SystemPathBuf::from(WARMUP_PATH);
    db.write_file(&warmup, warmup_code).expect("warmup write");
    let warmup_file = system_path_to_file(&db, &warmup).expect("warmup file");
    let _ = check_types(&db, warmup_file);

    db.memory_file_system()
        .remove_file(&warmup)
        .expect("warmup file exists during warmup cleanup");
    warmup_file.sync(&mut db);
    File::sync_path(&mut db, &src_root);

    db
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
