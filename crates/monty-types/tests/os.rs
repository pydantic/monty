//! The host-facing names of the OS calls hosts answer before their `os` handlers.

use monty_types::{GET_TYPE_HINTS_CALL, IMPORT_CALL, IMPORT_STUBS_CALL, ImportArgs, OsFunctionCall, TypeHintsTarget};

#[test]
fn host_call_names_match_their_variants() {
    assert_eq!(OsFunctionCall::Import(ImportArgs::new("tools")).name(), IMPORT_CALL);
    assert_eq!(OsFunctionCall::ImportStubs(vec![]).name(), IMPORT_STUBS_CALL);
    let hints = OsFunctionCall::GetTypeHints(TypeHintsTarget::Function("f".to_owned()));
    assert_eq!(hints.name(), GET_TYPE_HINTS_CALL);
}
