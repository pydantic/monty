//! Tests for importing a module the sandbox does not have: the import
//! suspends as an `import` OS call, and the host's answer is the module.

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, ReplProgress, RunProgress, Session, SessionRef, builtin_module_names, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, IMPORT_CALL, MontyException, MontyObject, MontyUuid, NameLookupResult,
    OsFunctionCall, PrintWriter, ResourceTracker, SANDBOX_MODULE_NAMES,
};

fn start(code: &str) -> RunProgress {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
}

/// Builds a host module with the given attributes.
fn host_module(name: &str, id: u128, attrs: Vec<(&str, MontyObject)>) -> MontyObject {
    MontyObject::module(
        name,
        MontyUuid::from_u128(id),
        attrs.into_iter().map(|(k, v)| (MontyObject::string(k), v)),
    )
}

/// Builds a host object answering an import, whose missing attributes the
/// sandbox looks up lazily on the host.
fn host_object(name: &str, id: u128, attrs: Vec<(&str, MontyObject)>) -> MontyObject {
    MontyObject::class_instance(
        MontyObject::class_type(name, MontyUuid::from_u128(id), true, true, []),
        MontyUuid::from_u128(id + 100),
        attrs.into_iter().map(|(k, v)| (MontyObject::string(k), v)),
    )
}

/// Answers the `import` OS call `progress` is suspended at, checking it names `module`.
fn answer_import(progress: RunProgress, module: &str, value: MontyObject) -> RunProgress {
    try_answer_import(progress, module, value).unwrap()
}

/// The last traceback line of the error `code` raises once its import of `module` is answered.
fn import_error(code: &str, module: &str, value: MontyObject) -> String {
    let err = try_answer_import(start(code), module, value).unwrap_err();
    err.to_string().lines().last().unwrap().to_owned()
}

fn try_answer_import(progress: RunProgress, module: &str, value: MontyObject) -> Result<RunProgress, MontyException> {
    let call = progress.into_os_call().unwrap();
    assert_eq!(call.function_call.name(), IMPORT_CALL);
    assert!(matches!(&call.function_call, OsFunctionCall::Import(name) if name == module));
    assert!(!call.allow_eager_await);
    call.resume(value, PrintWriter::Stdout)
}

#[test]
fn an_unknown_import_suspends_and_binds_the_answer() {
    let progress = start("import tools\ntools.x + 1");
    let done = answer_import(
        progress,
        "tools",
        host_module("tools", 1, vec![("x", MontyObject::int(41))]),
    );
    assert_eq!(done.into_complete(), Some(MontyObject::int(42)));
}

#[test]
fn a_host_module_is_a_module_object() {
    let code = "import tools\n(type(tools).__name__, repr(tools), hasattr(tools, '__name__'))";
    let done = answer_import(start(code), "tools", host_module("tools", 1, vec![]));
    assert_eq!(
        done.into_complete(),
        Some(MontyObject::tuple([
            MontyObject::string("module"),
            MontyObject::string("<module 'tools'>"),
            MontyObject::bool(false),
        ]))
    );

    assert_eq!(
        import_error("import tools\ntools.nope", "tools", host_module("tools", 1, vec![])),
        "AttributeError: module 'tools' has no attribute 'nope'"
    );
    assert_eq!(
        import_error("import tools\ntools()", "tools", host_module("tools", 1, vec![])),
        "TypeError: 'module' object is not callable"
    );
}

#[test]
fn a_host_module_exports_with_its_id() {
    let module = host_module("tools", 1, vec![("x", MontyObject::int(1))]);
    let done = answer_import(start("import tools\ntools"), "tools", module.clone());
    assert_eq!(done.into_complete(), Some(module));
}

#[test]
fn from_import_loads_names_from_the_answer() {
    let progress = start("from tools import x, y as z\nx * z");
    let module = host_module("tools", 1, vec![("x", MontyObject::int(6)), ("y", MontyObject::int(7))]);
    let done = answer_import(progress, "tools", module);
    assert_eq!(done.into_complete(), Some(MontyObject::int(42)));
}

#[test]
fn a_dotted_import_walks_submodules_of_the_answer() {
    let nested = |id| host_module("a.b", id, vec![("c", MontyObject::int(6))]);
    let module = |id| host_module("a", id, vec![("b", nested(id + 10))]);

    let done = answer_import(start("import a.b\na.b.c"), "a", module(1));
    assert_eq!(done.into_complete(), Some(MontyObject::int(6)));
    let done = answer_import(start("import a.b as m\nm.c * 2"), "a", module(1));
    assert_eq!(done.into_complete(), Some(MontyObject::int(12)));
    let done = answer_import(start("from a.b import c\nc + 1"), "a", module(1));
    assert_eq!(done.into_complete(), Some(MontyObject::int(7)));
    let done = answer_import(start("from a import b\nb.c"), "a", module(1));
    assert_eq!(done.into_complete(), Some(MontyObject::int(6)));
}

#[test]
fn a_missing_submodule_is_a_module_not_found_error() {
    let module = host_module("a", 1, vec![("b", MontyObject::int(1))]);
    for code in ["import a.nope.deeper", "from a.nope import x"] {
        assert_eq!(
            import_error(code, "a", module.clone()),
            "ModuleNotFoundError: No module named 'a.nope'"
        );
    }
    // a host module is a package: an attribute that is not a module is not a submodule
    assert_eq!(
        import_error("import a.b", "a", module),
        "ModuleNotFoundError: No module named 'a.b'"
    );
}

/// A host object (not a module) answering an import keeps lazy attribute lookups.
#[test]
fn a_host_object_answer_looks_up_missing_submodules_lazily() {
    let module = host_object("a", 1, vec![("b", MontyObject::int(1))]);
    let lookup = answer_import(start("import a.nope.deeper"), "a", module)
        .into_name_lookup()
        .unwrap();
    assert_eq!(lookup.name, "nope");
    let err = lookup
        .resume(NameLookupResult::Undefined, PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(
        err.to_string().lines().last(),
        Some("ModuleNotFoundError: No module named 'a.nope'")
    );
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

/// `not_found`, and a host with no handler at all, both raise `ModuleNotFoundError`
/// once the import reads no file for the module either (see `file_import.rs`).
#[test]
fn a_not_found_answer_is_a_module_not_found_error() {
    let call = start("import nope").into_os_call().unwrap();
    let progress = call
        .resume(ExtFunctionResult::NotFound(IMPORT_CALL.to_owned()), PrintWriter::Stdout)
        .unwrap();
    let err = read_no_files(progress).unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        import nope
    ModuleNotFoundError: No module named 'nope'
    "#);

    let call = start("import nope").into_os_call().unwrap();
    let progress = call
        .resume_with(PrintWriter::Stdout, |call| call.no_handler_result())
        .unwrap();
    let err = read_no_files(progress).unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        import nope
    ModuleNotFoundError: No module named 'nope'
    "#);
}

/// Answers each read of a module's candidate files as missing, with the error that follows.
fn read_no_files(mut progress: RunProgress) -> Result<RunProgress, MontyException> {
    while let RunProgress::OsCall(call) = progress {
        assert!(matches!(call.function_call, OsFunctionCall::ReadText(_)));
        let missing = MontyException::new(ExcType::FileNotFoundError, None);
        progress = call.resume(ExtFunctionResult::Error(missing), PrintWriter::Stdout)?;
    }
    Ok(progress)
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
    let ReplProgress::OsCall(call) = repl
        .feed_start("import tools", Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the import to suspend");
    };
    assert!(matches!(&call.function_call, OsFunctionCall::Import(name) if name == "tools"));
    let module = host_module("tools", 1, vec![("x", MontyObject::int(2))]);
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

/// A host module's name and id persist in a dump: the restored object is still a
/// module and exports back as the module that was imported.
#[test]
fn a_dumped_host_module_restores_as_the_same_module() {
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let ReplProgress::OsCall(call) = repl
        .feed_start("import tools", Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the import to suspend");
    };
    let module = host_module("tools", 1, vec![("x", MontyObject::int(2))]);
    let ReplProgress::Complete { repl, .. } = call.resume(module.clone(), PrintWriter::Stdout).unwrap() else {
        panic!("expected the snippet to complete");
    };
    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(repl) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped while idle");
    };
    let code = "(type(tools).__name__, repr(tools), tools.x, tools)";
    let ReplProgress::Complete { repl, value } = repl
        .feed_start(code, Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the snippet to complete");
    };
    assert_eq!(
        value,
        MontyObject::tuple([
            MontyObject::string("module"),
            MontyObject::string("<module 'tools'>"),
            MontyObject::int(2),
            module,
        ])
    );
    let err = repl
        .feed_start("tools.nope", Vec::<(String, MontyObject)>::new(), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(
        err.error.to_string().lines().last(),
        Some("AttributeError: module 'tools' has no attribute 'nope'")
    );
}

/// The frame must retain the module name across a dump between two lazy attribute lookups.
#[test]
fn a_restored_from_import_still_names_its_module() {
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let ReplProgress::OsCall(call) = repl
        .feed_start(
            "from tools import a, b",
            Vec::<(String, MontyObject)>::new(),
            PrintWriter::Stdout,
        )
        .unwrap()
    else {
        panic!("expected the import to suspend");
    };
    // no eager attrs, so each name is a lazy lookup the host answers
    let progress = call
        .resume(host_object("tools", 1, vec![]), PrintWriter::Stdout)
        .unwrap();
    let ReplProgress::NameLookup(lookup) = &progress else {
        panic!("expected the lookup of `a`, got {progress:?}");
    };
    assert_eq!(lookup.name, "a");
    let bytes = dump("test.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
    // release the in-flight state of the dumped original; the restored copy carries on
    drop(progress.into_name_lookup().unwrap().into_repl());
    let Session::Suspended(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped while suspended");
    };
    let progress = progress
        .into_name_lookup()
        .unwrap()
        .resume(NameLookupResult::Value(MontyObject::int(1)), PrintWriter::Stdout)
        .unwrap();
    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "b");
    let err = lookup
        .resume(NameLookupResult::Undefined, PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.error.to_string(), @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in <module>
        from tools import a, b
    ImportError: cannot import name 'b' from 'tools' (unknown location)
    "#);
}

#[test]
fn a_future_answer_to_an_import_is_refused() {
    let call = start("import tools").into_os_call().unwrap();
    let call_id = call.call_id;
    let err = call
        .resume(ExtFunctionResult::Future(call_id), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        import tools
        ~~~~~~~~~~~~
    RuntimeError: import of 'tools' cannot be answered with a future
    "#);
}

#[test]
fn a_host_exception_hides_its_caret_only_at_an_import() {
    let boom = || MontyException::new(ExcType::ImportError, Some("boom".to_owned()));
    let call = start("import tools").into_os_call().unwrap();
    let err = call
        .resume(ExtFunctionResult::Error(boom()), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        import tools
    ImportError: boom
    "#);

    let call = start("fetch()").into_function_call().unwrap();
    assert_eq!(call.function_name, "fetch");
    let err = call
        .resume(ExtFunctionResult::Error(boom()), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        fetch()
        ~~~~~~~
    ImportError: boom
    "#);
}

/// Spelling `__import__` cannot produce an import OS call.
#[test]
fn a_direct_dunder_import_is_an_ordinary_undefined_name() {
    let call = start("__import__('tools')").into_function_call().unwrap();
    assert_eq!(call.function_name, "__import__");
    let err = call
        .resume(
            ExtFunctionResult::NotFound("__import__".to_owned()),
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        __import__('tools')
        ~~~~~~~~~~~~~~~~~~~
    NameError: name '__import__' is not defined
    "#);

    let err = MontyRun::new(
        "__import__('tools')".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap()
    .run(vec![], ResourceTracker::default(), PrintWriter::Stdout)
    .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        __import__('tools')
        ~~~~~~~~~~
    NameError: name '__import__' is not defined
    "#);

    // a REPL asks the host to look the bare name up, as for any other
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let ReplProgress::NameLookup(lookup) = repl
        .feed_start(
            "f = __import__",
            Vec::<(String, MontyObject)>::new(),
            PrintWriter::Stdout,
        )
        .unwrap()
    else {
        panic!("expected the name to be looked up");
    };
    assert_eq!(lookup.name, "__import__");
    let err = lookup
        .resume(NameLookupResult::Undefined, PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.error.to_string(), @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in <module>
        f = __import__
            ~~~~~~~~~~
    NameError: name '__import__' is not defined
    "#);
}

/// Dotted built-ins such as `os.path` are excluded because host module names must be identifiers.
#[test]
fn every_builtin_module_is_a_reserved_module_name() {
    for name in builtin_module_names() {
        assert!(
            name.contains('.') || SANDBOX_MODULE_NAMES.contains(&name),
            "built-in module {name} is missing from SANDBOX_MODULE_NAMES"
        );
    }
}
