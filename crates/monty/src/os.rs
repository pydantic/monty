//! OS-level operations that require host system access.
//!
//! This module defines the `OsFunction` enum, which represents operations that
//! cannot be performed in a sandboxed environment. When a type method needs to
//! perform one of these operations, it returns an `CallResult::OsCall` variant
//! with the function and arguments. The VM then yields control to the host via
//! `FrameExit::OsCall`, allowing the host to execute the operation and resume.
//!
//! This design enables sandboxed execution: the interpreter never directly performs
//! I/O, filesystem, or network operations. Instead, the host decides whether to
//! permit and execute such operations.

use chrono::{Datelike, Local, Timelike, Utc};

use crate::{
    ExcType, MontyDate, MontyDateTime, MontyException, MontyObject, intern::StaticStrings, types::str::StringRepr,
};

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
    /// Get today's date from the host system (for `date.today()`).
    ///
    /// Takes no arguments. The host should return `MontyObject::Date`.
    #[strum(serialize = "date.today")]
    DateToday,
    /// Get the current date/time from the host system (for `datetime.now(tz=...)`).
    ///
    /// Takes one argument: the timezone (`MontyObject::TimeZone` or `MontyObject::None`).
    /// The host should return `MontyObject::DateTime`.
    #[strum(serialize = "datetime.now")]
    DateTimeNow,
}

impl OsFunction {
    /// Returns `true` if this is a filesystem operation that can be handled by a
    /// [`MountTable`](crate::fs::MountTable).
    ///
    /// Non-filesystem operations (`Getenv`, `GetEnviron`, `DateToday`, `DateTimeNow`)
    /// return `false` and should be passed through to the host callback.
    #[must_use]
    pub fn is_filesystem(&self) -> bool {
        !matches!(
            self,
            Self::Getenv | Self::GetEnviron | Self::DateToday | Self::DateTimeNow
        )
    }

    /// Returns `true` if this is a write operation that modifies the filesystem.
    ///
    /// Write operations are blocked in read-only mounts and redirected in overlay mounts.
    #[must_use]
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            Self::WriteText | Self::WriteBytes | Self::Mkdir | Self::Unlink | Self::Rmdir | Self::Rename
        )
    }

    /// Returns `true` for operations that check path existence without reading content.
    ///
    /// These operations return `false` for nonexistent paths instead of raising
    /// `FileNotFoundError`, matching CPython's `pathlib.Path` behavior.
    #[must_use]
    pub fn is_existence_check(&self) -> bool {
        matches!(self, Self::Exists | Self::IsFile | Self::IsDir | Self::IsSymlink)
    }

    /// Returns an appropriate exception for when no handler is available for this operation.
    ///
    /// Filesystem operations return `PermissionError` with the path from `args`.
    /// Non-filesystem operations return `RuntimeError` indicating the function
    /// isn't supported.
    #[must_use]
    pub fn on_no_handler(&self, args: &[MontyObject]) -> MontyException {
        if self.is_filesystem() {
            let path = args.first().map_or("<unknown>", |a| match a {
                MontyObject::Path(p) => p.as_str(),
                MontyObject::String(s) => s.as_str(),
                _ => "<unknown>",
            });
            MontyException::new(
                ExcType::PermissionError,
                Some(format!("Permission denied: {}", StringRepr(path))),
            )
        } else {
            MontyException::new(
                ExcType::RuntimeError,
                Some(format!("'{self}' is not supported in this environment")),
            )
        }
    }
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

/// Creates a `stat_result` for a regular file.
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

/// Creates a `stat_result` for a directory.
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

/// Creates a `stat_result` for a symbolic link.
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

/// Creates a full `stat_result` with all 10 fields specified.
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

// =============================================================================
// Host clock helpers
// =============================================================================
// These helpers are used by standard (non-iterative) execution to satisfy the
// `DateToday` and `DateTimeNow` OS calls directly from the host clock, without
// going through the suspend/resume plumbing. The iterative (`start`/`resume`)
// path still surfaces these as `RunProgress::OsCall`, so hosts can keep
// overriding them with deterministic clocks when needed.
//
// Reading the host wall-clock is deliberately allowed in the sandbox: it only
// exposes time-of-day, which untrusted code can already estimate via loop
// timing. It does not grant access to the filesystem, environment, or any
// external resources.

/// Returns the host system's local date as a `MontyObject::Date`.
///
/// Used by `date.today()` in standard execution. Mirrors the CPython semantics
/// of returning the local civil date (not UTC).
#[must_use]
pub(crate) fn host_date_today() -> MontyObject {
    let local = Local::now().naive_local();
    MontyObject::Date(MontyDate {
        year: local.year(),
        month: u8::try_from(local.month()).expect("month is always 1..=12"),
        day: u8::try_from(local.day()).expect("day is always 1..=31"),
    })
}

/// Returns the host system's current date/time as a `MontyObject::DateTime`.
///
/// Used by `datetime.now(tz=...)` in standard execution. The `tz` argument
/// determines the returned value:
/// - `MontyObject::None`: naive datetime using local wall-clock time.
/// - `MontyObject::TimeZone`: aware datetime, with the host's UTC instant
///   adjusted by the fixed offset and the original tz metadata retained so the
///   constructed datetime keeps `tzinfo == tz`.
///
/// Any other `tz` variant is treated like `None`; callers in the VM have
/// already validated the argument, so this is a defensive fallback.
#[must_use]
pub(crate) fn host_datetime_now(tz: &MontyObject) -> MontyObject {
    // For aware tz, compute local civil components by shifting the real UTC
    // instant by the fixed offset, and preserve the caller's offset/name so
    // `datetime.tzinfo` stays equal to the supplied tz. For naive/None, fall
    // back to the host's local wall-clock time.
    let (local, offset_seconds, timezone_name) = if let MontyObject::TimeZone(tz) = tz {
        let offset_delta =
            chrono::TimeDelta::try_seconds(i64::from(tz.offset_seconds)).expect("timezone offset validated");
        let local = (Utc::now() + offset_delta).naive_utc();
        (local, Some(tz.offset_seconds), tz.name.clone())
    } else {
        (Local::now().naive_local(), None, None)
    };

    MontyObject::DateTime(MontyDateTime {
        year: local.year(),
        month: u8::try_from(local.month()).expect("month is always 1..=12"),
        day: u8::try_from(local.day()).expect("day is always 1..=31"),
        hour: u8::try_from(local.hour()).expect("hour is always 0..=23"),
        minute: u8::try_from(local.minute()).expect("minute is always 0..=59"),
        second: u8::try_from(local.second()).expect("second is always 0..=59"),
        microsecond: local.and_utc().timestamp_subsec_micros(),
        offset_seconds,
        timezone_name,
    })
}
