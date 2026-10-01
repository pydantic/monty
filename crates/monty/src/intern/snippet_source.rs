//! Source text of snippets compiled into a running session, and its dump encoding.

use std::{fmt, sync::Arc};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// Source text compiled into a running session, kept so tracebacks through
/// its frames can show line numbers and carets long after it ran.
///
/// Two kinds share the table: `exec()` / `eval()` snippets, and REPL inputs
/// (`<python-input-N>`, including [`MontyRepl::call_function`](crate::MontyRepl::call_function)'s
/// synthetic call site). Entries are never removed: a function defined by a
/// snippet, or a saved exception, may produce a traceback frame pointing at it
/// at any later time.
///
/// # Dump encoding
///
/// An `exec()` / `eval()` source is written as a bare string, exactly as
/// Monty 1.0.0 wrote every entry of this table, so those dumps load unchanged.
/// A REPL input is written as a `{filename, text}` map; builds before REPL
/// inputs moved here cannot read such an entry.
#[derive(Debug, Clone)]
pub(crate) struct SnippetSource {
    /// Displayed filename, or `None` for `exec()` / `eval()`, which CPython
    /// shows as `<string>` with no source line, since it has nothing to read back.
    filename: Option<Box<str>>,
    /// The complete source text the snippet was compiled from.
    text: Arc<str>,
}

impl SnippetSource {
    /// An `exec()` / `eval()` snippet, displayed as `<string>` without a source line.
    pub(crate) fn eval(text: Arc<str>) -> Self {
        Self { filename: None, text }
    }

    /// A REPL input, displayed under `filename` with its source lines shown.
    pub(crate) fn named(filename: &str, text: Arc<str>) -> Self {
        Self {
            filename: Some(filename.into()),
            text,
        }
    }

    /// The source text frames in this snippet resolve their positions against.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Whether traceback frames in this snippet print the offending source line.
    pub(crate) fn shows_source_line(&self) -> bool {
        self.filename.is_some()
    }

    /// The filename tracebacks display for this snippet.
    pub(super) fn display_filename(&self) -> &str {
        self.filename.as_deref().unwrap_or("<string>")
    }
}

impl Serialize for SnippetSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.filename {
            None => serializer.serialize_str(&self.text),
            Some(filename) => {
                let mut map = serializer.serialize_struct("SnippetSource", 2)?;
                map.serialize_field("filename", filename)?;
                map.serialize_field("text", &self.text)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for SnippetSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(SnippetSourceVisitor)
    }
}

/// Reads either encoding described on [`SnippetSource`]: a string is an
/// `exec()` / `eval()` source, a map a REPL input. Unknown map keys are
/// skipped, as the dump naming contract requires.
struct SnippetSourceVisitor;

impl<'de> Visitor<'de> for SnippetSourceVisitor {
    type Value = SnippetSource;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a snippet source string or a {filename, text} map")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<SnippetSource, E> {
        Ok(SnippetSource::eval(Arc::from(text)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<SnippetSource, A::Error> {
        let mut filename: Option<Box<str>> = None;
        let mut text: Option<Arc<str>> = None;
        while let Some(key) = map.next_key::<Box<str>>()? {
            match &*key {
                "filename" => filename = Some(map.next_value()?),
                "text" => text = Some(map.next_value()?),
                _ => {
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
        }
        Ok(SnippetSource {
            filename: Some(filename.ok_or_else(|| de::Error::missing_field("filename"))?),
            text: text.ok_or_else(|| de::Error::missing_field("text"))?,
        })
    }
}
