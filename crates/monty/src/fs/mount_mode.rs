//! Mount mode definitions and overlay state management.

use std::{collections::BTreeMap, ops::Bound};

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

/// In-memory overlay state for [`MountMode::OverlayMemory`].
///
/// A single [`BTreeMap`] maps relative paths (within the mount) to
/// [`OverlayEntry`] variants describing the current overlay state.
/// Paths not in the map fall through to the real filesystem.
#[derive(Debug, Default)]
pub struct OverlayState {
    /// Entries keyed by forward-slash-separated relative path (e.g., `"subdir/file.txt"`).
    /// The mount root is `""`.
    ///
    /// Uses `BTreeMap` instead of a hash map so that prefix lookups (e.g. finding all
    /// entries under a directory) are O(log n + k) via range queries, rather than O(n) scans.
    entries: BTreeMap<String, OverlayEntry>,
}

impl OverlayState {
    /// Creates a new empty overlay state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up the entry for a relative path, or `None` to fall through to real FS.
    #[must_use]
    pub(super) fn get(&self, relative_path: &str) -> Option<&OverlayEntry> {
        self.entries.get(relative_path)
    }

    /// Inserts or replaces an entry for a relative path.
    pub(super) fn insert(&mut self, relative_path: String, entry: OverlayEntry) {
        self.entries.insert(relative_path, entry);
    }

    /// Iterates over overlay entries whose keys start with `prefix`.
    ///
    /// `prefix` must be either `""` (iterate everything) or end with `'/'`.
    /// Uses [`BTreeMap::range`] for O(log n + k) lookup instead of scanning all entries.
    pub(super) fn prefix_iter(&self, prefix: &str) -> impl Iterator<Item = (&str, &OverlayEntry)> {
        debug_assert!(prefix.is_empty() || prefix.ends_with('/'));

        let (lo, hi) = if prefix.is_empty() {
            (Bound::Unbounded, Bound::Unbounded)
        } else {
            // Increment last byte: '/' (0x2F) → '0' (0x30) to form exclusive upper bound.
            let mut upper = prefix.to_owned();
            upper.pop();
            upper.push('0');
            (Bound::Included(prefix.to_owned()), Bound::Excluded(upper))
        };

        self.entries.range::<String, _>((lo, hi)).map(|(k, v)| (k.as_str(), v))
    }
}

/// An entry in the in-memory overlay layer.
#[derive(Debug)]
pub(super) enum OverlayEntry {
    /// A file written by sandbox code, stored in memory.
    File(OverlayFile),

    /// A directory created by sandbox code.
    Directory {
        /// Modification time as Unix timestamp.
        mtime: f64,
    },

    /// A tombstone hiding a real FS entry from sandbox reads.
    Deleted,
}

/// A file stored in the in-memory overlay layer (raw bytes; text is UTF-8 encoded).
#[derive(Debug)]
pub(super) struct OverlayFile {
    /// The file content as raw bytes.
    pub content: Vec<u8>,
    /// Modification time as Unix timestamp.
    pub mtime: f64,
}
