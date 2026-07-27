//! [`FileMode`] — the parsed, validated form of a Python `open()` mode
//! string, carried by [`MontyFileHandle`](crate::object::MontyFileHandle).

use std::{borrow::Cow, str::FromStr};
/// A parsed Python `open()` mode.
///
/// This single enum captures everything that matters about how a file was
/// opened: the access pattern (`r`/`w`/`a` and the `+` update flag) and
/// whether the file is binary. The variant name encodes the access pattern;
/// the `bool` payload is `true` for binary and `false` for text — i.e.
/// `Read(true)` is `'rb'` and `Read(false)` is `'r'`.
///
/// Construct one with the [`FromStr`] impl (`mode_str.parse::<FileMode>()`).
/// The original input string is
/// intentionally not preserved; [`FileMode::as_str`] rebuilds the canonical
/// CPython form (`'r'`, `'rb+'`, `'wb'`, …), matching how CPython itself
/// normalizes input like `'rt'` → `'r'` and `'r+b'` → `'rb+'`.
///
/// `+` update modes (`ReadUpdate`/`WriteUpdate`/`AppendUpdate`) are reserved
/// in the enum so the mode space is fully represented, but [`FromStr`]
/// currently rejects them — properly modelling them needs read-position
/// tracking that the file wrapper does not yet implement. Treat the `Update`
/// variants as unreachable at runtime; do not pattern-match against them as
/// if they were a valid result of parsing user input.
///
/// Carried publicly by [`MontyObject::FileHandle`](crate::object::MontyObject) so a host servicing file
/// operations can inspect the mode without re-parsing the raw string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FileMode {
    /// `r` / `rb`: read-only; the file must already exist.
    Read(bool),
    /// `r+` / `rb+`: read and write an existing file. Reserved; not yet
    /// produced by [`FromStr`].
    ReadUpdate(bool),
    /// `w` / `wb`: write-only; truncate the file (creating it if missing) on open.
    Write(bool),
    /// `w+` / `wb+`: read and write; truncate the file (creating it if missing).
    /// Reserved; not yet produced by [`FromStr`].
    WriteUpdate(bool),
    /// `a` / `ab`: write-only appending; create the file if missing, preserving content.
    Append(bool),
    /// `a+` / `ab+`: read and append; create the file if missing, preserving content.
    /// Reserved; not yet produced by [`FromStr`].
    AppendUpdate(bool),
}
impl FileMode {
    /// Returns the canonical Python `open()` mode string for this mode,
    /// matching what CPython exposes via `file.mode`.
    ///
    /// The result is always one of the 12 well-formed mode strings (`r`, `rb`,
    /// `r+`, `rb+`, `w`, `wb`, `w+`, `wb+`, `a`, `ab`, `a+`, `ab+`). This is
    /// the canonical form CPython itself normalizes user input into — e.g.
    /// `'rt'` → `'r'`, `'r+b'` → `'rb+'`, `'br'` → `'rb'`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read(false) => "r",
            Self::Read(true) => "rb",
            Self::ReadUpdate(false) => "r+",
            Self::ReadUpdate(true) => "rb+",
            Self::Write(false) => "w",
            Self::Write(true) => "wb",
            Self::WriteUpdate(false) => "w+",
            Self::WriteUpdate(true) => "wb+",
            Self::Append(false) => "a",
            Self::Append(true) => "ab",
            Self::AppendUpdate(false) => "a+",
            Self::AppendUpdate(true) => "ab+",
        }
    }

    /// Whether the file is binary (`'rb'`, `'wb'`, …) rather than text.
    #[must_use]
    pub fn is_binary(&self) -> bool {
        let (Self::Read(b)
        | Self::ReadUpdate(b)
        | Self::Write(b)
        | Self::WriteUpdate(b)
        | Self::Append(b)
        | Self::AppendUpdate(b)) = self;
        *b
    }

    /// Whether `read()` is allowed by this mode.
    #[must_use]
    pub fn readable(&self) -> bool {
        matches!(
            self,
            Self::Read(_) | Self::ReadUpdate(_) | Self::WriteUpdate(_) | Self::AppendUpdate(_)
        )
    }

    /// Whether `write()` is allowed by this mode.
    #[must_use]
    pub fn writable(&self) -> bool {
        matches!(
            self,
            Self::Write(_) | Self::WriteUpdate(_) | Self::Append(_) | Self::AppendUpdate(_) | Self::ReadUpdate(_)
        )
    }

    /// Whether writes should always append (`a`/`a+`).
    #[must_use]
    pub fn is_append(&self) -> bool {
        matches!(self, Self::Append(_) | Self::AppendUpdate(_))
    }

    /// Whether `open()` must truncate the file to empty immediately (`w`/`w+`).
    #[must_use]
    pub fn truncate(&self) -> bool {
        matches!(self, Self::Write(_) | Self::WriteUpdate(_))
    }

    /// Whether `open()` must create the file immediately if missing.
    ///
    /// True for the `w`/`w+` and `a`/`a+` families. For append modes this must
    /// not disturb existing content.
    #[must_use]
    pub fn create(&self) -> bool {
        matches!(
            self,
            Self::Write(_) | Self::WriteUpdate(_) | Self::Append(_) | Self::AppendUpdate(_)
        )
    }
    /// Returns the bare Python type name (`type(f).__name__`) for this mode.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            _ if !self.is_binary() => "TextIOWrapper",
            Self::ReadUpdate(_) | Self::WriteUpdate(_) | Self::AppendUpdate(_) => "BufferedRandom",
            Self::Read(_) => "BufferedReader",
            Self::Write(_) | Self::Append(_) => "BufferedWriter",
        }
    }

    /// Returns the fully-qualified `_io` wrapper type name a file opened with
    /// this mode presents as, matching CPython's `repr(f)` (e.g.
    /// `"_io.TextIOWrapper"`). The module-less form is [`type_name`](Self::type_name).
    #[must_use]
    pub fn file_type_name(&self) -> &'static str {
        match self {
            _ if !self.is_binary() => "_io.TextIOWrapper",
            Self::ReadUpdate(_) | Self::WriteUpdate(_) | Self::AppendUpdate(_) => "_io.BufferedRandom",
            Self::Read(_) => "_io.BufferedReader",
            Self::Write(_) | Self::Append(_) => "_io.BufferedWriter",
        }
    }
}
/// Parses a Python `open()` mode string into a [`FileMode`].
///
/// Monty supports the common read, write, append, and update combinations in
/// text or binary form. Exclusive creation (`x`) is rejected for now because
/// it needs a dedicated mount-table operation to be race-free.
///
/// The `Err` payload is a CPython-matched message — an unknown mode
/// character, duplicated `b`/`t`/`+`, conflicting binary+text flags, more
/// than one of the `r`/`w`/`a` actions, or none at all (`''`, `'b'`, `'t'`).
impl FromStr for FileMode {
    type Err = Cow<'static, str>;

    fn from_str(mode: &str) -> Result<Self, Self::Err> {
        let mut action = None;
        let mut binary = false;
        let mut text = false;

        for ch in mode.chars() {
            match ch {
                'r' | 'w' | 'a' => {
                    if action.replace(ch).is_some() {
                        // CPython's duplicate-action message differs from the missing-action
                        // one below (lowercase, no `... and at most one plus` suffix).
                        return Err("must have exactly one of create/read/write/append mode".into());
                    }
                }
                'x' => return Err("exclusive creation mode is not supported".into()),
                'b' => {
                    if binary {
                        return Err("invalid mode: binary mode specified twice".into());
                    }
                    binary = true;
                }
                't' => {
                    if text {
                        return Err("invalid mode: text mode specified twice".into());
                    }
                    text = true;
                }
                // `+` modes (`r+`, `w+`, `a+`, and their `b` variants) need
                // read-position tracking that Monty does not yet implement.
                // Reject them outright rather than silently truncating on the
                // first write (which would happen because the OS-level read
                // and write ops are full-file one-shots).
                '+' => return Err("update modes ('+') are not yet supported".into()),
                _ => return Err(format!("invalid mode: {ch:?}").into()),
            }
        }

        if binary && text {
            return Err("can't have text and binary mode at once".into());
        }

        // A mode with no `r`/`w`/`a` action (`''`, `'b'`, `'t'`) gets
        // CPython's capitalized missing-action message, mirrored verbatim.
        match action {
            Some('w') => Ok(Self::Write(binary)),
            Some('a') => Ok(Self::Append(binary)),
            Some(_) => Ok(Self::Read(binary)),
            None => Err("Must have exactly one of create/read/write/append mode and at most one plus".into()),
        }
    }
}
