//! Newline handling shared by `open()` text files and `io.StringIO`.
//!
//! Both take CPython's `newline` argument with the same meaning: where
//! `readline()` splits, whether `\r\n` / `\r` are translated to `\n`, and what
//! `\n` becomes on write. Files translate when their content is loaded;
//! `StringIO` translates as text is written, as CPython's does.

use crate::{
    heap::Heap,
    types::{str::allocate_string, tuple::allocate_tuple},
    value::Value,
};

/// The `newline` argument: where lines end on read, whether `\r\n` / `\r` are
/// translated to `\n`, and what `\n` becomes on write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum Newline {
    /// `None`: universal newlines translated to `\n` on read; `\n` written as is,
    /// since the sandbox's `os.linesep` is `\n` on every host.
    #[default]
    Universal,
    /// `''`: universal line splitting with terminators kept; nothing translated.
    Passthrough,
    /// `'\n'`: lines end at `\n` only; nothing translated.
    Lf,
    /// `'\r'`: lines end at `\r`; `\n` is written as `\r`.
    Cr,
    /// `'\r\n'`: lines end at `\r\n`; `\n` is written as `\r\n`.
    CrLf,
}

impl Newline {
    /// Parses the str form of the argument; `None` for a value CPython rejects.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "" => Some(Self::Passthrough),
            "\n" => Some(Self::Lf),
            "\r" => Some(Self::Cr),
            "\r\n" => Some(Self::CrLf),
            _ => None,
        }
    }

    /// Whether `\r\n` and `\r` become `\n` on input.
    pub(crate) fn translates_on_read(self) -> bool {
        matches!(self, Self::Universal)
    }

    /// Whether `readline()` splits on any of `\r`, `\n` and `\r\n`, which is
    /// also when CPython tracks the `newlines` attribute.
    pub(crate) fn is_universal(self) -> bool {
        matches!(self, Self::Universal | Self::Passthrough)
    }

    /// Byte length of the first line of `tail` including its terminator, or all
    /// of `tail` for an unterminated last line. `Universal` text was translated
    /// on input, so only `\n` is left to find.
    pub(crate) fn line_len(self, tail: &str) -> usize {
        match self {
            Self::Universal | Self::Lf => tail.find('\n').map_or(tail.len(), |i| i + 1),
            Self::Cr => tail.find('\r').map_or(tail.len(), |i| i + 1),
            Self::CrLf => tail.find("\r\n").map_or(tail.len(), |i| i + 2),
            Self::Passthrough => match tail.find(['\r', '\n']) {
                Some(i) if tail[i..].starts_with("\r\n") => i + 2,
                Some(i) => i + 1,
                None => tail.len(),
            },
        }
    }

    /// Applies the write-side translation: `\n` becomes `\r` or `\r\n` for
    /// those modes, every other mode writes `text` as passed. Also returns the
    /// original char count when the payload changed, since `write()` reports
    /// the length of the text it was given rather than what was stored.
    pub(crate) fn translate_write(self, text: String) -> (String, Option<usize>) {
        let replacement = match self {
            Self::Cr => "\r",
            Self::CrLf => "\r\n",
            Self::Universal | Self::Passthrough | Self::Lf => return (text, None),
        };
        if text.contains('\n') {
            let chars = text.chars().count();
            (text.replace('\n', replacement), Some(chars))
        } else {
            (text, None)
        }
    }
}

/// Rewrites `\r\n` and lone `\r` to `\n` in one pass: universal newlines on
/// input. The output is never longer than the input, so a plain `String` is fine.
pub(crate) fn translate_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// The kinds of line ending seen so far, behind the `newlines` attribute of
/// universal-newline streams (CPython's `IncrementalNewlineDecoder.seennl`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct SeenNewlines(u8);

impl SeenNewlines {
    /// A lone `\r`.
    const CR: u8 = 1;
    /// A `\n` not preceded by `\r`.
    const LF: u8 = 2;
    /// A `\r\n` pair.
    const CRLF: u8 = 4;

    /// Records the line endings in `text`, which must be untranslated input.
    pub(crate) fn record(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut i = 0;
        // Line endings are ASCII, so scanning bytes never splits a char.
        while let Some(offset) = bytes[i..].iter().position(|&b| b == b'\r' || b == b'\n') {
            let at = i + offset;
            if bytes[at] == b'\n' {
                self.0 |= Self::LF;
                i = at + 1;
            } else if bytes.get(at + 1) == Some(&b'\n') {
                self.0 |= Self::CRLF;
                i = at + 2;
            } else {
                self.0 |= Self::CR;
                i = at + 1;
            }
        }
    }

    /// The `newlines` value: `None`, one terminator, or a tuple of them in
    /// CPython's fixed `('\r', '\n', '\r\n')` order.
    pub(crate) fn to_value(self, heap: &Heap) -> Value {
        let kinds: Vec<&str> = [(Self::CR, "\r"), (Self::LF, "\n"), (Self::CRLF, "\r\n")]
            .into_iter()
            .filter(|(flag, _)| self.0 & flag != 0)
            .map(|(_, text)| text)
            .collect();
        match kinds.as_slice() {
            [] => Value::None,
            [one] => allocate_string(*one, heap),
            many => allocate_tuple(many.iter().map(|text| allocate_string(*text, heap)).collect(), heap),
        }
    }
}
