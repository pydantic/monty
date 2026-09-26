//! Tests for importing a module the sandbox does not have: the import
//! suspends as an `__import__` host call, and the host's answer is the module.

use insta::assert_snapshot;
use monty::{MontyRepl, MontyRun, ReplProgress, RunProgress};
use monty_types::{
    CompileOptions, ExtFunctionResult, IMPORT_FUNCTION, MontyObject, MontyUuid, NameLookupResult, PrintWriter,
    ResourceTracker,
};

/// Starts `code` as a one-shot run.
fn start(code: &str) -> RunProgress {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
}

/// A host object standing in for a module: an instance of a host class
/// `name` (uuid `id`) with `attrs` as its eager attributes.
fn host_object(name: &str, id: u128, attrs: Vec<(&str, MontyObject)>) -> MontyObject {
    MontyObject::class_instance(
        MontyObject::class_type(name, MontyUuid::from_u128(id), true, true, []),
        MontyUuid::from_u128(id + 100),
        attrs.into_iter().map(|(k, v)| (MontyObject::string(k), v)),
    )
}

/// Answers the `__import__` call `progress` is suspended at, checking it names `module`.
fn answer_import(progress: RunProgress, module: &str, value: MontyObject) -> RunProgress {
    let call = progress.into_function_call().unwrap();
    assert_eq!(call.function_name, IMPORT_FUNCTION);
    assert_eq!(call.object_id, None);
    let args: Vec<_> = call.args.args().map(|arg| arg.to_owned()).collect();
    assert_eq!(args, vec![MontyObject::string(module)]);
    assert_eq!(call.args.kwargs().len(), 0);
    call.resume(value, PrintWriter::Stdout).unwrap()
}

#[test]
fn an_unknown_import_suspends_and_binds_the_answer() {
    let progress = start("import tools\ntools.x + 1");
    let done = answer_import(
        progress,
        "tools",
        host_object("tools", 1, vec![("x", MontyObject::int(41))]),
    );
    assert_eq!(done.into_complete(), Some(MontyObject::int(42)));
}

#[test]
fn from_import_loads_names_from_the_answer() {
    let progress = start("from tools import x, y as z\nx * z");
    let module = host_object("tools", 1, vec![("x", MontyObject::int(6)), ("y", MontyObject::int(7))]);
    let done = answer_import(progress, "tools", module);
    assert_eq!(done.into_complete(), Some(MontyObject::int(42)));
}

#[test]
fn a_missing_name_is_an_import_error_naming_the_module() {
    let progress = start("from tools import missing");
    let progress = answer_import(progress, "tools", host_object("tools", 1, vec![]));
    // the host object has no eager `missing`, so the sandbox asks for it lazily
    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "missing");
    assert!(lookup.object_id().is_some());
    let err = lookup
        .resume(NameLookupResult::Undefined, PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        from tools import missing
    ImportError: cannot import name 'missing' from 'tools' (unknown location)
    "#);
}

#[test]
fn a_not_found_answer_is_a_module_not_found_error() {
    let call = start("import nope").into_function_call().unwrap();
    let err = call
        .resume(
            ExtFunctionResult::NotFound(IMPORT_FUNCTION.to_owned()),
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        import nope
    ModuleNotFoundError: No module named 'nope'
    "#);
}

#[test]
fn a_run_with_no_host_raises_module_not_found_error() {
    let err = MontyRun::new("import nope".to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        import nope
    ModuleNotFoundError: No module named 'nope'
    "#);
}

#[test]
fn calling_a_host_object_suspends_as_dunder_call() {
    let progress = start("import tools\ntools.tool(1, k=2)");
    let tool = host_object("Tool", 2, vec![("name", MontyObject::string("tool"))]);
    let progress = answer_import(progress, "tools", host_object("tools", 1, vec![("tool", tool)]));
    let call = progress.into_function_call().unwrap();
    assert_eq!(call.function_name, "__call__");
    assert_eq!(call.object_id, Some(MontyUuid::from_u128(102)));
    let args: Vec<_> = call.args.args().map(|arg| arg.to_owned()).collect();
    assert_eq!(args, vec![MontyObject::int(1)]);
    let kwargs: Vec<_> = call.args.kwargs().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
    assert_eq!(kwargs, vec![(MontyObject::string("k"), MontyObject::int(2))]);
    let done = call.resume(MontyObject::int(3), PrintWriter::Stdout).unwrap();
    assert_eq!(done.into_complete(), Some(MontyObject::int(3)));
}

#[test]
fn a_repl_import_suspends_and_the_module_persists() {
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let ReplProgress::FunctionCall(call) = repl
        .feed_start("import tools", Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the import to suspend");
    };
    assert_eq!(call.function_name, IMPORT_FUNCTION);
    let module = host_object("tools", 1, vec![("x", MontyObject::int(2))]);
    let ReplProgress::Complete { repl, .. } = call.resume(module, PrintWriter::Stdout).unwrap() else {
        panic!("expected the snippet to complete");
    };
    // the bound module is ordinary session state
    let ReplProgress::Complete { value, .. } = repl
        .feed_start("tools.x * 21", Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the snippet to complete");
    };
    assert_eq!(value, MontyObject::int(42));
}
