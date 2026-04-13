use std::{
    fmt::{self, Display},
    io::ErrorKind,
    mem,
    sync::{Mutex, OnceLock},
};

use ruff_db::{
    files::{File, system_path_to_file},
    system::{DbWithTestSystem, DbWithWritableSystem as _, SystemPathBuf},
};

use crate::db::{MemoryDb, SRC_ROOT};

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
            db: Some(maybe_db.unwrap_or_default()),
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
/// On [`Drop`] the lease scrubs every file that was written during its lifetime
/// and, if cleanup succeeded, returns the database to the global pool. A db whose
/// cleanup fails is intentionally discarded — a contaminated db must never re-enter
/// the pool because it would poison every subsequent type-check.
///
/// This RAII pattern lets `TypeCheckingDiagnostics` keep the lease alive across the
/// entire lifetime of a returned diagnostics object (so lazy rendering still works
/// against the live db) and the db is released exactly when the diagnostics are no
/// longer reachable.
pub(crate) struct PooledMemoryDb {
    /// `Option` so `Drop` can move the db out into the pool without leaving an
    /// empty placeholder `MemoryDb` behind. Always `Some` while the lease is
    /// observable to the rest of the codebase.
    db: Option<MemoryDb>,
    pool: &'static MemoryDbPool,
    touched_files: Vec<TouchedRootFile>,
}

impl fmt::Debug for PooledMemoryDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledMemoryDb")
            .field("touched_files", &self.touched_files.len())
            .finish_non_exhaustive()
    }
}

impl PooledMemoryDb {
    /// Check out a database from the global pool for one type-check run.
    pub(crate) fn checkout() -> Result<Self, String> {
        MemoryDbPool::global().checkout()
    }

    /// Write one root file into the db and remember it for mandatory cleanup.
    pub(crate) fn write_root_file(&mut self, path: &SystemPathBuf, source: &str) -> Result<File, String> {
        let db = self.db_mut();
        db.write_file(path, source).map_err(to_string)?;

        // The write above succeeded, so interning the path must succeed — otherwise the
        // file would live in the db but be untracked, poisoning the pool on release, hence the panic.
        let file = system_path_to_file(db, path)
            .unwrap_or_else(|e| panic!("interning a just-written root file must succeed, DB in an unsafe state: {e}"));

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
        self.db_mut().write_file(path, source).map_err(to_string)
    }

    /// Borrow the checked-out database (e.g. for `check_types` or rendering).
    pub(crate) fn db(&self) -> &MemoryDb {
        self.db.as_ref().expect("db is only None inside Drop")
    }

    fn db_mut(&mut self) -> &mut MemoryDb {
        self.db.as_mut().expect("db is only None inside Drop")
    }
}

impl Drop for PooledMemoryDb {
    fn drop(&mut self) {
        // `Drop` only gives us `&mut self`. Take the db out of the `Option` so we own
        // it for cleanup and release, leaving `None` behind for the implicit drop of
        // `self`'s remaining fields.
        let Some(mut db) = self.db.take() else { return };
        let touched_files = mem::take(&mut self.touched_files);

        // Cleanup or release failures leave the global pool in a state we cannot
        // reason about, so we panic loudly. Drop-time panics will abort if they
        // happen during unwinding from another panic, which is the desired behavior:
        // a poisoned pool is worse than aborting the process.
        cleanup_touched_files(&mut db, &touched_files)
            .unwrap_or_else(|err| panic!("monty type-check pool: failed to scrub db on drop: {err}"));
        self.pool
            .release(db)
            .unwrap_or_else(|err| panic!("monty type-check pool: failed to release db on drop: {err}"));
    }
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

/// Remove all files written during a type-check run and sync the filesystem changes.
///
/// Each `TouchedRootFile::cleanup` removes its own file and walks its ancestor
/// chain up to `SRC_ROOT`, removing any directory that has become empty. Shared
/// parent directories collapse naturally once the last file inside them is gone.
/// We sync `SRC_ROOT` once at the end so the next pooled session cannot observe
/// the previous root directory listing.
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

    use ruff_db::{files::FileError, system::SystemPath};

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
        drop(pooled);

        assert_eq!(pool_len(), 1, "scrubbed db should be released back to the pool");

        // Second checkout pops the only entry, so we are guaranteed the same db.
        let pooled = PooledMemoryDb::checkout().expect("re-checkout");
        assert_eq!(pool_len(), 0, "pool should be empty after re-checkout");
        assert!(
            matches!(system_path_to_file(pooled.db(), &path), Err(FileError::NotFound)),
            "previous run's file must not be visible in the reused db",
        );
        drop(pooled);

        drain_pool();
    }

    /// Nested file should be removed AND its parent directory collapsed, so a
    /// reused db cannot tell the previous run's module structure ever existed.
    #[test]
    fn nested_file_cleanup_removes_parent_directory() {
        let _guard = TEST_LOCK.lock().unwrap();
        drain_pool();

        let path = SystemPathBuf::from("/sub_dir/nested.py");

        let mut pooled = PooledMemoryDb::checkout().expect("checkout");
        pooled.write_root_file(&path, "x = 1\n").expect("write nested file");
        assert!(pooled.db().memory_file_system().is_directory("/sub_dir"));
        assert!(pooled.db().memory_file_system().is_file(&path));
        drop(pooled);

        // Pop the same db back out and confirm both file and parent dir are gone.
        let pooled = PooledMemoryDb::checkout().expect("re-checkout");
        let fs = pooled.db().memory_file_system();
        assert!(!fs.exists(&path), "nested file must be removed after cleanup");
        assert!(
            !fs.is_directory(SystemPath::new("/sub_dir")),
            "empty parent directory must be removed so it cannot leak into the next run"
        );
        drop(pooled);

        drain_pool();
    }

    /// Deep nesting: the whole ancestor chain between the file and `/` should
    /// collapse, leaving only `/` behind.
    #[test]
    fn deeply_nested_file_cleans_up_full_chain() {
        let _guard = TEST_LOCK.lock().unwrap();
        drain_pool();

        let path = SystemPathBuf::from("/a/b/c/deep.py");

        let mut pooled = PooledMemoryDb::checkout().expect("checkout");
        pooled
            .write_root_file(&path, "x = 1\n")
            .expect("write deeply nested file");
        drop(pooled);

        let pooled = PooledMemoryDb::checkout().expect("re-checkout");
        let fs = pooled.db().memory_file_system();
        for dir in ["/a/b/c", "/a/b", "/a"] {
            assert!(
                !fs.is_directory(SystemPath::new(dir)),
                "ancestor directory '{dir}' must be removed after cleanup"
            );
        }
        assert!(fs.is_directory(SystemPath::new("/")), "SRC_ROOT itself must stay");
        drop(pooled);

        drain_pool();
    }

    /// Two touched files in the same parent dir: the first cleanup hits
    /// `DirectoryNotEmpty` (silently tolerated), the second cleanup finally
    /// removes the now-empty directory.
    #[test]
    fn shared_parent_directory_collapses_when_last_file_removed() {
        let _guard = TEST_LOCK.lock().unwrap();
        drain_pool();

        let a = SystemPathBuf::from("/shared/a.py");
        let b = SystemPathBuf::from("/shared/b.py");

        let mut pooled = PooledMemoryDb::checkout().expect("checkout");
        pooled.write_root_file(&a, "x = 1\n").expect("write a");
        pooled.write_root_file(&b, "y = 2\n").expect("write b");
        assert!(pooled.db().memory_file_system().is_directory("/shared"));
        drop(pooled);

        let pooled = PooledMemoryDb::checkout().expect("re-checkout");
        let fs = pooled.db().memory_file_system();
        assert!(!fs.exists(&a));
        assert!(!fs.exists(&b));
        assert!(
            !fs.is_directory(SystemPath::new("/shared")),
            "shared parent must be removed once both files are gone",
        );
        drop(pooled);

        drain_pool();
    }

    fn pool_len() -> usize {
        MemoryDbPool::global().dbs.lock().unwrap().len()
    }

    fn drain_pool() {
        MemoryDbPool::global().dbs.lock().unwrap().clear();
    }
}
