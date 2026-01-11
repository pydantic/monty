use std::sync::Arc;

use ruff_db::{
    diagnostic::{DiagnosticFormat, DisplayDiagnosticConfig, DisplayDiagnostics},
    files::{system_path_to_file, File, Files},
    system::{DbWithTestSystem, DbWithWritableSystem as _, System, TestSystem},
    vendored::VendoredFileSystem,
    Db as SourceDb,
};
use ruff_python_ast::PythonVersion;
use ty_module_resolver::{Db as ModuleResolverDb, SearchPathSettings, SearchPaths};
use ty_python_semantic::{
    default_lint_registry,
    lint::{LintRegistry, RuleSelection},
    types::check_types,
    AnalysisSettings, Db, Program, ProgramSettings, PythonPlatform, PythonVersionSource, PythonVersionWithSource,
};

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    files: Files,
    system: TestSystem,
    vendored: VendoredFileSystem,
    rule_selection: Arc<RuleSelection>,
    analysis_settings: Arc<AnalysisSettings>,
}

impl TestDb {
    pub(crate) fn new() -> Self {
        Self {
            storage: salsa::Storage::new(None),
            system: TestSystem::default(),
            vendored: ty_vendored::file_system().clone(),
            files: Files::default(),
            rule_selection: Arc::new(RuleSelection::from_registry(default_lint_registry())),
            analysis_settings: AnalysisSettings::default().into(),
        }
    }
}

impl DbWithTestSystem for TestDb {
    fn test_system(&self) -> &TestSystem {
        &self.system
    }

    fn test_system_mut(&mut self) -> &mut TestSystem {
        &mut self.system
    }
}

#[salsa::db]
impl SourceDb for TestDb {
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
impl Db for TestDb {
    fn should_check_file(&self, file: File) -> bool {
        !file.path(self).is_vendored_path()
    }

    fn rule_selection(&self, _file: File) -> &RuleSelection {
        &self.rule_selection
    }

    fn lint_registry(&self) -> &LintRegistry {
        default_lint_registry()
    }

    fn analysis_settings(&self) -> &AnalysisSettings {
        &self.analysis_settings
    }

    fn verbose(&self) -> bool {
        false
    }
}

#[salsa::db]
impl ModuleResolverDb for TestDb {
    fn search_paths(&self) -> &SearchPaths {
        Program::get(self).search_paths(self)
    }
}

#[salsa::db]
impl salsa::Database for TestDb {}

pub fn run_type_checking(_path: &str, _source: &str) {
    let path = "potato.py";
    let source = "
def foo(x: int) -> int:
    return x + 1

foo('wrong')
";

    let mut db = TestDb::new();

    Program::from_settings(
        &db,
        ProgramSettings {
            python_version: PythonVersionWithSource {
                version: db.python_version(),
                source: PythonVersionSource::default(),
            },
            python_platform: PythonPlatform::default(),
            search_paths: SearchPathSettings::new(vec![])
                .to_search_paths(db.system(), db.vendored())
                .unwrap(),
        },
    );

    db.write_files(vec![(path, source)]).unwrap();
    let file = system_path_to_file(&db, path).unwrap();
    let diagnostics = check_types(&db, file);

    // set format and color here.
    let display_config = DisplayDiagnosticConfig::default()
        .format(DiagnosticFormat::Full)
        .color(false);

    println!("{}", DisplayDiagnostics::new(&db, &display_config, &diagnostics));
}
