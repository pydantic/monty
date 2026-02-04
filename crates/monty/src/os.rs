//! OS-level operations that require host system access.
//!
//! This module defines the `OsFunction` enum, which represents operations that
//! cannot be performed in a sandboxed environment. When a type method needs to
//! perform one of these operations, it returns an `AttrCallResult::OsCall` variant
//! with the function and arguments. The VM then yields control to the host via
//! `FrameExit::OsCall`, allowing the host to execute the operation and resume.
//!
//! This design enables sandboxed execution: the interpreter never directly performs
//! I/O, filesystem, or network operations. Instead, the host decides whether to
//! permit and execute such operations.

use crate::{MontyObject, intern::StaticStrings};

/// OS operations that require host system access.
///
/// These represent operations that Monty cannot perform in isolation because
/// they require interacting with the operating system (filesystem, network, etc.).
/// The host application decides whether to permit and execute these operations.
///
/// # Extension
///
/// When adding new operations, add both the variant here and update the
/// `TryFrom<StaticStrings>` implementation to map method names to operations.
// #[repr(u8)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumString, strum::Display, serde::Serialize, serde::Deserialize,
)]
pub enum OsFunction {
    /// Check if a path exists
    #[strum(serialize = "Path.exists")]
    Exists,
    /// Check if path is a file
    #[strum(serialize = "Path.is_file")]
    IsFile,
    /// Check if path is a directory
    #[strum(serialize = "Path.is_dir")]
    IsDir,
    /// Check if path is a symbolic link
    #[strum(serialize = "Path.is_symlink")]
    IsSymlink,
    /// Read file contents as text
    #[strum(serialize = "Path.read_text")]
    ReadText,
    /// Read file contents as bytes
    #[strum(serialize = "Path.read_bytes")]
    ReadBytes,
    /// Write text to file
    #[strum(serialize = "Path.write_text")]
    WriteText,
    /// Write bytes to file
    #[strum(serialize = "Path.write_bytes")]
    WriteBytes,
    /// Create directory
    #[strum(serialize = "Path.mkdir")]
    Mkdir,
    /// Remove file
    #[strum(serialize = "Path.unlink")]
    Unlink,
    /// Remove directory
    #[strum(serialize = "Path.rmdir")]
    Rmdir,
    /// List directory contents
    #[strum(serialize = "Path.iterdir")]
    Iterdir,
    /// Get file stats
    #[strum(serialize = "Path.stat")]
    Stat,
    /// Rename/move file
    #[strum(serialize = "Path.rename")]
    Rename,
    /// Get resolved absolute path
    #[strum(serialize = "Path.resolve")]
    Resolve,
    /// Get absolute path (without resolving symlinks)
    #[strum(serialize = "Path.absolute")]
    Absolute,
    /// Get an environment variable value
    #[strum(serialize = "os.getenv")]
    Getenv,
    /// Get the entire environment as a dictionary
    #[strum(serialize = "os.environ")]
    GetEnviron,
}

impl TryFrom<StaticStrings> for OsFunction {
    type Error = ();

    /// Attempts to convert a method name (as a `StaticStrings` variant) to an `OsFunction`.
    ///
    /// Returns `Err(())` if the method name doesn't correspond to an OS operation.
    fn try_from(method: StaticStrings) -> Result<Self, Self::Error> {
        match method {
            // Read operations
            StaticStrings::Exists => Ok(Self::Exists),
            StaticStrings::IsFile => Ok(Self::IsFile),
            StaticStrings::IsDir => Ok(Self::IsDir),
            StaticStrings::IsSymlink => Ok(Self::IsSymlink),
            StaticStrings::ReadText => Ok(Self::ReadText),
            StaticStrings::ReadBytes => Ok(Self::ReadBytes),
            StaticStrings::StatMethod => Ok(Self::Stat),
            StaticStrings::Iterdir => Ok(Self::Iterdir),
            StaticStrings::Resolve => Ok(Self::Resolve),
            StaticStrings::Absolute => Ok(Self::Absolute),
            // Write operations
            StaticStrings::WriteText => Ok(Self::WriteText),
            StaticStrings::WriteBytes => Ok(Self::WriteBytes),
            StaticStrings::Mkdir => Ok(Self::Mkdir),
            StaticStrings::Unlink => Ok(Self::Unlink),
            StaticStrings::Rmdir => Ok(Self::Rmdir),
            StaticStrings::Rename => Ok(Self::Rename),
            _ => Err(()),
        }
    }
}

// =============================================================================
// stat_result builders
// =============================================================================
// These functions create MontyObject::NamedTuple values that match Python's
// os.stat_result structure. The stat_result has 10 fields:
// st_mode, st_ino, st_dev, st_nlink, st_uid, st_gid, st_size, st_atime, st_mtime, st_ctime

const STAT_RESULT_TYPE_NAME: &str = "StatResult";
const STAT_RESULT_FIELDS: &[&str] = &[
    "st_mode", "st_ino", "st_dev", "st_nlink", "st_uid", "st_gid", "st_size", "st_atime", "st_mtime", "st_ctime",
];

/// Creates a stat_result for a regular file.
///
/// The file type bits (`0o100_000`) are automatically added if not present.
///
/// # Arguments
/// * `mode` - File permissions as octal. Common values:
///   - `0o644` - rw-r--r-- (owner read/write, others read)
///   - `0o600` - rw------- (owner read/write only)
///   - `0o755` - rwxr-xr-x (executable, owner full, others read/execute)
///   - `0o100644` - same as 0o644 with explicit file type bits
/// * `size` - File size in bytes
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn file_stat(mode: i64, size: i64, mtime: f64) -> MontyObject {
    // If only permission bits provided (no file type), add regular file type
    let mode = if mode < 0o1000 { mode | 0o100_000 } else { mode };
    stat_result(mode, 0, 0, 1, 0, 0, size, mtime, mtime, mtime)
}

/// Creates a stat_result for a directory.
///
/// The directory type bits (`0o040_000`) are automatically added if not present.
///
/// # Arguments
/// * `mode` - Directory permissions as octal. Common values:
///   - `0o755` - rwxr-xr-x (owner full, others read/execute)
///   - `0o700` - rwx------ (owner only)
///   - `0o040755` - same as 0o755 with explicit directory type bits
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn dir_stat(mode: i64, mtime: f64) -> MontyObject {
    // If only permission bits provided (no file type), add directory type
    let mode = if mode < 0o1000 { mode | 0o040_000 } else { mode };
    stat_result(mode, 0, 0, 2, 0, 0, 4096, mtime, mtime, mtime)
}

/// Creates a stat_result for a symbolic link.
///
/// The symlink type bits (`0o120_000`) are automatically added if not present.
///
/// # Arguments
/// * `mode` - Symlink permissions as octal. Common values:
///   - `0o777` - rwxrwxrwx (symlinks typically have full permissions)
///   - `0o120777` - same as 0o777 with explicit symlink type bits
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn symlink_stat(mode: i64, mtime: f64) -> MontyObject {
    // If only permission bits provided (no file type), add symlink type
    let mode = if mode < 0o1000 { mode | 0o120_000 } else { mode };
    stat_result(mode, 0, 0, 1, 0, 0, 0, mtime, mtime, mtime)
}

/// Creates a full stat_result with all 10 fields specified.
///
/// This is the low-level builder; prefer `file_stat()`, `dir_stat()`, or `symlink_stat()`
/// for common cases.
#[must_use]
#[expect(clippy::too_many_arguments)]
pub fn stat_result(
    st_mode: i64,
    st_ino: i64,
    st_dev: i64,
    st_nlink: i64,
    st_uid: i64,
    st_gid: i64,
    st_size: i64,
    st_atime: f64,
    st_mtime: f64,
    st_ctime: f64,
) -> MontyObject {
    MontyObject::NamedTuple {
        type_name: STAT_RESULT_TYPE_NAME.to_owned(),
        field_names: STAT_RESULT_FIELDS.iter().map(|s| (*s).to_owned()).collect(),
        values: vec![
            MontyObject::Int(st_mode),
            MontyObject::Int(st_ino),
            MontyObject::Int(st_dev),
            MontyObject::Int(st_nlink),
            MontyObject::Int(st_uid),
            MontyObject::Int(st_gid),
            MontyObject::Int(st_size),
            MontyObject::Float(st_atime),
            MontyObject::Float(st_mtime),
            MontyObject::Float(st_ctime),
        ],
    }
}
