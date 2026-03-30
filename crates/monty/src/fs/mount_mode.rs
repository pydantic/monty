//! Mount mode definitions and overlay state management.
//!
//! Defines the access policies for mounted directories and the in-memory
//! overlay state used by [`MountMode::OverlayMemory`].

use std::path::PathBuf;

use ahash::AHashMap;

/// Defines the access policy for a mount point.
///
/// Controls what operations sandbox code can perform on files within
/// the mounted directory. The overlay modes provide copy-on-write
/// semantics where reads fall through to the real directory but
/// writes are captured separately.
///
/// # Security
///
/// Regardless of mode, path traversal and symlink escape protection is always
/// enforced. The monty runtime MUST NEVER read, write, or obtain any information
/// about any file or directory outside the specific directory that is mounted.
#[derive(Debug)]
pub enum MountMode {
    /// Full read and write access to the host directory.
    ///
    /// Files are read from and written directly to the real host path.
    /// Use with caution — sandbox code can modify real files.
    ReadWrite,

    /// Read-only access to the host directory.
    ///
    /// Read operations (exists, `is_file`, `read_text`, stat, iterdir, etc.) work
    /// normally against the real filesystem. Any write operation (`write_text`,
    /// `write_bytes`, mkdir, unlink, rmdir, rename) raises `PermissionError`.
    ReadOnly,

    /// Copy-on-write overlay backed by in-memory storage.
    ///
    /// Read operations fall through to the real host directory. Write operations
    /// are captured in the contained [`OverlayState`] and never touch the real
    /// filesystem. Deletions insert a tombstone ([`OverlayEntry::Deleted`]) that
    /// hides the real file from subsequent reads.
    ///
    /// Directory listings merge real entries with overlay entries, with overlay
    /// entries taking precedence on name conflicts, and tombstoned entries excluded.
    OverlayMemory(OverlayState),

    /// Copy-on-write overlay backed by a separate host directory.
    ///
    /// Reads fall through to the mounted host directory. Writes go to the
    /// `upper_dir` directory on the host filesystem, mirroring the path
    /// structure. Deletions are tracked via whiteout files (`.wh.<name>`)
    /// in the upper directory, following Linux overlayfs conventions.
    ///
    /// The `upper_dir` must exist and be writable at mount time.
    OverlayDirectory {
        /// The host directory where writes are stored. Must exist and be writable.
        upper_dir: PathBuf,
    },
}

/// In-memory overlay state for [`MountMode::OverlayMemory`].
///
/// Tracks all filesystem mutations made by sandbox code. A single [`AHashMap`]
/// maps relative paths (within the mount) to [`OverlayEntry`] variants that
/// describe the current state of each path in the overlay layer.
///
/// # Lookup order
///
/// For any path, the overlay is checked first:
/// - [`OverlayEntry::File`] → return the overlay content
/// - [`OverlayEntry::Directory`] → treat as an existing directory
/// - [`OverlayEntry::Deleted`] → treat as non-existent (hide real FS entry)
/// - No entry → fall through to the real filesystem
#[derive(Debug, Default)]
pub struct OverlayState {
    /// All overlay entries keyed by relative path within the mount.
    ///
    /// Keys are forward-slash-separated relative paths like `"subdir/file.txt"`.
    /// The root of the mount is represented by an empty string `""`.
    entries: AHashMap<String, OverlayEntry>,
}

impl OverlayState {
    /// Creates a new empty overlay state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up the overlay entry for a relative path.
    ///
    /// Returns `None` if no overlay entry exists for this path, meaning the
    /// real filesystem should be consulted.
    #[must_use]
    pub fn get(&self, relative_path: &str) -> Option<&OverlayEntry> {
        self.entries.get(relative_path)
    }

    /// Inserts or replaces an overlay entry for a relative path.
    ///
    /// This is the primary mutation method — all writes, directory creations,
    /// and deletions go through here.
    pub fn insert(&mut self, relative_path: String, entry: OverlayEntry) {
        self.entries.insert(relative_path, entry);
    }

    /// Returns an iterator over all overlay entries.
    ///
    /// Used by `iterdir` to merge overlay entries with real filesystem entries.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &OverlayEntry)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// An entry in the in-memory overlay layer.
///
/// Each variant represents a different kind of mutation that sandbox code
/// has performed on a path within the mount.
#[derive(Debug)]
pub enum OverlayEntry {
    /// A file written by sandbox code. The content is stored in memory.
    File(OverlayFile),

    /// A directory created by sandbox code that may not exist on the real filesystem.
    Directory {
        /// Modification time as Unix timestamp (seconds since epoch).
        mtime: f64,
    },

    /// A tombstone indicating that this path has been deleted by sandbox code.
    ///
    /// The real filesystem may still have a file or directory at this path, but
    /// it is hidden from all sandbox reads. This prevents real entries from
    /// "showing through" after deletion.
    Deleted,
}

/// A file stored in the in-memory overlay layer.
///
/// Represents a file that was written by sandbox code. The content is stored
/// as raw bytes — text files are UTF-8 encoded before storage.
#[derive(Debug)]
pub struct OverlayFile {
    /// The file content as raw bytes.
    pub content: Vec<u8>,
    /// Modification time as Unix timestamp (seconds since epoch).
    pub mtime: f64,
}
