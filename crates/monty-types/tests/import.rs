//! The host side of an `import` call: its arguments, the candidate files a
//! search reads, and how each read is classified.

use monty_types::{
    ExcType, ExtFunctionResult, ImportArgs, ModuleFile, ModuleRead, MontyException, MontyObject, MontyPath,
    OsFunctionCall, find_module_file,
};

fn args(module: &str, search_path: &[&str]) -> ImportArgs {
    ImportArgs {
        module: module.to_owned(),
        search_path: search_path.iter().map(|dir| (*dir).to_owned()).collect(),
    }
}

#[test]
fn candidates_put_the_package_first_in_each_directory() {
    assert_eq!(
        args("pkg.sub", &["/a", "/b/"]).candidates().collect::<Vec<_>>(),
        ["/a/sub/__init__.py", "/a/sub.py", "/b/sub/__init__.py", "/b/sub.py"]
    );
    assert_eq!(
        args("top", &["/"]).candidates().collect::<Vec<_>>(),
        ["/top/__init__.py", "/top.py"]
    );
}

/// The call comes from an untrusted sandbox: nothing outside a plain search is read.
#[test]
fn candidates_reject_untrusted_names_and_directories() {
    assert_eq!(
        args("../etc", &["/a"]).candidates().collect::<Vec<_>>(),
        Vec::<String>::new()
    );
    assert_eq!(
        args("a/b", &["/a"]).candidates().collect::<Vec<_>>(),
        Vec::<String>::new()
    );
    assert_eq!(
        args("pkg..sub", &["/a"]).candidates().collect::<Vec<_>>(),
        Vec::<String>::new()
    );
    assert_eq!(
        args("m", &["relative", "/nul\0", "/ok"])
            .candidates()
            .collect::<Vec<_>>(),
        ["/ok/m/__init__.py", "/ok/m.py"]
    );
}

/// Candidates keep the directory as sent, so a mount checks its length and
/// depth before collapsing `..`; an `os` handler sees them normalized.
#[test]
fn candidates_keep_the_directory_as_sent() {
    let search = args("secrets", &["/allowed/../private"]);
    let candidates = search.candidates().collect::<Vec<_>>();
    assert_eq!(
        candidates,
        [
            "/allowed/../private/secrets/__init__.py",
            "/allowed/../private/secrets.py"
        ]
    );
    let handler_args = OsFunctionCall::ReadText(MontyPath::new(candidates[1].clone())).to_args();
    let path = handler_args.args().next().unwrap().to_owned();
    assert_eq!(path, MontyObject::path("/private/secrets.py".to_owned()));
}

/// A long name across many directories is expanded one directory at a time,
/// never all at once.
#[test]
fn candidates_are_built_lazily() {
    let name = "m".repeat(100_000);
    let search = ImportArgs {
        module: name.clone(),
        search_path: vec!["/".to_owned(); 10_000],
    };
    let first = search.candidates().next();
    assert_eq!(first, Some(format!("/{name}/__init__.py")));
}

#[test]
fn call_args_round_trip() {
    let import = args("pkg.sub", &["/proj/pkg"]);
    let call_args = OsFunctionCall::Import(import.clone()).to_args();
    assert_eq!(ImportArgs::from_call_args(&call_args), Some(import));

    // a sandbox predating file imports sends the name alone
    let mut legacy = monty_types::CallArgs::new();
    legacy.push_arg(MontyObject::string("tools"));
    assert_eq!(ImportArgs::from_call_args(&legacy), Some(ImportArgs::new("tools")));
}

/// A dump written before file imports stores the module name alone.
#[test]
fn a_legacy_dump_loads_with_no_search_path() {
    let legacy: OsFunctionCall = serde_json::from_str(r#"{"Import":"tools"}"#).unwrap();
    let OsFunctionCall::Import(import) = legacy else {
        panic!("expected an import");
    };
    assert_eq!(import, ImportArgs::new("tools"));

    let current = serde_json::to_string(&OsFunctionCall::Import(args("m", &["/a"]))).unwrap();
    let OsFunctionCall::Import(import) = serde_json::from_str(&current).unwrap() else {
        panic!("expected an import");
    };
    assert_eq!(import, args("m", &["/a"]));
}

#[test]
fn the_search_classifies_each_read() {
    let search = args("m", &["/a", "/b"]);
    let mut read = Vec::new();
    let found = find_module_file(&search, |path| {
        read.push(path.to_owned());
        match path {
            "/a/m/__init__.py" => ModuleRead::NotHandled,
            "/a/m.py" => ModuleRead::from_exception(MontyException::new(ExcType::PermissionError, None)),
            "/b/m/__init__.py" => ModuleRead::Missing,
            _ => ModuleRead::Text("x = 1".to_owned()),
        }
    });
    assert_eq!(read, ["/a/m/__init__.py", "/a/m.py", "/b/m/__init__.py", "/b/m.py"]);
    let Some(ExtFunctionResult::ModuleFile(file)) = found else {
        panic!("expected the module file");
    };
    assert_eq!(
        file,
        ModuleFile {
            path: "/b/m.py".to_owned(),
            source: "x = 1".to_owned()
        }
    );

    // any other error ends the search, raised at the import
    let io_error = MontyException::new(ExcType::OSError, Some("disk".to_owned()));
    let found = find_module_file(&search, |_| ModuleRead::from_exception(io_error.clone()));
    assert!(matches!(found, Some(ExtFunctionResult::Error(exc)) if exc == io_error));

    // a missing module is no answer: the host replies not-found
    assert!(find_module_file(&search, |_| ModuleRead::Missing).is_none());
}

#[test]
fn a_read_must_answer_with_text() {
    let read = ModuleRead::from_result(MontyObject::bytes(b"x".to_vec()).into());
    let ModuleRead::Error(exc) = read else {
        panic!("expected an error");
    };
    assert_eq!(
        exc.message(),
        Some("invalid return type: Path.read_text for an import must return a str, not bytes")
    );
}
