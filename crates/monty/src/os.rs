//! Access to pseudo OS environment, including:
//! - Sandboxed filesystem access
//! - (TODO) Access to environment variables
//! - (TODO) Access to python version and platform information

use crate::intern::StaticStrings;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum OsFunction {
    // Path filesystem methods
    PathExists,
    PathIsFile,
    PathIsDir,
    PathIsSymlink,
    PathStatMethod,
    PathReadBytes,
    PathIterdir,
    PathResolve,
    PathAbsolute,
    // more methods to come
}

/// If the string matches a known OS function, return it.
impl TryFrom<StaticStrings> for OsFunction {
    type Error = ();
    fn try_from(s: StaticStrings) -> Result<Self, ()> {
        match s {
            StaticStrings::Exists => Ok(OsFunction::PathExists),
            StaticStrings::IsFile => Ok(OsFunction::PathIsFile),
            StaticStrings::IsDir => Ok(OsFunction::PathIsDir),
            StaticStrings::IsSymlink => Ok(OsFunction::PathIsSymlink),
            StaticStrings::StatMethod => Ok(OsFunction::PathStatMethod),
            StaticStrings::ReadBytes => Ok(OsFunction::PathReadBytes),
            StaticStrings::Iterdir => Ok(OsFunction::PathIterdir),
            StaticStrings::Resolve => Ok(OsFunction::PathResolve),
            StaticStrings::Absolute => Ok(OsFunction::PathAbsolute),
            _ => Err(()),
        }
    }
}
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
