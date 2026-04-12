use std::{
    fmt,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
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

/// Virtual source root — user files are written under this prefix and module resolution
/// treats it as the project root. Shared between [`build_warm_db`] and [`type_check`].
pub(crate) const SRC_ROOT: &str = "/";

/// Path used by the warmup dummy file. Picked to never collide with any user-supplied
/// script path so its stale interning is never queried again after the db is built.
const WARMUP_PATH: &str = "/__monty_warmup__.py";

/// Prefix used to uniquify per-call internal file paths.
///
/// Each `type_check` call writes to paths like `/__mc42__main.py`, which creates a
/// distinct salsa `File` ingredient per call so memos from one call never shadow
/// another's. The prefix is stripped from rendered diagnostics so users see plain
/// paths like `main.py` — see [`strip_unique_prefix`].
pub(crate) const CALL_PATH_PREFIX: &str = "__mc";

/// Separator between the call counter and the user's script name (e.g. `__mc42__`).
pub(crate) const CALL_PATH_SEP: &str = "__";

/// Process-wide counter feeding [`CALL_PATH_PREFIX`]; bumped atomically per call.
static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return the next unique internal-path prefix and its call counter.
///
/// The returned `String` (e.g. `"__mc42__"`) is what appears in internal file paths
/// and must be passed to [`strip_unique_prefix`] when rendering diagnostics.
pub(crate) fn next_call_prefix() -> String {
    let counter = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{CALL_PATH_PREFIX}{counter}{CALL_PATH_SEP}")
}

/// Remove every `__mc<digits>__` segment from `rendered`.
///
/// Used by `TypeCheckingDiagnostics` to hide the internal uniquifying prefix from
/// the end-user — file paths in error output should read `main.py`, not
/// `__mc42__main.py`. Implemented as a hand-rolled scanner to avoid pulling in a
/// regex crate for a trivial pattern.
pub(crate) fn strip_unique_prefix(rendered: &str) -> String {
    let mut out = String::with_capacity(rendered.len());
    let mut cursor = 0;
    let bytes = rendered.as_bytes();
    while let Some(start_rel) = rendered[cursor..].find(CALL_PATH_PREFIX) {
        let start = cursor + start_rel;
        out.push_str(&rendered[cursor..start]);
        let after_prefix = start + CALL_PATH_PREFIX.len();
        let mut digit_end = after_prefix;
        while digit_end < bytes.len() && bytes[digit_end].is_ascii_digit() {
            digit_end += 1;
        }
        // Only strip when we have at least one digit followed by the separator — otherwise
        // `__mc` is an ordinary substring (e.g. appearing in user code) and should stay.
        if digit_end > after_prefix && rendered[digit_end..].starts_with(CALL_PATH_SEP) {
            cursor = digit_end + CALL_PATH_SEP.len();
        } else {
            out.push_str(CALL_PATH_PREFIX);
            cursor = after_prefix;
        }
    }
    out.push_str(&rendered[cursor..]);
    out
}

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

/// The single process-wide type-checking database.
///
/// ## Why one shared db instead of "clone-per-call"?
///
/// Salsa's input setters (what `write_file` ultimately calls) require exclusive access to
/// the underlying `Arc<Zalsa>` via `Arc::get_mut`. If anything else holds a clone of the
/// storage, `get_mut` fails and salsa's `cancel_others()` waits indefinitely for the other
/// handles to drop — a single long-lived "warm template" whose handle never drops would
/// deadlock the very next write, even from a single-threaded caller. So instead we keep
/// **one** `MemoryDb` behind a [`Mutex`] and serialize all `type_check` calls through it.
///
/// ## How correctness is preserved across calls
///
/// Each call writes its user code to a path containing a unique counter (see
/// [`next_call_prefix`]), which creates a fresh salsa `File` ingredient per call.
/// That means memos computed for call 1's `File` can never be accidentally returned
/// for call 2's `File` (different salsa key), so diagnostics produced by an earlier
/// call continue to render correctly even after later calls have run — they query
/// their own `File`'s source text, which still lives in the shared in-memory fs.
///
/// ## Warmup
///
/// First access runs `build_warm_db`, which initializes the salsa `Program` singleton,
/// resolves vendored typeshed search paths, and runs `check_types` on a trivial snippet
/// referencing the common builtins (`int`, `str`, etc.). This populates the salsa memo
/// table so the first *real* call doesn't pay the stdlib parse cost.
pub(crate) fn global_db() -> &'static Mutex<MemoryDb> {
    static GLOBAL_DB: OnceLock<Mutex<MemoryDb>> = OnceLock::new();
    GLOBAL_DB.get_or_init(|| Mutex::new(build_warm_db()))
}

/// Build the one-shot warm database. Runs once behind the [`global_db`] `OnceLock`.
fn build_warm_db() -> MemoryDb {
    let mut db = MemoryDb::new();
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

    // Prewarm: parse and analyze the stdlib stubs every real call transitively touches.
    // The warmup references the most common builtin types (`int`, `str`, `float`, `list`,
    // `dict`, `bool`) so typechecking resolves them from typeshed's `builtins.pyi` /
    // `typing.pyi` / `types.pyi` and caches the resulting semantic graph. These memos
    // live in the db's salsa storage and are reused by every subsequent `type_check`
    // call, so the first real call doesn't pay the stdlib parse cost.
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
