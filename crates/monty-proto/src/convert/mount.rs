//! Builds a child-local `MountTable` from wire `Mount` entries.
//!
//! All path security (canonicalization, symlink containment, boundary checks)
//! is enforced by `MountTable::mount` itself — this module only translates
//! the wire representation and must never add its own path handling.

use monty::fs::{MountMode, MountTable, OverlayState};

use crate::{convert::ProtoConvertError, pb};

/// Builds a `MountTable` from wire mounts; `Ok(None)` when `mounts` is empty
/// (execution then has no filesystem and every OS call bubbles to the parent).
pub fn build_mount_table(mounts: Vec<pb::Mount>) -> Result<Option<MountTable>, ProtoConvertError> {
    if mounts.is_empty() {
        return Ok(None);
    }
    let mut table = MountTable::new();
    for mount in mounts {
        let mode = match mount.mode() {
            pb::MountMode::ReadOnly => MountMode::ReadOnly,
            pb::MountMode::ReadWrite => MountMode::ReadWrite,
            // Overlay state is created fresh per table: writes live only as
            // long as the session and are discarded with it.
            pb::MountMode::Overlay => MountMode::OverlayMemory(OverlayState::new()),
            pb::MountMode::Unspecified => {
                return Err(ProtoConvertError::InvalidMount(format!(
                    "mount {:?} has no mode",
                    mount.virtual_path
                )));
            }
        };
        table
            .mount(&mount.virtual_path, &mount.host_path, mode, mount.write_bytes_limit)
            .map_err(|e| {
                // Strip the "invalid mount: " prefix MountError already carries,
                // since ProtoConvertError::InvalidMount re-adds it.
                let msg = e.to_string();
                let msg = match msg.strip_prefix("invalid mount: ") {
                    Some(stripped) => stripped.to_owned(),
                    None => msg,
                };
                ProtoConvertError::InvalidMount(msg)
            })?;
    }
    Ok(Some(table))
}
