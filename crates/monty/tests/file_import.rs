//! Tests for importing modules from `.py` files: an import the host has no
//! module for reads candidate files with `Path.read_text` OS calls, runs the
//! first it finds as the module, and caches the module for the session.

use monty::{Dump, MontyRepl, MontyRun, ReplOsCall, ReplProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, IMPORT_CALL, MontyException, MontyObject, MontyUuid, OsFunctionCall,
    PrintWriter, ResourceTracker,
};

/// The files a test's host serves, by virtual path.
type Files<'a> = &'a [(&'a str, &'a str)];

/// A REPL whose working directory is `/proj`.
fn repl() -> MontyRepl {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    repl.set_cwd("/proj");
    repl
}

/// Feeds `code`, answering reads from `files` and every other OS call (host
/// imports included) as unhandled. The session is dumped and reloaded at each
/// suspension. Returns the session with the result, or the exception's last
/// traceback line.
fn feed(repl: MontyRepl, code: &str, files: Files<'_>) -> (MontyRepl, Result<MontyObject, String>) {
    feed_logged(repl, code, files, &mut Vec::new())
}

/// [`feed`], recording the path of every file read in `reads`.
fn feed_logged(
    repl: MontyRepl,
    code: &str,
    files: Files<'_>,
    reads: &mut Vec<String>,
) -> (MontyRepl, Result<MontyObject, String>) {
    let mut progress = repl.feed_start(code, Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout);
    loop {
        match progress {
            Ok(ReplProgress::Complete { repl, value }) => return (repl, Ok(value)),
            Ok(ReplProgress::OsCall(call)) => {
                let result = match &call.function_call {
                    OsFunctionCall::ReadText(path) => {
                        reads.push(path.as_str().to_owned());
                        read(files, path.as_str())
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

/// The host's answer to reading `path`.
fn read(files: Files<'_>, path: &str) -> ExtFunctionResult {
    match files.iter().find(|(file, _)| *file == path) {
        Some((_, text)) => MontyObject::string(*text).into(),
        None => ExtFunctionResult::Error(MontyException::new(
            ExcType::FileNotFoundError,
            Some(format!("[Errno 2] No such file or directory: '{path}'")),
        )),
    }
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

/// Runs `code` in a fresh session and returns its exception's last traceback line.
fn eval_err(code: &str, files: Files<'_>) -> String {
    feed(repl(), code, files).1.unwrap_err()
}

/// Feeds `code` to a fresh session, answers its host import `not_found`, and
/// returns the first read that follows.
fn first_read(code: &str) -> ReplOsCall {
    let progress = repl()
        .feed_start(code, Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap();
    let ReplProgress::OsCall(call) = progress else {
        panic!("expected the host import");
    };
    assert_eq!(call.function_call.name(), IMPORT_CALL);
    let not_found = ExtFunctionResult::NotFound(IMPORT_CALL.to_owned());
    let ReplProgress::OsCall(call) = call.resume(not_found, PrintWriter::Stdout).unwrap() else {
        panic!("expected a read");
    };
    assert!(matches!(call.function_call, OsFunctionCall::ReadText(_)));
    call
}

/// The full traceback of the exception `code` raises, whose first read is of a file in `files`.
fn traceback(code: &str, files: Files<'_>) -> String {
    let call = first_read(code);
    let OsFunctionCall::ReadText(path) = &call.function_call else {
        unreachable!("first_read returns a read")
    };
    let result = read(files, path.as_str());
    let Err(err) = call.resume(result, PrintWriter::Stdout) else {
        panic!("expected the import to raise");
    };
    err.error.to_string()
}

/// The last traceback line of the error resuming `call` with `result` raises.
fn resume_err(call: ReplOsCall, result: impl Into<ExtFunctionResult>) -> String {
    let err = call.resume(result, PrintWriter::Stdout).unwrap_err();
    err.error.to_string().lines().last().unwrap().to_owned()
}

const FOO: &str = "x = 41\ndef f():\n    return x + 1\n";

#[test]
fn a_module_file_in_the_cwd_is_imported() {
    let files = &[("/proj/foo.py", FOO)];
    assert_eq!(eval("import foo\nfoo.f()", files), MontyObject::int(42));
    assert_eq!(eval("import foo as m\nm.x", files), MontyObject::int(41));
    assert_eq!(
        eval("from foo import f, x as y\n(f(), y)", files),
        eval("(42, 41)", &[])
    );
}

#[test]
fn a_module_reports_its_dunders_and_repr() {
    let files = &[("/proj/foo.py", FOO)];
    assert_eq!(
        eval(
            "import foo\n(foo.__name__, foo.__file__, foo.__package__, repr(foo))",
            files
        ),
        MontyObject::tuple([
            MontyObject::string("foo"),
            MontyObject::string("/proj/foo.py"),
            MontyObject::string(""),
            MontyObject::string("<module 'foo' from '/proj/foo.py'>"),
        ])
    );
    let files = &[("/proj/foo.py", "name = __name__\nfile = __file__\n")];
    assert_eq!(
        eval("import foo\n(foo.name, foo.file)", files),
        eval("('foo', '/proj/foo.py')", &[])
    );
}

/// Attribute writes go to the module's namespace, which its functions read.
#[test]
fn assigning_a_module_attribute_rebinds_its_global() {
    let files = &[("/proj/foo.py", FOO)];
    assert_eq!(eval("import foo\nfoo.x = 1\nfoo.f()", files), MontyObject::int(2));
    assert_eq!(eval("import foo\nfoo.y = 5\nfoo.y", files), MontyObject::int(5));
}

#[test]
fn a_module_runs_once_per_session() {
    let files = &[("/proj/counter.py", "print('loading')\nitems = []\n")];
    let mut reads = Vec::new();
    let (repl, result) = feed_logged(
        repl(),
        "import counter\nimport counter as again\ncounter.items.append(1)\n(counter is again, again.items)",
        files,
        &mut reads,
    );
    assert_eq!(result.unwrap(), eval("(True, [1])", &[]));
    assert_eq!(reads, ["/proj/counter.py"]);

    // the cache is session state: a later feed binds the same module, reading nothing
    let mut reads = Vec::new();
    let (_, result) = feed_logged(repl, "import counter\ncounter.items", files, &mut reads);
    assert_eq!(result.unwrap(), eval("[1]", &[]));
    assert_eq!(reads, Vec::<String>::new());
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
    let mut reads = Vec::new();
    let (_, result) = feed_logged(*repl, "import foo\nfoo.f()", files, &mut reads);
    assert_eq!(result.unwrap(), MontyObject::int(8));
    assert_eq!(reads, Vec::<String>::new());
}

#[test]
fn a_package_and_its_submodules_are_imported() {
    let files = &[
        ("/proj/pkg/__init__.py", "top = 'pkg'\n"),
        ("/proj/pkg/sub.py", "value = 3\n"),
        ("/proj/pkg/inner/__init__.py", ""),
        ("/proj/pkg/inner/deep.py", "value = 4\n"),
    ];
    assert_eq!(eval("import pkg\npkg.top", files), MontyObject::string("pkg"));
    assert_eq!(eval("import pkg.sub\npkg.sub.value", files), MontyObject::int(3));
    assert_eq!(eval("import pkg.sub as s\ns.value", files), MontyObject::int(3));
    assert_eq!(eval("from pkg import sub\nsub.value", files), MontyObject::int(3));
    assert_eq!(eval("from pkg.sub import value\nvalue", files), MontyObject::int(3));
    assert_eq!(
        eval("import pkg.inner.deep\npkg.inner.deep.value", files),
        MontyObject::int(4)
    );
    assert_eq!(
        eval(
            "import pkg.sub\nfrom pkg import sub\nimport pkg\n(pkg.sub is sub, pkg.__path__, pkg.__package__, sub.__package__)",
            files
        ),
        eval("(True, ['/proj/pkg'], 'pkg', 'pkg')", &[])
    );
}

#[test]
fn missing_names_and_submodules_raise_cpython_errors() {
    let files = &[("/proj/foo.py", FOO), ("/proj/pkg/__init__.py", "")];
    assert_eq!(
        eval_err("from foo import nope", files),
        "ImportError: cannot import name 'nope' from 'foo' (/proj/foo.py)"
    );
    assert_eq!(
        eval_err("from pkg import nope", files),
        "ImportError: cannot import name 'nope' from 'pkg' (/proj/pkg/__init__.py)"
    );
    assert_eq!(
        eval_err("import foo.x", files),
        "ModuleNotFoundError: No module named 'foo.x'; 'foo' is not a package"
    );
    assert_eq!(
        eval_err("import pkg.nope", files),
        "ModuleNotFoundError: No module named 'pkg.nope'"
    );
    assert_eq!(
        eval_err("import nope", files),
        "ModuleNotFoundError: No module named 'nope'"
    );
    assert_eq!(
        eval_err("import foo\nfoo.nope", files),
        "AttributeError: module 'foo' has no attribute 'nope'"
    );
}

/// The host is asked first; only a module it lacks is read from a file.
#[test]
fn a_module_the_host_lacks_is_read_from_a_file() {
    let mut reads = Vec::new();
    let (_, result) = feed_logged(repl(), "import tools", &[], &mut reads);
    assert_eq!(result.unwrap_err(), "ModuleNotFoundError: No module named 'tools'");
    assert_eq!(reads, ["/proj/tools.py", "/proj/tools/__init__.py"]);

    // a host module shadows a file of the same name, which is never read
    let progress = repl()
        .feed_start(
            "import tools
tools.x",
            Vec::<(String, MontyObject)>::new(),
            PrintWriter::Stdout,
        )
        .unwrap();
    let ReplProgress::OsCall(call) = progress else {
        panic!("expected the host import");
    };
    assert!(matches!(&call.function_call, OsFunctionCall::Import(name) if name == "tools"));
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

#[test]
fn a_raising_module_is_not_cached() {
    let files = &[
        (
            "/proj/flaky.py",
            "import attempts\nattempts.count += 1\nif attempts.count == 1:\n    raise ValueError('first time')\n",
        ),
        ("/proj/attempts.py", "count = 0\n"),
    ];
    let code = "try:\n    import flaky\nexcept ValueError as e:\n    first = str(e)\nimport flaky\nimport attempts\n(first, attempts.count)";
    assert_eq!(eval(code, files), eval("('first time', 2)", &[]));
}

#[test]
fn an_error_in_a_module_shows_its_file_in_the_traceback() {
    let files = &[("/proj/bad.py", "def f():\n    return 1 / 0\n\nf()\n")];
    insta::assert_snapshot!(traceback("import bad", files), @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in <module>
        import bad
        ~~~~~~~~~~
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
    // nothing is cached: the next import reads the file again
    let mut reads = Vec::new();
    let (repl, _) = feed_logged(repl(), "import bad", files, &mut reads);
    let _ = feed_logged(repl, "import bad", files, &mut reads);
    assert_eq!(reads, ["/proj/bad.py", "/proj/bad.py"]);
}

#[test]
fn circular_imports_bind_the_partly_initialised_module() {
    let files = &[
        ("/proj/a.py", "first = 1\nimport b\nlast = b.seen\n"),
        (
            "/proj/b.py",
            "import a\nseen = a.first\nhas_last = hasattr(a, 'last')\n",
        ),
    ];
    assert_eq!(eval("import a\n(a.last, a.b.has_last)", files), eval("(1, False)", &[]));
}

#[test]
fn unusual_answers_to_a_read() {
    // a handler that does not serve the read: the file is not there
    let call = first_read("import foo");
    assert!(matches!(&call.function_call, OsFunctionCall::ReadText(path) if path.as_str() == "/proj/foo.py"));
    let ReplProgress::OsCall(next) = call
        .resume(
            ExtFunctionResult::NotFound("Path.read_text".to_owned()),
            PrintWriter::Stdout,
        )
        .unwrap()
    else {
        panic!("expected the next read");
    };
    assert!(matches!(&next.function_call, OsFunctionCall::ReadText(path) if path.as_str() == "/proj/foo/__init__.py"));

    let call = first_read("import foo");
    let call_id = call.call_id;
    assert_eq!(
        resume_err(call, ExtFunctionResult::Future(call_id)),
        "RuntimeError: import cannot be answered with a future"
    );
    assert_eq!(
        resume_err(first_read("import foo"), MontyObject::bytes(b"x = 1".to_vec())),
        "RuntimeError: invalid return type: Path.read_text for an import must return a str, not bytes"
    );
    let unreadable = MontyException::new(ExcType::OSError, Some("[Errno 5] Input/output error".to_owned()));
    assert_eq!(
        resume_err(first_read("import foo"), ExtFunctionResult::Error(unreadable)),
        "OSError: [Errno 5] Input/output error"
    );
}

#[test]
fn a_run_with_no_host_reads_no_files() {
    let err = MontyRun::new("import foo".to_owned(), "main.py", vec![], CompileOptions::default())
        .unwrap()
        .run(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(
        err.to_string().lines().last(),
        Some("ModuleNotFoundError: No module named 'foo'")
    );
}

/// Module code resolves names through its own globals and the builtins, so a
/// name the session would ask the host for is undefined there unless bound.
#[test]
fn module_code_does_not_ask_the_host_for_names() {
    let files = &[("/proj/helpers.py", "def run():\n    return fetch()\n")];
    assert_eq!(
        eval_err("import helpers\nhelpers.run()", files),
        "NameError: name 'fetch' is not defined"
    );
    assert_eq!(
        eval("import helpers\nhelpers.fetch = lambda: 42\nhelpers.run()", files),
        MontyObject::int(42)
    );
}

/// Only `not_found` (what a host with no handler answers) looks for a file: a
/// `ModuleNotFoundError` the host raises is the import's error.
#[test]
fn a_host_import_error_reads_no_files() {
    let progress = repl()
        .feed_start("import tools", Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap();
    let ReplProgress::OsCall(call) = progress else {
        panic!("expected the host import");
    };
    let missing_dependency = MontyException::new(
        ExcType::ModuleNotFoundError,
        Some("No module named 'dependency'".to_owned()),
    );
    assert_eq!(
        resume_err(call, ExtFunctionResult::Error(missing_dependency)),
        "ModuleNotFoundError: No module named 'dependency'"
    );
}

#[test]
fn a_package_path_may_be_a_tuple() {
    let files = &[
        ("/proj/pkg/__init__.py", "__path__ = ('/elsewhere',)\n"),
        ("/elsewhere/sub.py", "value = 5\n"),
    ];
    assert_eq!(eval("import pkg.sub\npkg.sub.value", files), MontyObject::int(5));
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
    let files = &[("/proj/foo.py", "import os\nstage = os.getenv('STAGE')\n")];
    let call = first_read("import foo");
    let progress = call.resume(read(files, "/proj/foo.py"), PrintWriter::Stdout).unwrap();
    let ReplProgress::OsCall(getenv) = progress else {
        panic!("expected the module body to suspend");
    };
    assert_eq!(getenv.function_call.name(), "os.getenv");
    let repl = getenv.into_repl();

    let mut reads = Vec::new();
    let (_, result) = feed_logged(repl, "import foo", files, &mut reads);
    assert_eq!(reads, ["/proj/foo.py"]);
    // the body runs again, reaching the `os.getenv` this harness does not serve
    assert_eq!(
        result.unwrap_err(),
        "RuntimeError: 'os.getenv' is not supported in this environment"
    );
}
