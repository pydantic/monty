//! The host-facing names of the OS calls hosts answer before their `os` handlers.

use monty_types::{
    CreateExclusiveCallArgs, FileMode, GET_TYPE_HINTS_CALL, IMPORT_CALL, IMPORT_STUBS_CALL, ImportArgs, OsFunctionCall,
    TypeHintsTarget,
};

#[test]
fn host_call_names_match_their_variants() {
    assert_eq!(OsFunctionCall::Import(ImportArgs::new("tools")).name(), IMPORT_CALL);
    assert_eq!(OsFunctionCall::ImportStubs(vec![]).name(), IMPORT_STUBS_CALL);
    let hints = OsFunctionCall::GetTypeHints(TypeHintsTarget::Function("f".to_owned()));
    assert_eq!(hints.name(), GET_TYPE_HINTS_CALL);
}

#[test]
fn exclusive_creation_routes_as_a_write_and_projects_callback_arguments() {
    let mut call = OsFunctionCall::CreateExclusive(CreateExclusiveCallArgs {
        path: "/mnt/sub/../new".into(),
        mode: FileMode::Read(false),
        permissions: 0o600,
    });
    assert!(call.is_write());
    assert_eq!(call.name(), "Path.create_exclusive");
    assert_eq!(call.fs_primary_path(), Some("/mnt/sub/../new"));
    assert!(!call.has_empty_path());
    assert_eq!(call.fs_paths_mut().count(), 1);
    let args = call.to_args();
    assert_eq!(args.args().len(), 2);
    assert_eq!(
        args.arg(0).unwrap().to_owned(),
        monty_types::MontyObject::path("/mnt/new".to_owned())
    );
    assert_eq!(args.arg(1).unwrap().as_str(), Some("r"));
    assert_eq!(args.kwarg("permissions").unwrap().as_int(), Some(0o600));
}

#[test]
fn exclusive_creation_checks_empty_and_null_paths() {
    let call = |path: &str| {
        OsFunctionCall::CreateExclusive(CreateExclusiveCallArgs {
            path: path.into(),
            mode: FileMode::Write(false),
            permissions: 0o600,
        })
    };
    assert!(call("").has_empty_path());
    assert_eq!(call("/mnt/a\0b").check_path_null_bytes(), Err("embedded null byte"));
}
