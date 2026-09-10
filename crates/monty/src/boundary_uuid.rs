//! Worker-side uuid generation for sandbox-defined classes and instances.
//!
//! Sandbox objects need boundary identities generated where they live — in the
//! worker — so ids never encode addresses and stay stable for the life of the
//! heap object (dumps included). A uuid is generated only when a value crosses
//! to the host; sandboxed code has no way to observe these ids, so this is not
//! an entropy side-channel into the sandbox.

use monty_types::MontyUuid;

/// Generates a fresh uuid4 from OS entropy.
///
/// # Panics
/// If the OS entropy source fails — unrecoverable, and a worker panic is
/// treated as a crash by the pool, which replaces the process.
pub(crate) fn create_uuid() -> MontyUuid {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS entropy source unavailable");
    MontyUuid::from_random_bytes(bytes)
}
