//! Public mount mode definitions.
//!
//! The public API only needs to describe the access policy for a mount. The
//! in-memory overlay storage lives in [`super::overlay_state`] so the public
//! enum can stay focused on behavior rather than internal data layout.

use super::overlay_state::OverlayState;

/// Access policy for a mount point.
///
/// Controls what operations sandbox code can perform on files within the mounted
/// directory. The overlay modes provide copy-on-write semantics where reads fall
/// through to the real directory but writes are captured separately.
///
/// Regardless of mode, path traversal and symlink escape protection is always enforced.
#[derive(Debug)]
pub enum MountMode {
    /// Full read and write access to the host directory.
    /// Use with caution — sandbox code can modify real files.
    ReadWrite,

    /// Read-only access. Write operations raise `PermissionError`.
    ReadOnly,

    /// Copy-on-write overlay backed by in-memory storage.
    ///
    /// Reads fall through to the host directory. Writes are captured in the
    /// contained [`OverlayState`]. Deletions insert [`OverlayEntry::Deleted`]
    /// tombstones that hide real files from subsequent reads. Directory listings
    /// merge real and overlay entries, with overlay taking precedence.
    OverlayMemory(OverlayState),
}

impl MountMode {
    /// Parses a mode string into a [`MountMode`].
    ///
    /// Accepted values: `"read-only"`, `"read-write"`, `"overlay"`.
    /// Returns a descriptive error string on invalid input.
    pub fn from_mode_str(mode: &str) -> Result<Self, String> {
        match mode {
            "read-only" => Ok(Self::ReadOnly),
            "read-write" => Ok(Self::ReadWrite),
            "overlay" => Ok(Self::OverlayMemory(OverlayState::new())),
            other => Err(format!(
                "Invalid mode '{other}', expected 'read-only', 'read-write', or 'overlay'"
            )),
        }
    }

    /// Returns a short string label for this mode (`"read-write"`, `"read-only"`,
    /// or `"overlay"`).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadWrite => "read-write",
            Self::ReadOnly => "read-only",
            Self::OverlayMemory(_) => "overlay",
        }
    }
}
