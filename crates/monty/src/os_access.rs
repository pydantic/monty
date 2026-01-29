//! Sandboxed filesystem access via the `OsAccess` trait.
//!
//! This module provides the types needed for sandboxed filesystem operations.
//! When Python code uses `pathlib.Path` methods that require I/O (like `exists()`,
//! `read_text()`), the VM yields control via external function calls. The host
//! application then routes these calls through its `OsAccess` implementation.

use std::fmt;

/// Result of a `stat()` operation - matches Python's `os.stat_result`.
///
/// Contains file metadata including modification time, size, and mode.
/// The `st_mode` field uses Unix-style mode bits (e.g., `0o100644` for regular file).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Stat {
    /// Modification time as Unix timestamp (seconds since epoch).
    pub st_mtime: f64,
    /// File size in bytes.
    pub st_size: u64,
    /// File mode (type and permissions).
    /// - `0o040000` (S_IFDIR): directory
    /// - `0o100000` (S_IFREG): regular file
    /// - `0o120000` (S_IFLNK): symbolic link
    pub st_mode: u32,
}

/// File type bits from `st_mode`.
const S_IFMT: u32 = 0o170_000;
/// Directory type bit.
const S_IFDIR: u32 = 0o040_000;
/// Regular file type bit.
const S_IFREG: u32 = 0o100_000;
/// Symbolic link type bit.
const S_IFLNK: u32 = 0o120_000;

impl Stat {
    /// Returns `true` if this stat result represents a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        (self.st_mode & S_IFMT) == S_IFDIR
    }

    /// Returns `true` if this stat result represents a regular file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        (self.st_mode & S_IFMT) == S_IFREG
    }

    /// Returns `true` if this stat result represents a symbolic link.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        (self.st_mode & S_IFMT) == S_IFLNK
    }
}

/// Object-safe trait for sandboxed filesystem access.
///
/// Implementations provide the actual filesystem operations. The runtime calls
/// these methods when Python code uses `pathlib.Path` methods that require I/O.
///
/// All methods take a path string and return either the result or an error message.
/// Error messages should be suitable for display in Python exceptions (e.g., `OSError`).
///
/// # Example Implementation
///
/// ```ignore
/// struct RealFilesystem;
///
/// impl OsAccess for RealFilesystem {
///     fn stat(&self, path: &str) -> Result<Stat, String> {
///         let meta = std::fs::metadata(path)
///             .map_err(|e| e.to_string())?;
///         Ok(Stat {
///             st_mtime: meta.modified()?.duration_since(UNIX_EPOCH)?.as_secs_f64(),
///             st_size: meta.len(),
///             st_mode: if meta.is_dir() { 0o040755 } else { 0o100644 },
///         })
///     }
///     // ... other methods
/// }
/// ```
pub trait OsAccess: fmt::Debug + Send + Sync {
    /// Returns file metadata for the given path.
    ///
    /// # Errors
    /// Returns an error message if the file doesn't exist or cannot be accessed.
    fn stat(&self, path: &str) -> Result<Stat, String>;

    /// Returns `true` if the path exists (as file, directory, or symlink).
    ///
    /// # Errors
    /// Returns an error only for access errors, not for non-existent paths.
    fn exists(&self, path: &str) -> Result<bool, String>;

    /// Returns `true` if the path exists and is a regular file.
    ///
    /// # Errors
    /// Returns an error only for access errors, not for non-existent paths.
    fn is_file(&self, path: &str) -> Result<bool, String>;

    /// Returns `true` if the path exists and is a directory.
    ///
    /// # Errors
    /// Returns an error only for access errors, not for non-existent paths.
    fn is_dir(&self, path: &str) -> Result<bool, String>;

    /// Returns `true` if the path exists and is a symbolic link.
    ///
    /// # Errors
    /// Returns an error only for access errors, not for non-existent paths.
    fn is_symlink(&self, path: &str) -> Result<bool, String>;

    /// Reads the entire file as bytes.
    ///
    /// # Errors
    /// Returns an error if the file doesn't exist, cannot be read, or is too large.
    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, String>;

    /// Reads the entire file as text with the given encoding.
    ///
    /// # Arguments
    /// * `path` - The file path to read.
    /// * `encoding` - The text encoding (e.g., `"utf-8"`). Implementations may
    ///   only support UTF-8.
    ///
    /// # Errors
    /// Returns an error if the file doesn't exist, cannot be read, or contains
    /// invalid text for the given encoding.
    fn read_text(&self, path: &str, encoding: &str) -> Result<String, String>;

    /// Lists directory contents.
    ///
    /// Returns a list of entry names (not full paths) in the directory.
    ///
    /// # Errors
    /// Returns an error if the path doesn't exist or is not a directory.
    fn iterdir(&self, path: &str) -> Result<Vec<String>, String>;

    /// Resolves the path to an absolute path, following symlinks.
    ///
    /// # Errors
    /// Returns an error if the path cannot be resolved (e.g., symlink loop).
    fn resolve(&self, path: &str) -> Result<String, String>;

    /// Returns the absolute form of the path without resolving symlinks.
    ///
    /// # Errors
    /// Returns an error if the current directory cannot be determined.
    fn absolute(&self, path: &str) -> Result<String, String>;
}
