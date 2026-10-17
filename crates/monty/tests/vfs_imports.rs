//! Native imports delegated to sandbox code, without host Python imports or source rewriting.

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, ReplProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, IMPORT_CALL, MontyException, MontyObject, NameLookupResult,
    OsFunctionCall, PrintWriter, ResourceLimits, ResourceTracker,
};

const LOADER: &str = include_str!("fixtures/vfs_import_loader.py");

/// Runs a snippet in an isolated REPL, keeping library functions and namespaces alive.
fn run(repl: &mut MontyRepl, code: &str) -> MontyObject {
    repl.feed_run(code, vec![], PrintWriter::Stdout).unwrap()
}

/// Creates a loader whose source reads can only address this in-memory virtual filesystem.
fn repl_with_files(files: &str) -> MontyRepl {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    run(&mut repl, LOADER);
    run(
        &mut repl,
        &format!(
            r"
files = {files}
reads = []
def read_source(path):
    reads.append(path)
    if path not in files:
        raise FileNotFoundError(path)
    return files[path]
__monty_import__ = make_vfs_importer(read_source, {{}})
"
        ),
    );
    repl
}

#[test]
fn native_dot_named_alias_and_transitive_imports_share_one_cache() {
    let mut repl = repl_with_files(
        r#"{
            '/modules/shared.py': '''value = 40
def add(n):
    return value + n''',
            '/modules/left.py': '''from shared import add
def answer():
    return add(2)''',
            '/modules/right.py': '''import shared
def answer():
    return shared.add(3)''',
            '/modules/tools.py': 'extra = __monty_import__("tools.extra")',
            '/modules/tools/extra.py': 'value = 7',
        }"#,
    );
    run(
        &mut repl,
        r"
value = 900
import left
import right as r
from shared import add as plus, value as library_value
import tools.extra as extra
from tools.extra import value as extra_value
import tools.extra
assert left.answer() == 42
assert r.answer() == 43
assert plus(4) == 44
assert library_value == 40
assert value == 900
assert extra.value == 7
assert extra_value == 7
assert tools.extra is extra
assert reads == [
    '/modules/left.py', '/modules/shared.py', '/modules/right.py',
    '/modules/tools.py', '/modules/tools/extra.py',
]
",
    );
}

#[test]
fn builtins_cannot_be_hijacked_and_module_globals_keep_approved_bindings() {
    let mut repl = repl_with_files(
        r#"{
            '/modules/math.py': 'raise RuntimeError("must not execute")',
            '/modules/json.py': 'raise RuntimeError("must not execute")',
            '/modules/os/path.py': 'raise RuntimeError("must not execute")',
            '/modules/math/plugin.py': 'raise RuntimeError("must not execute")',
            '/modules/library.py': '''from math import sqrt
def answer():
    return host_answer(len([1, 2])) + sqrt(4)''',
        }"#,
    );
    run(
        &mut repl,
        r#"
def host_answer(n):
    return n + 38
__monty_import__ = make_vfs_importer(read_source, {'host_answer': host_answer})
len = 123
make_vfs_importer = 456
import math
import json
import os.path
assert os.path.join('a', 'b') == 'a/b'
try:
    import math.plugin
    assert False, 'builtin namespace hijacked'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'math.plugin'; 'math' is not a package"
import library
assert library.answer() == 42
assert reads == ['/modules/library.py']
"#,
    );
}

#[test]
fn errors_do_not_poison_the_cache_or_loading_stack() {
    let mut repl = repl_with_files(
        r#"{
            '/modules/a.py': 'import b',
            '/modules/b.py': 'import a',
            '/modules/broken.py': 'raise ValueError("bad module")',
        }"#,
    );
    run(
        &mut repl,
        r#"
try:
    import missing
    assert False, 'missing import succeeded'
except ModuleNotFoundError as e:
    assert str(e) == "No virtual module named 'missing'"
try:
    import a
    assert False, 'cycle succeeded'
except ImportError as e:
    assert str(e) == 'circular virtual import: a -> b -> a'
try:
    import broken
    assert False, 'broken import succeeded'
except ValueError as e:
    assert str(e) == 'bad module'
files['/modules/b.py'] = 'answer = 41'
files['/modules/broken.py'] = 'answer = 42'
import a
import broken
assert broken.answer == 42
assert a.b.answer == 41
try:
    from broken import missing
    assert False, 'missing export succeeded'
except ImportError:
    pass
"#,
    );
}

#[test]
fn a_new_loader_rereads_files_but_existing_functions_keep_their_own_globals() {
    let mut repl = repl_with_files(
        r"{'/modules/library.py': '''value = 40
def answer():
    return value + 2'''}",
    );
    run(&mut repl, "from library import answer as old_answer");
    run(
        &mut repl,
        r"
files['/modules/library.py'] = '''value = 50
def answer():
    return value + 2'''
__monty_import__ = make_vfs_importer(read_source, {})
from library import answer
assert answer() == 52
assert old_answer() == 42
assert reads == ['/modules/library.py', '/modules/library.py']
",
    );
}

#[test]
fn no_global_hook_preserves_host_imports() {
    for code in [
        r"
def f():
    __monty_import__ = lambda name: 1
    import missing
f()
",
        "__monty_import__ = lambda name: 1\nexec('import missing', {})",
    ] {
        let runner = MontyRun::new(code.to_owned(), "main.py", vec![], CompileOptions::default()).unwrap();
        let progress = runner
            .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
            .unwrap();
        let call = progress.into_os_call().unwrap();
        assert!(matches!(&call.function_call, OsFunctionCall::Import(name) if name == "missing"));
        let error = call
            .resume(ExtFunctionResult::NotFound(IMPORT_CALL.to_owned()), PrintWriter::Stdout)
            .unwrap_err();
        assert_eq!(error.summary(), "ModuleNotFoundError: No module named 'missing'");
    }
}

#[test]
fn non_function_and_async_hooks_fail_explicitly_without_affecting_builtins() {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    run(
        &mut repl,
        r"
class Loader:
    def load(self, name):
        return 42
async def async_load(name):
    return 42
async def async_defaults(name, answer=42):
    return answer
def make_async_loader(answer):
    async def load(name):
        return answer
    return load
def make_async_defaults(answer):
    async def load(name, offset=2):
        return answer + offset
    return load
for hook in [
    None, 42, len, Loader, Loader(), Loader().load,
    async_load, async_defaults, make_async_loader(42), make_async_defaults(40),
]:
    __monty_import__ = hook
    try:
        import library
        assert False, 'invalid hook succeeded'
    except TypeError as e:
        assert str(e) == '__monty_import__ must be a synchronous sandbox-defined function'
    import math
    assert math.sqrt(4) == 2
",
    );
    let runner = MontyRun::new(
        "import missing".to_owned(),
        "main.py",
        vec!["__monty_import__".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let error = runner
        .start(
            vec![MontyObject::function("host_import", None)],
            ResourceTracker::default(),
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_eq!(
        error.summary(),
        "TypeError: __monty_import__ must be a synchronous sandbox-defined function"
    );
}

#[test]
fn fixture_loader_validates_names_and_rejects_reserved_bindings() {
    let mut repl = repl_with_files("{}");
    run(
        &mut repl,
        r"
for name in ['../secret', '/etc/passwd', 'a..b', 'a/b', '', 'a\\b', 'a\x00b']:
    try:
        __monty_import__(name)
        assert False, 'invalid path accepted'
    except ImportError as e:
        assert str(e) == 'invalid virtual module name: ' + name
assert reads == []
for name in ['__monty_import__', '__name__', '__file__']:
    try:
        make_vfs_importer(read_source, {name: 1})
        assert False, 'reserved binding accepted'
    except ValueError as e:
        assert str(e) == 'bindings contain a reserved module name'
",
    );
}

#[test]
fn imports_can_suspend_in_source_reads_and_host_calls_and_survive_dumps() {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    run(&mut repl, LOADER);
    run(
        &mut repl,
        r"
def read_source(path):
    return fetch_source(path)
def host_answer(n):
    return fetch_answer(n)
__monty_import__ = make_vfs_importer(read_source, {'host_answer': host_answer})
",
    );
    let mut progress = repl
        .feed_start("import library\nlibrary.answer()", vec![], PrintWriter::Stdout)
        .unwrap();
    let mut calls = Vec::new();
    loop {
        let bytes = dump("main.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
        drop(progress.into_repl());
        progress = match Dump::load(&bytes).unwrap().state {
            Session::Suspended(progress) => *progress,
            _ => panic!("expected suspended session"),
        };
        progress = match progress {
            ReplProgress::NameLookup(lookup) => {
                let value = MontyObject::function(lookup.name.clone(), None);
                lookup
                    .resume(NameLookupResult::Value(value), PrintWriter::Stdout)
                    .unwrap()
            }
            ReplProgress::FunctionCall(call) => {
                calls.push(call.function_name.clone());
                let answer = match call.function_name.as_str() {
                    "fetch_source" => MontyObject::string(
                        r"def answer():
    return host_answer(40) + 2",
                    ),
                    "fetch_answer" => MontyObject::int(40),
                    other => panic!("unexpected host call: {other}"),
                };
                call.resume(answer, PrintWriter::Stdout).unwrap()
            }
            ReplProgress::Complete { repl, value } => {
                assert_eq!(value, MontyObject::int(42));
                assert_eq!(calls, ["fetch_source", "fetch_answer"]);
                let bytes = dump("main.py", None, &[], SessionRef::Idle(&repl)).unwrap();
                let mut repl = match Dump::load(&bytes).unwrap().state {
                    Session::Idle(repl) => *repl,
                    _ => panic!("expected idle session"),
                };
                run(&mut repl, "assert __monty_import__('library') is library");
                break;
            }
            other => panic!("unexpected suspension: {other:?}"),
        };
    }
}

#[test]
fn sandbox_hook_with_defaults_also_works_in_one_shot_runs() {
    let code = r"
class Library:
    pass
def __monty_import__(name, answer=42):
    module = Library()
    module.answer = answer
    return module
import library
library.answer
";
    let mut runner = MontyRun::new(code.to_owned(), "main.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::int(42));
}

#[test]
fn source_reads_use_the_existing_filesystem_boundary_and_preserve_permission_errors() {
    for denied in [false, true] {
        let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
        run(&mut repl, LOADER);
        run(
            &mut repl,
            r"
from pathlib import Path
def read_source(path):
    return Path(path).read_text()
__monty_import__ = make_vfs_importer(read_source, {})
",
        );
        let source = if denied {
            r"
try:
    import library
    assert False, 'denied read succeeded'
except PermissionError as e:
    assert str(e) == 'virtual file denied'
"
        } else {
            "import library\nassert library.answer() == 42"
        };
        let progress = repl.feed_start(source, vec![], PrintWriter::Stdout).unwrap();
        let bytes = dump("main.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
        drop(progress.into_repl());
        let Session::Suspended(progress) = Dump::load(&bytes).unwrap().state else {
            panic!("expected suspended session");
        };
        let ReplProgress::OsCall(call) = *progress else {
            panic!("expected filesystem call");
        };
        let OsFunctionCall::ReadText(path) = &call.function_call else {
            panic!("expected virtual text read");
        };
        assert_eq!(path.as_str(), "/modules/library.py");
        let result = if denied {
            ExtFunctionResult::Error(MontyException::new(
                ExcType::PermissionError,
                Some("virtual file denied".to_owned()),
            ))
        } else {
            MontyObject::string("def answer():\n    return 42").into()
        };
        assert!(matches!(
            call.resume(result, PrintWriter::Stdout).unwrap(),
            ReplProgress::Complete { .. }
        ));
    }
}

#[test]
fn module_execution_has_no_implicit_caller_or_host_names() {
    let mut repl = repl_with_files(r"{'/modules/library.py': 'answer = secret'}");
    run(
        &mut repl,
        r#"
secret = 42
try:
    import library
    assert False, 'caller secret leaked'
except NameError as e:
    assert str(e) == "name 'secret' is not defined"
"#,
    );
}

#[test]
fn import_hook_frames_obey_the_existing_recursion_limit() {
    let mut repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    run(
        &mut repl,
        r"def __monty_import__(name):
    import recursive",
    );
    *repl.tracker_mut() = ResourceTracker::new(ResourceLimits::default().max_recursion_depth(5));
    let error = repl
        .feed_run("import recursive", vec![], PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::RecursionError);
}

#[test]
fn exec_globals_find_dynamic_hook_keys_and_retain_function_namespaces() {
    let code = r"
key = ''.join(['__monty', '_import__'])
def loader(name):
    return 42
namespace = {key: loader}
exec('import library\nassert library == 42', namespace)
sources = [
    '''def load(name):
    return answer''',
    '''def load(name, offset=2):
    return answer + offset''',
    '''def make_loader(value):
    def load(name):
        return value
    return load
load = make_loader(answer)''',
    '''def make_loader(value):
    def load(name, offset=2):
        return value + offset
    return load
load = make_loader(answer)''',
]
for source, expected in zip(sources, [40, 42, 40, 42]):
    namespace = {'answer': 40}
    exec(source, namespace)
    namespace[key] = namespace['load']
    local_namespace = {key: None}
    exec('import library\nresult = library', namespace, local_namespace)
    assert local_namespace['result'] == expected
    exec('def run_import():\n    import library\n    return library', namespace)
    assert namespace['run_import']() == expected
    namespace.pop('load')
    assert namespace['run_import']() == expected
";
    let mut runner = MontyRun::new(code.to_owned(), "main.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn exec_globals_reject_async_hooks_under_dynamic_keys() {
    let code = r"
key = ''.join(['__monty', '_import__'])
namespace = {}
exec('async def load(name, answer=42):\n    return answer', namespace)
namespace[key] = namespace['load']
try:
    exec('from library import answer', namespace)
    assert False, 'async hook succeeded'
except TypeError as e:
    assert str(e) == key + ' must be a synchronous sandbox-defined function'
";
    let mut runner = MontyRun::new(code.to_owned(), "main.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn hook_errors_keep_sandbox_frames_and_do_not_request_host_fallback() {
    let code = r"def __monty_import__(name):
    exec('raise ModuleNotFoundError(name)', {'name': name})
import library";
    let runner = MontyRun::new(code.to_owned(), "main.py", vec![], CompileOptions::default()).unwrap();
    let error = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(error);
}
