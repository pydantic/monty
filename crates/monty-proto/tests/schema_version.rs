//! Ties `PROTOCOL_VERSION` to the schema it describes.
//!
//! A protocol version is only worth what the habit of bumping it is worth, and
//! nothing about editing a `.proto` file forces that decision. This snapshots
//! the schema with comments stripped, so any *structural* change fails here and
//! the diff shows exactly what moved — at which point the author decides
//! whether an older peer could mis-read it (bump `PROTOCOL_VERSION`) or not
//! (accept the snapshot). Comment-only edits are free.

use insta::assert_snapshot;

/// The schema with comments and blank lines removed, so the snapshot tracks
/// structure only.
fn schema_structure() -> String {
    include_str!("../proto/monty/v1/monty.proto")
        .lines()
        .map(|line| match line.find("//") {
            Some(start) => &line[..start],
            None => line,
        })
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// If this snapshot changed, decide whether the change is one a peer at an
/// older `PROTOCOL_VERSION` could mis-read.
///
/// Additive changes it can ignore (a new optional field, a new oneof arm it
/// never sends) only need the snapshot accepted. Anything else — removing or
/// repurposing a tag, changing a field's meaning, requiring a new field —
/// needs `PROTOCOL_VERSION` bumped first, and `MIN_SUPPORTED_PROTOCOL_VERSION`
/// raised if the old version can no longer be served.
#[test]
fn protocol_version_matches_schema() {
    assert_snapshot!(schema_structure());
}
