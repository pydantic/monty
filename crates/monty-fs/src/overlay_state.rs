//! Internal storage for in-memory overlay mounts.
//!
//! This module keeps the overlay data structures separate from the public
//! [`MountMode`](super::MountMode) definition so the public API stays easy to
//! scan while the storage internals can evolve independently.

use std::{
    collections::BTreeMap,
    fs, mem,
    ops::Bound,
    path::{Path, PathBuf},
    time::SystemTime,
};

use super::{MountError, common::as_u64};

/// Conservative bookkeeping charge for each overlay map entry.
///
/// This covers the map node, key allocation, and entry metadata. Variable-size
/// file contents and host paths are charged separately.
pub(super) const ENTRY_MEMORY_USAGE: u64 = 256;

/// In-memory overlay state for [`super::MountMode::OverlayMemory`].
///
/// A single [`BTreeMap`] stores relative mount paths and the overlay entry that
/// currently shadows or extends the underlying real filesystem.
#[derive(Debug, Default)]
pub struct OverlayState {
    /// Entries keyed by forward-slash-separated relative path (e.g.
    /// `"subdir/file.txt"`). The mount root is represented by `""`.
    ///
    /// [`BTreeMap`] is used so prefix walks for directory operations can stay
    /// `O(log n + k)` rather than scanning the entire overlay.
    entries: BTreeMap<String, OverlayEntry>,
    /// Estimated live bytes retained by `entries`.
    memory_usage: u64,
}

impl OverlayState {
    /// Creates a new empty overlay state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up the overlay entry for `relative_path`.
    #[must_use]
    pub(super) fn get(&self, relative_path: &str) -> Option<&OverlayEntry> {
        self.entries.get(relative_path)
    }

    /// Returns the estimated live memory retained by this overlay.
    #[must_use]
    pub(super) fn memory_usage(&self) -> u64 {
        self.memory_usage
    }

    /// Removes and returns the entry for `relative_path`.
    pub(super) fn remove(&mut self, relative_path: &str) -> Option<OverlayEntry> {
        let entry = self.entries.remove(relative_path)?;
        self.memory_usage = self
            .memory_usage
            .saturating_sub(entry_memory_usage(relative_path, &entry));
        Some(entry)
    }

    /// Inserts an entry if the resulting overlay stays within `limit`.
    pub(super) fn insert(&mut self, relative_path: String, entry: OverlayEntry, limit: u64) -> Result<(), MountError> {
        let projected = self.projected_usage(&relative_path, &entry);
        if projected > limit {
            Err(MountError::MemoryUsageLimitExceeded(limit))
        } else {
            self.entries.insert(relative_path, entry);
            self.memory_usage = projected;
            Ok(())
        }
    }

    /// Inserts an entry after the caller has preflighted a multi-entry update.
    pub(super) fn insert_unchecked(&mut self, relative_path: String, entry: OverlayEntry) {
        self.memory_usage = self.projected_usage(&relative_path, &entry);
        self.entries.insert(relative_path, entry);
    }

    /// Returns total retained usage as if `entry` replaced `relative_path`.
    fn projected_usage(&self, relative_path: &str, entry: &OverlayEntry) -> u64 {
        let old_usage = self
            .entries
            .get(relative_path)
            .map_or(0, |old| entry_memory_usage(relative_path, old));
        let new_usage = entry_memory_usage(relative_path, entry);
        self.memory_usage.saturating_sub(old_usage).saturating_add(new_usage)
    }

    /// Appends bytes to an overlay file while accounting for retained content.
    pub(super) fn append_file(
        &mut self,
        relative_path: &str,
        data: &[u8],
        mtime: f64,
        limit: u64,
    ) -> Result<bool, MountError> {
        let Some(OverlayEntry::File(file)) = self.entries.get_mut(relative_path) else {
            return Ok(false);
        };
        let projected = self.memory_usage.saturating_add(as_u64(data.len()));
        if projected > limit {
            Err(MountError::MemoryUsageLimitExceeded(limit))
        } else {
            file.content.extend_from_slice(data);
            file.mtime = mtime;
            self.memory_usage = projected;
            Ok(true)
        }
    }

    /// Checks replacing `relative_path` with a file of `content_len` bytes.
    pub(super) fn check_file_replacement(
        &self,
        relative_path: &str,
        content_len: usize,
        limit: u64,
    ) -> Result<(), MountError> {
        let old_usage = self
            .entries
            .get(relative_path)
            .map_or(0, |old| entry_memory_usage(relative_path, old));
        let new_usage = base_entry_memory_usage(relative_path).saturating_add(as_u64(content_len));
        let projected = self.memory_usage.saturating_sub(old_usage).saturating_add(new_usage);
        if projected > limit {
            Err(MountError::MemoryUsageLimitExceeded(limit))
        } else {
            Ok(())
        }
    }

    /// Checks a sequence of replacements as one atomic overlay update.
    pub(super) fn check_replacements<'a>(
        &self,
        replacements: impl IntoIterator<Item = (&'a str, &'a OverlayEntry)>,
        limit: u64,
    ) -> Result<(), MountError> {
        let mut projected = self.memory_usage;
        let mut replaced = BTreeMap::new();
        for (path, entry) in replacements {
            let old_usage = replaced
                .get(path)
                .copied()
                .unwrap_or_else(|| self.entries.get(path).map_or(0, |old| entry_memory_usage(path, old)));
            let new_usage = entry_memory_usage(path, entry);
            projected = projected.saturating_sub(old_usage).saturating_add(new_usage);
            replaced.insert(path, new_usage);
        }
        if projected > limit {
            Err(MountError::MemoryUsageLimitExceeded(limit))
        } else {
            Ok(())
        }
    }

    /// Iterates over overlay entries whose keys start with `prefix`.
    ///
    /// `prefix` must be either `""` or end with `'/'`. The upper bound uses a
    /// lexical successor so the range query stays tight without scanning the
    /// whole map.
    pub(super) fn prefix_iter(&self, prefix: &str) -> impl Iterator<Item = (&str, &OverlayEntry)> {
        debug_assert!(prefix.is_empty() || prefix.ends_with('/'));

        let upper_storage;
        let bounds: (Bound<&str>, Bound<&str>) = if prefix.is_empty() {
            (Bound::Unbounded, Bound::Unbounded)
        } else {
            upper_storage = {
                let mut upper = prefix.to_owned();
                upper.pop();
                upper.push('0');
                upper
            };
            (Bound::Included(prefix), Bound::Excluded(upper_storage.as_str()))
        };

        self.entries
            .range::<str, _>(bounds)
            .map(|(key, value)| (key.as_str(), value))
    }
}

/// Estimates retained heap bytes for one overlay entry.
fn entry_memory_usage(relative_path: &str, entry: &OverlayEntry) -> u64 {
    let variable = match entry {
        OverlayEntry::File(file) => file.content.len(),
        OverlayEntry::RealFileRef(file_ref) => file_ref.host_path.as_os_str().len(),
        OverlayEntry::Directory { .. } | OverlayEntry::Deleted => 0,
    };
    base_entry_memory_usage(relative_path).saturating_add(as_u64(variable))
}

/// Returns the fixed and key-dependent charge for an overlay entry.
fn base_entry_memory_usage(relative_path: &str) -> u64 {
    ENTRY_MEMORY_USAGE
        .saturating_add(as_u64(relative_path.len()))
        .saturating_add(as_u64(mem::size_of::<OverlayEntry>()))
}

/// An entry stored in an overlay mount.
#[derive(Debug)]
pub(super) enum OverlayEntry {
    /// A file written by sandbox code and stored directly in memory.
    File(OverlayFile),

    /// A lazily-read reference to a real host file that has been renamed into
    /// the overlay without eagerly loading its contents.
    RealFileRef(OverlayFileRef),

    /// A directory that exists only in the overlay.
    Directory {
        /// Modification time recorded for synthetic stat results.
        mtime: f64,
    },

    /// A tombstone hiding a real or previously-overlay entry.
    Deleted,
}

/// In-memory contents of a file owned by the overlay.
#[derive(Debug)]
pub(super) struct OverlayFile {
    /// Raw file contents.
    pub content: Vec<u8>,
    /// Modification time recorded for synthetic stat results.
    pub mtime: f64,
}

/// A lazy reference to a real host file preserved during overlay rename.
#[derive(Debug)]
pub(super) struct OverlayFileRef {
    /// Canonical host path for the original file contents.
    pub host_path: PathBuf,
    /// Modification time copied from the original file.
    pub mtime: f64,
    /// File size in bytes.
    pub size: i64,
}

impl OverlayFileRef {
    /// Builds a lazy file reference from a host path if metadata can be read.
    ///
    /// Uses `fs::metadata` which follows symlinks, so the size and mtime
    /// reflect the target file. Use [`from_lstat`](Self::from_lstat) when
    /// the path itself is a symlink that should be preserved as-is.
    #[must_use]
    pub fn from_host_path(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        let mtime = metadata
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0.0, |duration| duration.as_secs_f64());
        let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        Some(Self {
            host_path: path.to_path_buf(),
            mtime,
            size,
        })
    }

    /// Builds a lazy file reference using `symlink_metadata` (lstat).
    ///
    /// Unlike [`from_host_path`](Self::from_host_path), this does not follow
    /// symlinks. The stored `host_path` is the symlink itself, preserving
    /// symlink identity across overlay renames.
    #[must_use]
    pub fn from_lstat(path: &Path) -> Option<Self> {
        let metadata = fs::symlink_metadata(path).ok()?;
        let mtime = metadata
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0.0, |duration| duration.as_secs_f64());
        let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        Some(Self {
            host_path: path.to_path_buf(),
            mtime,
            size,
        })
    }
}
