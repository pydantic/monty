//! The host-facing names of the module OS calls.

use monty_types::{IMPORT_CALL, IMPORT_STUBS_CALL, OsFunctionCall};

/// The constants hosts match an OS call's name against are the names the
/// variants carry, since the host never sees the typed call.
#[test]
fn module_call_names_match_their_variants() {
    assert_eq!(OsFunctionCall::Import("tools".to_owned()).name(), IMPORT_CALL);
    assert_eq!(OsFunctionCall::ImportStubs(vec![]).name(), IMPORT_STUBS_CALL);
}
