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
            let mut dbs = self.dbs.lock().map_err(to_string)?;
            dbs.pop()
        };

        Ok(PooledMemoryDb {
            db: maybe_db.unwrap_or_else(build_pooled_db),
            pool: self,
            touched_files: Vec::new(),
        })
    }

    /// Return a fully scrubbed database to the pool if there is capacity left.
    fn release(&self, db: MemoryDb) -> Result<(), String> {
        let mut dbs = self.dbs.lock().map_err(to_string)?;
        if dbs.len() < MAX_POOLED_DBS {
            dbs.push(db);
        }
        Ok(())
    }
}

/// Exclusive lease for one pooled database.
///
/// The caller must return the lease with [`Self::finish`], which scrubs the files
/// written during the run before releasing the db back to the pool. Dropping the
/// lease without calling `finish` discards the db, which is the panic-safe fallback.
pub(crate) struct PooledMemoryDb {
    db: MemoryDb,
    pool: &'static MemoryDbPool,
    touched_files: Vec<TouchedRootFile>,
}

impl PooledMemoryDb {
    /// Check out a database from the global pool for one type-check run.
    pub(crate) fn checkout() -> Result<Self, String> {
        MemoryDbPool::global().checkout()
    }

    /// Write one root file into the db and remember it for mandatory cleanup.
    pub(crate) fn write_root_file(&mut self, path: &SystemPathBuf, source: &str) -> Result<File, String> {
        self.db().write_file(path, source).map_err(to_string)?;

        // The write above succeeded, so interning the path must succeed — otherwise the
        // file would live in the db but be untracked, poisoning the pool on release, hence the panic.
        let file = match system_path_to_file(self.db(), path) {
            Ok(file) => file,
            Err(e) => panic!("interning a just-written root file must succeed, DB in an unsafe state: {e}"),
        };

        self.touched_files.push(TouchedRootFile::new(path.clone(), file));
        Ok(file)
    }

    /// Overwrite the contents of a root file that was previously written via
    /// [`Self::write_root_file`].
    ///
    /// Panics if `path` is not already tracked — the caller would otherwise be
    /// leaving an untracked write behind that cleanup would miss, poisoning the
    /// pool on release.
    pub(crate) fn rewrite_root_file(&mut self, path: &SystemPathBuf, source: &str) -> Result<(), String> {
        assert!(
            self.touched_files.iter().any(|t| &t.path == path),
            "rewrite_root_file called for untracked path '{path}' — must call write_root_file first",
        );
        self.db().write_file(path, source).map_err(to_string)
    }

    /// Borrow the checked-out database mutably for one type-check run.
    pub(crate) fn db(&mut self) -> &mut MemoryDb {
        &mut self.db
    }

    /// Scrub the run's temporary files, optionally return the db to the pool, and
    /// then forward the caller's original result.
    pub(crate) fn finish<T>(self, run_result: Result<T, String>) -> Result<T, String> {
        let Self {
            mut db,
            pool,
            touched_files,
        } = self;

        let cleanup_result = cleanup_touched_files(&mut db, &touched_files);
        if cleanup_result.is_ok() {
            pool.release(db)?;
        }

        match (run_result, cleanup_result) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(cleanup_err)) => Err(cleanup_err),
            (Err(run_err), Ok(())) => Err(run_err),
            (Err(run_err), Err(cleanup_err)) => Err(format!("{run_err}\ncleanup error: {cleanup_err}")),
        }
    }
}

/// Build one fresh database ready to enter the pool.
///
/// This sets up the source root and `Program` settings, but it intentionally does
/// not run a synthetic type-check. The first real caller that uses a brand new db
/// pays the cold-start cost; subsequent pooled reuses benefit from the populated
/// Salsa caches created by real user work.
fn build_pooled_db() -> MemoryDb {
    let db = MemoryDb::default();
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

/// File written into a pooled database during one type-check run.
///
/// The path is used to remove the file from the in-memory filesystem, and the
/// interned `File` handle is then synced so Salsa observes the deletion before
/// the db is returned to the pool.
struct TouchedRootFile {
    path: SystemPathBuf,
    file: File,
}

impl TouchedRootFile {
    /// Track a root file and its interned handle for mandatory cleanup.
    fn new(path: SystemPathBuf, file: File) -> Self {
        Self { path, file }
    }

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
        Ok(())
    }
}

/// Remove all files written during a type-check run and sync the filesystem changes.
///
/// Cleanup runs in reverse write order and always syncs `/` once at the end so root
/// directory listings cannot leak between pooled sessions.
fn cleanup_touched_files(db: &mut MemoryDb, touched_files: &[TouchedRootFile]) -> Result<(), String> {
    for touched in touched_files.iter().rev() {
        touched.cleanup(db)?;
    }

    File::sync_path(db, &SystemPathBuf::from(SRC_ROOT));
    Ok(())
}

/// Convert a displayable error into the string type used throughout type checking.
pub(crate) fn to_string(err: impl Display) -> String {
    err.to_string()
}

#[cfg(test)]
mod tests {
    use std::{ptr, sync::Mutex};

    use ruff_db::files::FileError;

    use super::*;

    /// Serializes tests that manipulate the process-wide pool so they observe a
    /// deterministic pool state rather than racing with each other.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pool_is_global_singleton() {
        assert!(
            ptr::eq(MemoryDbPool::global(), MemoryDbPool::global()),
            "global pool must resolve to the same instance on every call",
        );
    }

    #[test]
    fn reused_db_does_not_leak_previous_files() {
        let _guard = TEST_LOCK.lock().unwrap();
        drain_pool();

        let path = SystemPathBuf::from("/pool_test_reuse.py");

        let mut pooled = PooledMemoryDb::checkout().expect("initial checkout");
        pooled.write_root_file(&path, "x = 1\n").expect("write root file");
        assert!(
            system_path_to_file(pooled.db(), &path).is_ok(),
            "file should be visible within the run that wrote it",
        );
        pooled.finish::<()>(Ok(())).expect("finish first run");

        assert_eq!(pool_len(), 1, "scrubbed db should be released back to the pool");

        // Second checkout pops the only entry, so we are guaranteed the same db.
        let mut pooled = PooledMemoryDb::checkout().expect("re-checkout");
        assert_eq!(pool_len(), 0, "pool should be empty after re-checkout");
        assert!(
            matches!(system_path_to_file(pooled.db(), &path), Err(FileError::NotFound)),
            "previous run's file must not be visible in the reused db",
        );
        pooled.finish::<()>(Ok(())).expect("finish second run");

        drain_pool();
    }

    fn pool_len() -> usize {
        MemoryDbPool::global().dbs.lock().unwrap().len()
    }

    fn drain_pool() {
        MemoryDbPool::global().dbs.lock().unwrap().clear();
    }
}
