use std::{
    fmt::Display,
    io::ErrorKind,
    sync::{Mutex, OnceLock},
};

use ruff_db::{
    Db as SourceDb,
    files::{File, FileRootKind, system_path_to_file},
    system::{DbWithTestSystem, DbWithWritableSystem as _, SystemPathBuf},
};
use ty_module_resolver::SearchPathSettings;
use ty_python_semantic::{Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource};

use crate::db::MemoryDb;

/// Virtual source root used for all in-memory type-checking files.
///
/// The reusable database pool only supports root-level user files, so every public
/// `SourceFile.path` is mapped directly under `/`.
pub(crate) const SRC_ROOT: &str = "/";

/// Maximum number of reusable `MemoryDb` instances kept in the process-wide pool.
///
/// The pool is intentionally small because every reused database retains its own
/// Salsa memo graph and typeshed-derived semantic state.
const MAX_POOLED_DBS: usize = 8;

/// File written into a pooled database during one type-check run.
///
/// Cleanup uses both the path and, when available, the interned `File` handle to
/// make sure Salsa observes the deletion before the db is returned to the pool.
struct TouchedRootFile {
    path: SystemPathBuf,
    file: Option<File>,
}

impl TouchedRootFile {
    /// Track a root file path that must be deleted before the db is reused.
    fn new(path: SystemPathBuf) -> Self {
        Self { path, file: None }
    }
}

/// Pool of reusable databases for root-file type checking.
///
/// Each checked-out db is owned by exactly one caller until it is either returned
/// clean or dropped. This keeps Salsa's single-writer invariant intact while still
/// allowing concurrent type checks to use different databases.
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

    /// Check out one pooled database, creating a fresh configured db if needed.
    fn checkout(&'static self) -> Result<PooledMemoryDb, String> {
        let maybe_db = {
            let mut dbs = self.dbs.lock().map_err(|e| e.to_string())?;
            dbs.pop()
        };

        Ok(PooledMemoryDb {
            db: Some(maybe_db.unwrap_or_else(build_pooled_db)),
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
struct PooledMemoryDb {
    db: Option<MemoryDb>,
    pool: &'static MemoryDbPool,
}

impl PooledMemoryDb {
    /// Borrow the checked-out database mutably for one type-check run.
    fn db(&mut self) -> &mut MemoryDb {
        self.db
            .as_mut()
            .expect("pooled memory db accessed after being returned")
    }

    /// Return a scrubbed database to the pool.
    fn return_clean(mut self) -> Result<(), String> {
        let db = self.db.take().expect("pooled memory db returned more than once");
        self.pool.release(db)
    }
}

/// Checked-out pooled database together with the files written during one run.
///
/// This wrapper lets `type_check.rs` write root files without carrying around its
/// own cleanup bookkeeping. Calling [`Self::finish`] performs the required scrub.
pub(crate) struct PooledTypeCheckDb {
    lease: PooledMemoryDb,
    touched_files: Vec<TouchedRootFile>,
}

impl PooledTypeCheckDb {
    /// Check out a database from the global pool for one type-check run.
    pub(crate) fn checkout() -> Result<Self, String> {
        Ok(Self {
            lease: MemoryDbPool::global().checkout()?,
            touched_files: Vec::new(),
        })
    }

    /// Borrow the checked-out database mutably.
    pub(crate) fn db(&mut self) -> &mut MemoryDb {
        self.lease.db()
    }

    /// Write one root file into the db and remember it for mandatory cleanup.
    pub(crate) fn write_root_file(&mut self, path: &SystemPathBuf, source: &str) -> Result<File, String> {
        self.lease.db().write_file(path, source).map_err(to_string)?;
        self.touched_files.push(TouchedRootFile::new(path.clone()));

        let file = system_path_to_file(self.lease.db(), path).map_err(to_string)?;
        self.touched_files
            .last_mut()
            .expect("newly pushed touched file must exist")
            .file = Some(file);

        Ok(file)
    }

    /// Scrub the run's temporary files, optionally return the db to the pool, and
    /// then forward the caller's original result.
    pub(crate) fn finish<T>(self, result: Result<T, String>) -> Result<T, String> {
        let Self {
            mut lease,
            touched_files,
        } = self;

        let cleanup = cleanup_touched_files(lease.db(), &touched_files);
        if cleanup.is_ok() {
            lease.return_clean()?;
        }

        combine_run_result(result, cleanup)
    }
}

/// Build one fresh database ready to enter the pool.
///
/// This sets up the source root and `Program` settings, but it intentionally does
/// not run a synthetic type-check. The first real caller that uses a brand new db
/// pays the cold-start cost; subsequent pooled reuses benefit from the populated
/// Salsa caches created by real user work.
fn build_pooled_db() -> MemoryDb {
    let db = MemoryDb::new();
    let src_root = SystemPathBuf::from(SRC_ROOT);
    db.files().try_add_root(&db, &src_root, FileRootKind::Project);

    let search_paths = SearchPathSettings::new(vec![src_root])
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

/// Remove all files written during a type-check run and sync the filesystem changes.
///
/// Cleanup runs in reverse write order and always syncs `/` once at the end so root
/// directory listings cannot leak between pooled sessions.
fn cleanup_touched_files(db: &mut MemoryDb, touched_files: &[TouchedRootFile]) -> Result<(), String> {
    for touched in touched_files.iter().rev() {
        match db.memory_file_system().remove_file(&touched.path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "Failed to remove pooled type-check file '{}': {err}",
                    touched.path
                ));
            }
        }

        if let Some(file) = touched.file {
            file.sync(db);
        } else {
            File::sync_path(db, &touched.path);
        }
    }

    File::sync_path(db, &SystemPathBuf::from(SRC_ROOT));
    Ok(())
}

/// Merge the caller's result with the cleanup result without hiding either error.
fn combine_run_result<T>(run_result: Result<T, String>, cleanup_result: Result<(), String>) -> Result<T, String> {
    match (run_result, cleanup_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(cleanup_err)) => Err(cleanup_err),
        (Err(run_err), Ok(())) => Err(run_err),
        (Err(run_err), Err(cleanup_err)) => Err(format!("{run_err}\ncleanup error: {cleanup_err}")),
    }
}

/// Convert a displayable error into the string type used throughout pooling logic.
fn to_string(err: impl Display) -> String {
    err.to_string()
}
