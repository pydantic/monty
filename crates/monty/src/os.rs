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

use strum::{EnumString, FromRepr, IntoStaticStr};

use crate::intern::StaticStrings;

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
#[repr(u8)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, FromRepr, EnumString, IntoStaticStr, serde::Serialize, serde::Deserialize,
)]
#[strum(serialize_all = "snake_case")]
pub enum OsFunction {
    /// Check if a path exists: `Path.exists()`
    Exists,
    /// Check if path is a file: `Path.is_file()`
    IsFile,
    /// Check if path is a directory: `Path.is_dir()`
    IsDir,
    /// Check if path is a symbolic link: `Path.is_symlink()`
    IsSymlink,
    /// Read file contents as text: `Path.read_text()`
    ReadText,
    /// Read file contents as bytes: `Path.read_bytes()`
    ReadBytes,
    /// Write text to file: `Path.write_text(content)`
    WriteText,
    /// Write bytes to file: `Path.write_bytes(content)`
    WriteBytes,
    /// Create directory: `Path.mkdir()`
    Mkdir,
    /// Remove file: `Path.unlink()`
    Unlink,
    /// Remove directory: `Path.rmdir()`
    Rmdir,
    /// List directory contents: `Path.iterdir()`
    Iterdir,
    /// Get file stats: `Path.stat()`
    Stat,
    /// Rename/move file: `Path.rename(target)`
    Rename,
    /// Get resolved absolute path: `Path.resolve()`
    Resolve,
    /// Get absolute path (without resolving symlinks): `Path.absolute()`
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
