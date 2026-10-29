//! Tests for importing modules from `.py` files: an import the sandbox does
//! not bundle or cache is one `import` call carrying where its file may be; a
//! host with no module of that name answers with the file it found, which runs
//! as the module and is cached for the session.
//!
//! These cover the host protocol: which imports are asked, unusual answers,
//! dumps and REPL feeds. Python semantics, compared with CPython, are in
//! `test_cases/import__files.py`.

use monty::{Dump, MontyRepl, MontyRun, ReplOsCall, ReplProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, IMPORT_CALL, ImportArgs, ModuleFile, ModuleRead, MontyException,
    MontyObject, MontyUuid, OsFunctionCall, PrintWriter, ResourceTracker, find_module_file,
};

/// The files a test's host serves, by virtual path.
type Files<'a> = &'a [(&'a str, &'a str)];

/// A REPL whose working directory is `/proj`.
fn repl() -> MontyRepl {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    repl.set_cwd("/proj");
    repl
}

/// Feeds `code`, answering imports with a file from `files` and every other OS
/// call as unhandled. The session is dumped and reloaded at each suspension.
/// Returns the session with the result, or the exception's last traceback line.
fn feed(repl: MontyRepl, code: &str, files: Files<'_>) -> (MontyRepl, Result<MontyObject, String>) {
    feed_logged(repl, code, files, &mut Vec::new())
}

/// [`feed`], recording the arguments of every `import` call in `imports`.
fn feed_logged(
    repl: MontyRepl,
    code: &str,
    files: Files<'_>,
    imports: &mut Vec<ImportArgs>,
) -> (MontyRepl, Result<MontyObject, String>) {
    let mut progress = repl.feed_start(code, Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout);
    loop {
        match progress {
            Ok(ReplProgress::Complete { repl, value }) => return (repl, Ok(value)),
            Ok(ReplProgress::OsCall(call)) => {
                let result = match &call.function_call {
                    OsFunctionCall::Import(args) => {
                        imports.push(args.clone());
                        answer_import(files, args)
                    }
                    other => other.no_handler_result(),
                };
                let ReplProgress::OsCall(call) = round_trip(ReplProgress::OsCall(call)) else {
                    unreachable!("a dump restores the same suspension")
                };
                progress = call.resume(result, PrintWriter::Stdout);
            }
            Ok(_) => panic!("unexpected suspension"),
            Err(err) => {
                let line = err.error.to_string().lines().last().unwrap().to_owned();
                return (err.repl, Err(line));
            }
        }
    }
}

/// A host with no host modules: the module's file from `files`, else not-found.
fn answer_import(files: Files<'_>, args: &ImportArgs) -> ExtFunctionResult {
    find_module_file(args, |path| match files.iter().find(|(file, _)| *file == path) {
        Some((_, text)) => ModuleRead::Text((*text).to_owned()),
        None => ModuleRead::Missing,
    })
    .unwrap_or_else(|| ExtFunctionResult::NotFound(IMPORT_CALL.to_owned()))
}

/// Dumps a suspended feed and loads it back, releasing the original.
fn round_trip(progress: ReplProgress) -> ReplProgress {
    let bytes = dump("main.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
    drop(progress.into_repl());
    let Session::Suspended(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped a suspended feed, loaded something else");
    };
    *progress
}

/// Runs `code` in a fresh session and returns its result.
fn eval(code: &str, files: Files<'_>) -> MontyObject {
    feed(repl(), code, files).1.unwrap()
}

/// The `(module, search path)` of each logged `import` call.
fn asked(imports: &[ImportArgs]) -> Vec<(&str, Vec<&str>)> {
    imports
        .iter()
        .map(|args| {
            (
                args.module.as_str(),
                args.search_path.iter().map(String::as_str).collect(),
            )
        })
        .collect()
}

/// Feeds `code` to a fresh session and returns its first `import` call.
fn first_import(code: &str) -> ReplOsCall {
    let progress = repl()
        .feed_start(code, Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap();
    let ReplProgress::OsCall(call) = progress else {
        panic!("expected an import");
    };
    assert_eq!(call.function_call.name(), IMPORT_CALL);
    call
}

/// The full traceback of the exception `code` raises, answering its imports from `files`.
fn traceback(code: &str, files: Files<'_>) -> String {
    let mut call = first_import(code);
    loop {
        let OsFunctionCall::Import(args) = &call.function_call else {
            panic!("expected only imports, got {}", call.function_call.name())
        };
        let result = answer_import(files, args);
        match call.resume(result, PrintWriter::Stdout) {
            Ok(ReplProgress::OsCall(next)) => call = next,
            Ok(_) => panic!("expected the import to raise"),
            Err(err) => return err.error.to_string(),
        }
    }
}

/// The last traceback line of the error resuming `call` with `result` raises.
fn resume_err(call: ReplOsCall, result: impl Into<ExtFunctionResult>) -> String {
    let err = call.resume(result, PrintWriter::Stdout).unwrap_err();
    err.error.to_string().lines().last().unwrap().to_owned()
}

/// The answer that the module's file at `path` holds `source`.
fn module_file(path: &str, source: &str) -> ExtFunctionResult {
    ExtFunctionResult::ModuleFile(ModuleFile {
        path: path.to_owned(),
        source: source.to_owned(),
    })
}

const FOO: &str = "x = 41\ndef f():\n    return x + 1\n";

#[test]
fn a_module_runs_once_per_session() {
    let files = &[("/proj/counter.py", "print('loading')\nitems = []\n")];
    let mut imports = Vec::new();
    let (repl, result) = feed_logged(
        repl(),
        "import counter\nimport counter as again\ncounter.items.append(1)\n(counter is again, again.items)",
        files,
        &mut imports,
    );
    assert_eq!(result.unwrap(), eval("(True, [1])", &[]));
    assert_eq!(asked(&imports), [("counter", vec!["/proj"])]);

    // the cache is session state: a later feed binds the same module, asking nothing
    let mut imports = Vec::new();
    let (_, result) = feed_logged(repl, "import counter\ncounter.items", files, &mut imports);
    assert_eq!(result.unwrap(), eval("[1]", &[]));
    assert_eq!(imports, []);
}

#[test]
fn the_cache_survives_an_idle_dump() {
    let files = &[("/proj/foo.py", FOO)];
    let (repl, result) = feed(repl(), "import foo\nfoo.x = 7", files);
    result.unwrap();
    let bytes = dump("main.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(repl) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped while idle");
    };
    let mut imports = Vec::new();
    let (_, result) = feed_logged(*repl, "import foo\nfoo.f()", files, &mut imports);
    assert_eq!(result.unwrap(), MontyObject::int(8));
    assert_eq!(imports, []);
}

#[test]
fn a_missing_module_is_one_import_call() {
    let mut imports = Vec::new();
    let (_, result) = feed_logged(repl(), "import tools", &[], &mut imports);
    assert_eq!(result.unwrap_err(), "ModuleNotFoundError: No module named 'tools'");
    assert_eq!(asked(&imports), [("tools", vec!["/proj"])]);
}

/// A host module answers the import call itself, so no file is used.
#[test]
fn a_host_module_shadows_a_file() {
    let call = first_import("import tools\ntools.x");
    let module = MontyObject::module(
        "tools",
        MontyUuid::from_u128(1),
        [(MontyObject::string("x"), MontyObject::string("host"))],
    );
    let ReplProgress::Complete { value, .. } = call.resume(module, PrintWriter::Stdout).unwrap() else {
        panic!("expected the snippet to complete");
    };
    assert_eq!(value, MontyObject::string("host"));
}

#[test]
fn the_cwd_is_searched_when_the_import_runs() {
    let files = &[
        ("/other/foo.py", "where = 'other'\n"),
        ("/proj/foo.py", "where = 'proj'\n"),
    ];
    assert_eq!(eval("import foo\nfoo.where", files), MontyObject::string("proj"));
    let mut repl = repl();
    repl.set_cwd("/other");
    assert_eq!(
        feed(repl, "import foo\nfoo.where", files).1.unwrap(),
        MontyObject::string("other")
    );
}

/// Every submodule of a file package is its own import call, searching the
/// package's `__path__`, whichever statement imports it.
#[test]
fn a_submodule_is_asked_with_the_package_path() {
    let files = &[
        ("/proj/pkg/__init__.py", ""),
        ("/proj/pkg/sub.py", "value = 3\n"),
        ("/proj/pkg/other.py", "value = 4\n"),
        ("/proj/far/__init__.py", "__path__ = ['elsewhere']\n"),
        ("/proj/elsewhere/mod.py", "value = 5\n"),
    ];
    let mut imports = Vec::new();
    let code = "import pkg.sub\nfrom pkg import other\nimport far.mod\n(pkg.sub.value, other.value, far.mod.value)";
    let (_, result) = feed_logged(repl(), code, files, &mut imports);
    assert_eq!(result.unwrap(), eval("(3, 4, 5)", &[]));
    assert_eq!(
        asked(&imports),
        [
            ("pkg", vec!["/proj"]),
            ("pkg.sub", vec!["/proj/pkg"]),
            ("pkg.other", vec!["/proj/pkg"]),
            ("far", vec!["/proj"]),
            // a relative `__path__` entry is joined to the working directory
            ("far.mod", vec!["/proj/elsewhere"]),
        ]
    );
}

#[test]
fn a_missing_submodule_raises_by_statement() {
    let files = &[("/proj/pkg/__init__.py", "")];
    assert_eq!(
        feed(repl(), "import pkg.nope", files).1.unwrap_err(),
        "ModuleNotFoundError: No module named 'pkg.nope'"
    );
    assert_eq!(
        feed(repl(), "from pkg import nope", files).1.unwrap_err(),
        "ImportError: cannot import name 'nope' from 'pkg' (/proj/pkg/__init__.py)"
    );
}

#[test]
fn an_error_in_a_module_shows_its_file_in_the_traceback() {
    let files = &[("/proj/bad.py", "def f():\n    return 1 / 0\n\nf()\n")];
    insta::assert_snapshot!(traceback("import bad", files), @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in <module>
        import bad
      File "/proj/bad.py", line 4, in <module>
        f()
        ~~~
      File "/proj/bad.py", line 2, in f
        return 1 / 0
               ~~~~~
    ZeroDivisionError: division by zero
    "#);
}

#[test]
fn a_syntax_error_in_a_module_names_its_file() {
    let files = &[("/proj/bad.py", "x = 1\nx = = 2\n")];
    insta::assert_snapshot!(traceback("import bad", files), @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in <module>
        import bad
        ~~~~~~~~~~
      File "/proj/bad.py", line 2
        x = = 2
            ~
    SyntaxError: Expected an expression
    "#);
    // nothing is cached: the next import asks for the file again
    let mut imports = Vec::new();
    let (repl, _) = feed_logged(repl(), "import bad", files, &mut imports);
    let _ = feed_logged(repl, "import bad", files, &mut imports);
    assert_eq!(asked(&imports), [("bad", vec!["/proj"]), ("bad", vec!["/proj"])]);
}

#[test]
fn unusual_answers() {
    let call = first_import("import foo");
    let call_id = call.call_id;
    assert_eq!(
        resume_err(call, ExtFunctionResult::Future(call_id)),
        "RuntimeError: import of 'foo' cannot be answered with a future"
    );
    let unreadable = MontyException::new(ExcType::OSError, Some("[Errno 5] Input/output error".to_owned()));
    assert_eq!(
        resume_err(first_import("import foo"), ExtFunctionResult::Error(unreadable)),
        "OSError: [Errno 5] Input/output error"
    );

    // a module file answers only an import
    let progress = repl()
        .feed_start(
            "import os\nos.getenv('X')",
            Vec::<(String, MontyObject)>::new(),
            PrintWriter::Stdout,
        )
        .unwrap();
    let ReplProgress::OsCall(getenv) = progress else {
        panic!("expected os.getenv to suspend");
    };
    assert_eq!(
        resume_err(getenv, module_file("/proj/x.py", "")),
        "RuntimeError: only an import can be answered with a module file"
    );
}

/// The reply's path decides whether the module is a package.
#[test]
fn a_package_is_a_file_named_init() {
    let call = first_import("import pkg\n(pkg.__file__, pkg.__path__, pkg.__package__)");
    let ReplProgress::Complete { value, .. } = call
        .resume(module_file("/srv/pkg/__init__.py", ""), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the snippet to complete");
    };
    assert_eq!(value, eval("('/srv/pkg/__init__.py', ['/srv/pkg'], 'pkg')", &[]));
}

#[test]
fn a_run_with_no_host_finds_no_file() {
    let err = MontyRun::new("import foo".to_owned(), "main.py", vec![], CompileOptions::default())
        .unwrap()
        .run(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(
        err.to_string().lines().last(),
        Some("ModuleNotFoundError: No module named 'foo'")
    );
}

/// A `ModuleNotFoundError` the host raises is the import's error, unchanged.
#[test]
fn a_host_import_error_is_raised() {
    let missing_dependency = MontyException::new(
        ExcType::ModuleNotFoundError,
        Some("No module named 'dependency'".to_owned()),
    );
    assert_eq!(
        resume_err(
            first_import("import tools"),
            ExtFunctionResult::Error(missing_dependency)
        ),
        "ModuleNotFoundError: No module named 'dependency'"
    );
}

#[test]
fn a_module_repr_quotes_its_path() {
    let files = &[("/it's/foo.py", "")];
    let mut repl = repl();
    repl.set_cwd("/it's");
    assert_eq!(
        feed(repl, "import foo\nrepr(foo)", files).1.unwrap(),
        MontyObject::string("<module 'foo' from \"/it's/foo.py\">")
    );
}

/// A snippet abandoned while a module body is suspended must not leave the
/// unfinished module cached: the next import runs the file again.
#[test]
fn an_abandoned_import_is_not_cached() {
    let files = &[("/proj/foo/__init__.py", "import os\nstage = os.getenv('STAGE')\n")];
    let progress = first_import("import foo")
        .resume(module_file(files[0].0, files[0].1), PrintWriter::Stdout)
        .unwrap();
    let ReplProgress::OsCall(getenv) = progress else {
        panic!("expected the module body to suspend");
    };
    assert_eq!(getenv.function_call.name(), "os.getenv");
    let repl = getenv.into_repl();

    let mut imports = Vec::new();
    let (_, result) = feed_logged(repl, "import foo", files, &mut imports);
    assert_eq!(asked(&imports), [("foo", vec!["/proj"])]);
    // the body runs again, reaching the `os.getenv` this harness does not serve
    assert_eq!(
        result.unwrap_err(),
        "RuntimeError: 'os.getenv' is not supported in this environment"
    );
}
