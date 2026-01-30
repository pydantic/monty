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
    Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::Display, serde::Serialize, serde::Deserialize,
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
}

impl TryFrom<StaticStrings> for OsFunction {
    type Error = ();

    /// Attempts to convert a method name (as a `StaticStrings` variant) to an `OsFunction`.
    ///
    /// Returns `Err(())` if the method name doesn't correspond to an OS operation.
    fn try_from(method: StaticStrings) -> Result<Self, Self::Error> {
        match method {
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

const STAT_RESULT_TYPE_NAME: &str = "os.stat_result";
const STAT_RESULT_FIELDS: &[&str] = &[
    "st_mode", "st_ino", "st_dev", "st_nlink", "st_uid", "st_gid", "st_size", "st_atime", "st_mtime", "st_ctime",
];

/// Unix file permission bits for a single class (owner, group, or others).
///
/// Each permission class has read (r), write (w), and execute (x) bits.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatPerms {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl StatPerms {
    /// No permissions (---).
    pub const NONE: Self = Self {
        read: false,
        write: false,
        execute: false,
    };
    /// Read only (r--).
    pub const R: Self = Self {
        read: true,
        write: false,
        execute: false,
    };
    /// Read and write (rw-).
    pub const RW: Self = Self {
        read: true,
        write: true,
        execute: false,
    };
    /// Read and execute (r-x).
    pub const RX: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
    /// Read, write, and execute (rwx).
    pub const RWX: Self = Self {
        read: true,
        write: true,
        execute: true,
    };

    /// Converts to the 3-bit octal representation (0-7).
    #[must_use]
    pub const fn as_bits(self) -> i64 {
        let r = if self.read { 4 } else { 0 };
        let w = if self.write { 2 } else { 0 };
        let x = if self.execute { 1 } else { 0 };
        r | w | x
    }
}

/// Unix file mode combining file type and permissions for owner, group, and others.
///
/// Use the `file()`, `dir()`, or `symlink()` constructors to create modes for
/// specific file types.
#[derive(Debug, Clone, Copy)]
pub struct StatMode {
    /// File type bits (regular file, directory, symlink, etc.)
    file_type: i64,
    /// Owner permissions (user).
    pub owner: StatPerms,
    /// Group permissions.
    pub group: StatPerms,
    /// Others permissions (world).
    pub others: StatPerms,
}

impl StatMode {
    const FILE_TYPE_REGULAR: i64 = 0o100_000;
    const FILE_TYPE_DIRECTORY: i64 = 0o040_000;
    const FILE_TYPE_SYMLINK: i64 = 0o120_000;

    /// Creates a mode for a regular file with the given permissions.
    #[must_use]
    pub const fn file(owner: StatPerms, group: StatPerms, others: StatPerms) -> Self {
        Self {
            file_type: Self::FILE_TYPE_REGULAR,
            owner,
            group,
            others,
        }
    }

    /// Creates a mode for a directory with the given permissions.
    #[must_use]
    pub const fn dir(owner: StatPerms, group: StatPerms, others: StatPerms) -> Self {
        Self {
            file_type: Self::FILE_TYPE_DIRECTORY,
            owner,
            group,
            others,
        }
    }

    /// Creates a mode for a symbolic link with the given permissions.
    #[must_use]
    pub const fn symlink(owner: StatPerms, group: StatPerms, others: StatPerms) -> Self {
        Self {
            file_type: Self::FILE_TYPE_SYMLINK,
            owner,
            group,
            others,
        }
    }
}

impl From<StatMode> for i64 {
    fn from(mode: StatMode) -> Self {
        mode.file_type | (mode.owner.as_bits() << 6) | (mode.group.as_bits() << 3) | mode.others.as_bits()
    }
}

/// Creates a stat_result for a regular file.
///
/// # Arguments
/// * `mode` - File permissions (use `StatMode::file()` or a raw i64 like `0o644`)
/// * `size` - File size in bytes
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn file_stat(mode: impl Into<i64>, size: i64, mtime: f64) -> MontyObject {
    let mode_bits: i64 = mode.into();
    // If only permission bits provided (no file type), add regular file type
    let mode_bits = if mode_bits < 0o1000 {
        mode_bits | 0o100_000
    } else {
        mode_bits
    };
    stat_result(mode_bits, 0, 0, 1, 0, 0, size, mtime, mtime, mtime)
}

/// Creates a stat_result for a directory.
///
/// # Arguments
/// * `mode` - Directory permissions (use `StatMode::dir()` or a raw i64 like `0o755`)
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn dir_stat(mode: impl Into<i64>, mtime: f64) -> MontyObject {
    let mode_bits: i64 = mode.into();
    // If only permission bits provided (no file type), add directory type
    let mode_bits = if mode_bits < 0o1000 {
        mode_bits | 0o040_000
    } else {
        mode_bits
    };
    stat_result(mode_bits, 0, 0, 2, 0, 0, 4096, mtime, mtime, mtime)
}

/// Creates a stat_result for a symbolic link.
///
/// # Arguments
/// * `mode` - Symlink permissions (use `StatMode::symlink()` or a raw i64 like `0o777`)
/// * `mtime` - Modification time as Unix timestamp
#[must_use]
pub fn symlink_stat(mode: impl Into<i64>, mtime: f64) -> MontyObject {
    let mode_bits: i64 = mode.into();
    // If only permission bits provided (no file type), add symlink type
    let mode_bits = if mode_bits < 0o1000 {
        mode_bits | 0o120_000
    } else {
        mode_bits
    };
    stat_result(mode_bits, 0, 0, 1, 0, 0, 0, mtime, mtime, mtime)
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
