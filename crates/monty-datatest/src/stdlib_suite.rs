//! Runs vendored suites with native discovery but without shipping `unittest` in Monty.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, PoisonError},
};

use monty_fs::{MountMode, MountTable};
use monty_type_checking::{SourceFile, TypeCheckContext, TypeChecker};
use monty_types::{ModuleStub, TypeCheckingConfig};
use pyo3::prelude::*;
use serde::Deserialize;

use crate::{
    CPYTHON_TEST_LOCK, CpythonWatchdog, Expectation, TestConfig, format_cpython_exception, import_script,
    run_monty_case, try_run_mounted_test,
};

pub(crate) const STDLIB_TESTS_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/cpython/work/generated/Lib/test");
const PROJECTED_TESTS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vendor/cpython/work/generated/monty/Lib/test"
);

#[derive(Deserialize)]
struct VendorManifest {
    tests: TestSources,
}

#[derive(Deserialize)]
struct TestSources {
    suites: Vec<String>,
    #[serde(default)]
    project: BTreeMap<String, Vec<String>>,
}

static TEST_SOURCES: LazyLock<TestSources> = LazyLock::new(|| {
    let manifest: VendorManifest =
        toml::from_str(include_str!("../../monty/stdlib/vendor.toml")).expect("invalid vendor.toml");
    manifest.tests
});

/// Only run the enabled suites even though the complete upstream test tree is present.
pub(crate) fn suite_pattern() -> String {
    assert!(
        !TEST_SOURCES.suites.is_empty(),
        "vendor.toml: no CPython suites enabled"
    );
    let mut patterns = Vec::with_capacity(TEST_SOURCES.suites.len());
    for file in &TEST_SOURCES.suites {
        let path = Path::new(STDLIB_TESTS_DIR).join(file);
        assert!(
            path.is_file(),
            "missing CPython suite {}: run `make prepare-cpython-tests`",
            path.display()
        );
        patterns.push(
            file.split('/')
                .map(regex_syntax::escape)
                .collect::<Vec<_>>()
                .join(r"[/\\]"),
        );
    }
    format!("^(?:{})$", patterns.join("|"))
}

/// Run per-method fixtures and drain the cleanup stack even if setup or cleanup fails.
const MONTY_RUNNER: &str = r"

def _monty_run_unittest(cls, method):
    case = cls(method)
    try:
        case.setUp()
        try:
            getattr(case, method)()
        finally:
            case.tearDown()
    finally:
        cleanup_error = None
        while case._cleanups:
            function, args, kwargs = case._cleanups.pop()
            try:
                function(*args, **kwargs)
            except BaseException as exc:
                if cleanup_error is None:
                    cleanup_error = exc
        if cleanup_error is not None:
            raise cleanup_error

";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Manifest {
    suites: BTreeMap<String, SuitePolicy>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SuitePolicy {
    /// Use this instead of method exclusions if Monty cannot load the suite.
    skip: Option<String>,
    exclude: BTreeMap<String, String>,
}

static MANIFEST: LazyLock<Result<Manifest, String>> = LazyLock::new(|| {
    let manifest: Manifest =
        toml::from_str(include_str!("../vendor/cpython/skips.toml")).map_err(|err| format!("skips.toml: {err}"))?;
    for (file, policy) in &manifest.suites {
        if !TEST_SOURCES.suites.contains(file) {
            return Err(format!("skips.toml: suite {file} is not enabled in vendor.toml"));
        }
        if policy.skip.is_some() && !policy.exclude.is_empty() {
            return Err(format!("skips.toml: {file} cannot combine 'skip' with 'exclude'"));
        }
        if policy
            .skip
            .iter()
            .chain(policy.exclude.values())
            .any(|reason| reason.trim().is_empty())
        {
            return Err(format!("skips.toml: {file} has an exclusion without a reason"));
        }
    }
    Ok(manifest)
});

/// One file in the vendored CPython `test` package.
struct Suite {
    path: PathBuf,
    file: String,
    module: String,
}

impl Suite {
    fn new(path: &Path) -> Result<Self, Box<dyn Error>> {
        let root = Path::new(STDLIB_TESTS_DIR).canonicalize()?;
        let path = path.canonicalize()?;
        let file = path
            .strip_prefix(root)?
            .to_str()
            .ok_or("non-UTF-8 suite filename")?
            .replace('\\', "/");
        if !TEST_SOURCES.suites.contains(&file) {
            return Err(format!("suite {file} is not enabled in vendor.toml").into());
        }
        let name = file
            .strip_suffix("/__init__.py")
            .or_else(|| file.strip_suffix(".py"))
            .ok_or("expected a Python suite")?;
        let module = format!("test.{}", name.replace('/', "."));
        Ok(Self { path, file, module })
    }

    /// The test tree Monty imports and type-checks, which may contain a subset of the suite.
    fn monty_tree(&self) -> &'static Path {
        let root = if TEST_SOURCES.project.contains_key(&self.file) {
            PROJECTED_TESTS_DIR
        } else {
            STDLIB_TESTS_DIR
        };
        Path::new(root)
    }

    /// Discovery executes suite code too, so it shares the CPython runner's lock and timeout.
    fn with_cpython<T>(
        &self,
        tree: &Path,
        run: impl for<'py> FnOnce(Bound<'py, PyModule>, &str, &str, &str) -> PyResult<T>,
    ) -> Result<T, Box<dyn Error>> {
        let path = tree.join(&self.file);
        let path = path.to_str().ok_or("non-UTF-8 suite path")?;
        let root = tree.to_str().ok_or("non-UTF-8 test tree")?;
        let _lock = CPYTHON_TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let watchdog = CpythonWatchdog::arm(&self.module);
        let result = Python::attach(|py| {
            run(import_script(py, "run_stdlib_suite"), root, path, &self.module)
                .map_err(|err| format_cpython_exception(py, &err))
        });
        watchdog.check(&self.module).map_err(|err| err.to_string())?;
        Ok(result?)
    }
}

pub(crate) fn run_test_cases_monty_stdlib(path: &Path) -> Result<(), Box<dyn Error>> {
    let suite = Suite::new(path)?;
    let manifest = MANIFEST.as_ref().map_err(String::as_str)?;
    let policy = manifest.suites.get(&suite.file);
    if let Some(reason) = policy.and_then(|policy| policy.skip.as_ref()) {
        eprintln!("Monty excluded suite {}: {reason}", suite.module);
        return Ok(());
    }

    let tree = suite.monty_tree();
    let cases: Vec<(String, String, String)> = suite.with_cpython(tree, |runner, root, path, module| {
        runner.call_method1("discover", (root, path, module))?.extract()
    })?;
    let calls = monty_test_calls(&suite.module, &cases, policy)?;
    let code = format!("import {} as _monty_suite\n{MONTY_RUNNER}{calls}", suite.module);
    let lib = tree.parent().expect("test directory is under Lib");
    let mut mounts = MountTable::new();
    mounts.mount("/mnt", lib, MountMode::ReadOnly, None)?;
    run_monty_case(&suite.path, false, move |path| {
        try_run_mounted_test(
            path,
            &code,
            &Expectation::NoException,
            &TestConfig::default(),
            &mut mounts,
        )
    })
}

/// Check the suite source staged for Monty, including runtime-excluded methods.
pub(crate) fn run_type_check_stdlib(path: &Path) -> Result<(), Box<dyn Error>> {
    let suite = Suite::new(path)?;
    let source = fs::read_to_string(suite.monty_tree().join(&suite.file))?;
    let module_stubs = [ModuleStub::new(
        "unittest",
        include_str!("../vendor/cpython/unittest.pyi"),
    )?];
    let context = TypeCheckContext {
        module_stubs: &module_stubs,
        ..Default::default()
    };
    let mut checker = TypeChecker::default();
    let config = TypeCheckingConfig {
        color: false,
        ..Default::default()
    };
    match checker.run_with(&SourceFile::new(&source, &suite.file), &context, config)? {
        Some(diagnostics) => {
            eprintln!("Type diagnostics for {}:\n{diagnostics}", suite.file);
            Err("upstream suite did not type-check".into())
        }
        None => Ok(()),
    }
}

/// Render calls for the selected tests and reject stale exclusions or an empty selection.
fn monty_test_calls(
    module: &str,
    cases: &[(String, String, String)],
    policy: Option<&SuitePolicy>,
) -> Result<String, Box<dyn Error>> {
    let mut discovered = BTreeSet::new();
    let mut calls = String::new();
    for (id, class, method) in cases {
        if !discovered.insert(id) {
            return Err(format!("{module}: unittest discovered {id} twice").into());
        }
        if let Some(reason) = policy.and_then(|policy| policy.exclude.get(id)) {
            eprintln!("Monty excluded test {id}: {reason}");
        } else {
            writeln!(calls, "_monty_run_unittest(_monty_suite.{class}, '{method}')")?;
        }
    }
    if let Some(policy) = policy {
        for id in policy.exclude.keys() {
            if !discovered.contains(id) {
                return Err(format!("skips.toml: {module} did not discover {id}").into());
            }
        }
    }
    if calls.is_empty() {
        Err(format!("{module}: no tests selected on Monty").into())
    } else {
        Ok(calls)
    }
}

pub(crate) fn run_test_cases_cpython_stdlib(path: &Path) -> Result<(), Box<dyn Error>> {
    let suite = Suite::new(path)?;
    let report: String = suite.with_cpython(Path::new(STDLIB_TESTS_DIR), |runner, root, path, module| {
        runner.call_method1("run", (root, path, module))?.extract()
    })?;
    if !report.is_empty() {
        eprint!("{report}");
    }
    Ok(())
}
