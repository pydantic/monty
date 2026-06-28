//! Validation for Python package requirement strings carried by the protocol.
//!
//! Requirement strings are eventually appended as individual argv entries to
//! `uv pip install`. A valid PEP 508 requirement is data, not a command-line
//! option, so protocol clients and workers share this guard before invoking uv.

/// Rejects a requirement string that uv would interpret as a command-line
/// option rather than a package specifier.
///
/// A valid PEP 508 requirement never begins with `-`, so a string that does
/// (e.g. `--index-url=…`, `-r /etc/hosts`, `-e .`) would be smuggled onto uv's
/// command line as a flag. Empty/whitespace-only entries are also rejected
/// since uv has no use for them and they only signal caller confusion.
pub fn validate_requirement(requirement: &str) -> Result<(), String> {
    let trimmed = requirement.trim();
    let problem = if trimmed.is_empty() {
        "must not be empty"
    } else if trimmed.starts_with('-') {
        "must not start with '-' (it would be parsed as a uv option)"
    } else {
        return Ok(());
    };
    Err(format!("invalid requirement {requirement:?}: {problem}"))
}
