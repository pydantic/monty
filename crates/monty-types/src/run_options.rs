//! Compile-time configuration: [`CompileOptions`] and the
//! [`AssertMessageAnnotations`] introspected-assert setting.

use std::num::NonZeroU32;

/// Byte length up to which a source is compiled without the pre-parse nesting
/// scan; the default for [`CompileOptions::source_scan_threshold`].
///
/// ruff grows its parser stack on demand, outside the sandbox allocator, at
/// roughly 2 KiB per nesting level, and a source cannot nest deeper than it is
/// long. 4 KiB (about 40 lines of 100 characters) caps that untracked growth
/// at ~8 MiB while sparing ordinary programs the extra lexer pass.
pub const SOURCE_SCAN_THRESHOLD: usize = 4 * 1024;

/// Options controlling how Monty behavior diverges from plain CPython.
///
/// Consumed when code is compiled: a `MontyRun` bakes the choices into the
/// program at construction, while a `MontyRepl` stores them so every snippet
/// fed to the session compiles the same way.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct CompileOptions {
    /// Give failed `assert` statements pytest-style introspected messages,
    /// deliberately diverging from CPython; see `limitations/assert.md`.
    /// On by default with a 120-byte operand-repr truncation.
    pub assert_message_annotations: AssertMessageAnnotations,
    /// Sources longer than this many bytes are scanned for parser nesting
    /// before ruff sees them (see `limitations/language.md`); `0` scans every
    /// source and `usize::MAX` none. Defaults to [`SOURCE_SCAN_THRESHOLD`].
    pub source_scan_threshold: usize,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            assert_message_annotations: AssertMessageAnnotations::default(),
            source_scan_threshold: SOURCE_SCAN_THRESHOLD,
        }
    }
}

/// Controls the pytest-style introspected `assert` failure messages of
/// [`CompileOptions::assert_message_annotations`].
///
/// The choice is baked in at compile time (whether the introspecting opcodes
/// are emitted) but the truncation limit is applied at runtime, so it also
/// travels with serialized sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AssertMessageAnnotations {
    /// Disable introspection; bare asserts use CPython's empty message.
    Off,
    /// Retain at most this many UTF-8 bytes per operand before any `…` suffix.
    /// Non-zero because `0` encodes [`Off`](Self::Off) on the wire.
    MaxBytes(NonZeroU32),
}

impl AssertMessageAnnotations {
    /// Operand-repr truncation used by [`Default`] and `From<bool>`.
    pub const DEFAULT_MAX_BYTES: NonZeroU32 = NonZeroU32::new(120).expect("120 is non-zero");

    /// Whether the compiler should emit introspecting assert opcodes.
    #[must_use]
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Returns the wire value: `0` when disabled, otherwise the UTF-8 byte cap.
    #[must_use]
    pub fn max_bytes(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::MaxBytes(n) => n.get(),
        }
    }

    /// Decodes the wire value: `0` is off and any other value is the byte cap.
    #[must_use]
    pub fn from_max_bytes(value: u32) -> Self {
        match NonZeroU32::new(value) {
            Some(n) => Self::MaxBytes(n),
            None => Self::Off,
        }
    }
}

impl Default for AssertMessageAnnotations {
    fn default() -> Self {
        Self::MaxBytes(Self::DEFAULT_MAX_BYTES)
    }
}

impl From<bool> for AssertMessageAnnotations {
    /// `true` enables the 120-byte default; `false` disables annotations.
    fn from(enabled: bool) -> Self {
        if enabled { Self::default() } else { Self::Off }
    }
}
