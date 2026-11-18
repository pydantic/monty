//! A JSON writer matching pydantic-core's output byte for byte, and the text
//! forms and errors pydantic's JSON mode shares between `dump_json`,
//! `dump_python(mode='json')` and `ValidationError.json()`.
//!
//! pydantic-core serialises through `serde_json`, so this writer follows its
//! rules rather than the `json` module's: compact separators (`,` and `:`),
//! `serde_json`'s pretty layout under `indent`, non-ASCII text left as UTF-8,
//! control characters escaped as `\uXXXX` save the short forms `\b \f \n \r \t`,
//! floats in `zmij`'s shortest form and `null` for an infinite or NaN float.
//! Output grows through a [`StringBuilder`], so an oversized document fails
//! softly at the resource limit.

use std::{fmt, mem, str};

use monty_types::{
    FormatFloat, MontyObject, ObjectRef, ResourceTracker,
    unstable::{self, MontyNode, NodeId},
};
use num_bigint::BigInt;

use crate::{
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    modules::pydantic::iso,
    string_builder::StringBuilder,
    types::long_int::check_bigint_str_digits_limit,
};

/// How a non-finite float is written: `ValidationError.json()` uses the
/// `NaN` / `Infinity` constants, `dump_json` writes `null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InfNan {
    Null,
    Constants,
}

/// Nesting of inferred values pydantic's recursion guard allows; the next
/// level is `Circular reference detected (depth exceeded)`, or `"..."` in
/// `ValidationError.json()`.
pub(crate) const RECURSION_LIMIT: usize = 255;

/// What `ValidationError.json()` writes for a value its recursion guard stops.
const RECURSION_PLACEHOLDER: &str = "...";

/// Accumulates one JSON document.
pub(crate) struct JsonWriter<'t> {
    out: StringBuilder<'t>,
    /// Spaces per nesting level, `None` for compact output.
    indent: Option<usize>,
    inf_nan: InfNan,
    /// Escape every non-ASCII character as `\uXXXX` (`ensure_ascii=True`).
    ensure_ascii: bool,
    /// One flag per open container, innermost last: whether it has had a member written.
    has_member: Vec<bool>,
}

impl<'t> JsonWriter<'t> {
    pub(crate) fn new(indent: Option<usize>, inf_nan: InfNan, tracker: &'t ResourceTracker) -> Self {
        Self {
            out: StringBuilder::new(tracker),
            indent,
            inf_nan,
            ensure_ascii: false,
            has_member: Vec::new(),
        }
    }

    /// Escapes non-ASCII text when `ensure_ascii` is set.
    pub(crate) fn with_ensure_ascii(mut self, ensure_ascii: bool) -> Self {
        self.ensure_ascii = ensure_ascii;
        self
    }

    /// The finished document.
    pub(crate) fn finish(self) -> RunResult<String> {
        self.out.finish_raw()
    }

    pub(crate) fn null(&mut self) -> RunResult<()> {
        Ok(self.out.push_str("null")?)
    }

    pub(crate) fn bool(&mut self, value: bool) -> RunResult<()> {
        Ok(self.out.push_str(bool_text(value))?)
    }

    pub(crate) fn int(&mut self, value: i64) -> RunResult<()> {
        Ok(self.out.push_str(itoa::Buffer::new().format(value))?)
    }

    pub(crate) fn bigint(&mut self, value: &BigInt) -> RunResult<()> {
        Ok(self.out.push_str(&value.to_string())?)
    }

    /// A float in `serde_json`'s form; a non-finite one follows [`InfNan`].
    pub(crate) fn float(&mut self, value: f64) -> RunResult<()> {
        if value.is_finite() {
            Ok(self.out.push_str(zmij::Buffer::new().format_finite(value))?)
        } else if self.inf_nan == InfNan::Null {
            self.null()
        } else if value.is_nan() {
            Ok(self.out.push_str("NaN")?)
        } else if value.is_sign_negative() {
            Ok(self.out.push_str("-Infinity")?)
        } else {
            Ok(self.out.push_str("Infinity")?)
        }
    }

    pub(crate) fn string(&mut self, value: &str) -> RunResult<()> {
        self.out.push('"')?;
        let mut start = 0;
        for (index, c) in value.char_indices() {
            let escape: &str = match c {
                '"' => "\\\"",
                '\\' => "\\\\",
                '\n' => "\\n",
                '\r' => "\\r",
                '\t' => "\\t",
                '\u{8}' => "\\b",
                '\u{c}' => "\\f",
                '\0'..='\u{1f}' => "",
                _ if self.ensure_ascii && !c.is_ascii() => "",
                _ => continue,
            };
            self.out.push_str(&value[start..index])?;
            if escape.is_empty() {
                // `\uXXXX`, as a surrogate pair past the BMP.
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    self.out.push_str("\\u")?;
                    for shift in [12, 8, 4, 0] {
                        self.out.push(HEX[usize::from((*unit >> shift) & 0xf)] as char)?;
                    }
                }
            } else {
                self.out.push_str(escape)?;
            }
            start = index + c.len_utf8();
        }
        self.out.push_str(&value[start..])?;
        Ok(self.out.push('"')?)
    }

    pub(crate) fn begin_array(&mut self) -> RunResult<()> {
        self.begin('[')
    }

    pub(crate) fn end_array(&mut self) -> RunResult<()> {
        self.end(']')
    }

    pub(crate) fn begin_object(&mut self) -> RunResult<()> {
        self.begin('{')
    }

    pub(crate) fn end_object(&mut self) -> RunResult<()> {
        self.end('}')
    }

    /// Starts an array element: the separator and, when indenting, the newline
    /// and indentation that precede it.
    pub(crate) fn element(&mut self) -> RunResult<()> {
        self.member_start()
    }

    /// Writes an object key and its separator, leaving the value to the caller.
    pub(crate) fn key(&mut self, key: &str) -> RunResult<()> {
        self.member_start()?;
        self.string(key)?;
        Ok(self.out.push_str(if self.indent.is_some() { ": " } else { ":" })?)
    }

    /// Writes a `ValidationError` payload value as pydantic's `json()` does:
    /// inferred as `dump_json` infers, but a value of no JSON type writes its
    /// text, and a reference back to an enclosing container, or nesting past
    /// [`RECURSION_LIMIT`], writes `"..."`.
    pub(crate) fn object(&mut self, value: &MontyObject) -> RunResult<()> {
        self.node(value.as_ref(), 1)
    }

    fn begin(&mut self, open: char) -> RunResult<()> {
        self.out.push(open)?;
        self.has_member.push(false);
        Ok(())
    }

    fn end(&mut self, close: char) -> RunResult<()> {
        if self.has_member.pop().unwrap_or(false) {
            self.newline()?;
        }
        Ok(self.out.push(close)?)
    }

    fn member_start(&mut self) -> RunResult<()> {
        let first = match self.has_member.last_mut() {
            Some(flag) => !mem::replace(flag, true),
            None => true,
        };
        if !first {
            self.out.push(',')?;
        }
        self.newline()
    }

    /// Under `indent`, a newline followed by the current level's indentation.
    fn newline(&mut self) -> RunResult<()> {
        if let Some(indent) = self.indent {
            self.out.push('\n')?;
            self.out
                .push_repeated(' ', self.has_member.len().saturating_mul(indent))?;
        }
        Ok(())
    }

    /// One payload node, `depth` inferred values down.
    fn node(&mut self, value: ObjectRef<'_>, depth: usize) -> RunResult<()> {
        if depth > RECURSION_LIMIT {
            return self.string(RECURSION_PLACEHOLDER);
        }
        match unstable::node(value) {
            MontyNode::None => self.null(),
            MontyNode::Bool(b) => self.bool(*b),
            MontyNode::Int(i) => self.int(*i),
            MontyNode::BigInt(b) => self.bigint(b),
            MontyNode::Float(f) => self.float(*f),
            MontyNode::String(s) | MontyNode::Repr(s) | MontyNode::Path(s) | MontyNode::Decimal(s) => self.string(s),
            MontyNode::Cycle(_) => self.string(RECURSION_PLACEHOLDER),
            MontyNode::Bytes(b) => self.string(Surface::ErrorJson.utf8_value(b)?),
            MontyNode::List(items) | MontyNode::Tuple(items) | MontyNode::Set(items) | MontyNode::FrozenSet(items) => {
                self.array(value, items, depth)
            }
            MontyNode::NamedTuple { values, .. } => self.array(value, values, depth),
            MontyNode::Dict(pairs) => {
                self.begin_object()?;
                for (key, item) in pairs {
                    self.key(&node_key(unstable::child(value, *key), self.inf_nan)?)?;
                    self.node(unstable::child(value, *item), depth + 1)?;
                }
                self.end_object()
            }
            other => self.string(&node_text(value, other)),
        }
    }

    fn array(&mut self, holder: ObjectRef<'_>, items: &[NodeId], depth: usize) -> RunResult<()> {
        self.begin_array()?;
        for item in items {
            self.element()?;
            self.node(unstable::child(holder, *item), depth + 1)?;
        }
        self.end_array()
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// The text pydantic writes for a scalar node with no JSON type of its own:
/// ISO text, a complex's parts, an exception's message, `str()` of the rest.
/// Snapshots keep the repr of a value they cannot hold, and Monty writes that.
fn node_text(value: ObjectRef<'_>, node: &MontyNode) -> String {
    match node {
        MontyNode::String(s) | MontyNode::Repr(s) | MontyNode::Path(s) | MontyNode::Decimal(s) => s.clone(),
        MontyNode::Complex(c) => complex_text(c.real, c.imag),
        MontyNode::Date(d) => iso::date(d),
        MontyNode::DateTime(dt) => iso::datetime(dt),
        MontyNode::Time(t) => iso::time(t),
        MontyNode::TimeDelta(td) => iso::timedelta(td),
        MontyNode::Exception { exc_type, arg } => match arg {
            Some(arg) => SimpleException::new_msg(*exc_type, arg),
            None => SimpleException::new_none(*exc_type),
        }
        .py_str(),
        MontyNode::Ellipsis => "Ellipsis".to_owned(),
        MontyNode::NotImplemented => "NotImplemented".to_owned(),
        _ => value.py_repr(),
    }
}

/// A payload dict key as pydantic's `json()` writes it: a tuple's items
/// joined by commas, containers refused, scalars as their text.
fn node_key(key: ObjectRef<'_>, inf_nan: InfNan) -> RunResult<String> {
    let surface = Surface::ErrorJson;
    match unstable::node(key) {
        MontyNode::None => Ok("None".to_owned()),
        MontyNode::Bool(b) => Ok(bool_text(*b).to_owned()),
        MontyNode::Int(i) => Ok(i.to_string()),
        MontyNode::BigInt(b) => surface.bigint_key(b),
        MontyNode::Float(f) => Ok(float_key(*f, inf_nan)),
        MontyNode::Cycle(_) => Ok(RECURSION_PLACEHOLDER.to_owned()),
        MontyNode::Bytes(b) => surface.utf8_key(b).map(str::to_owned),
        MontyNode::Tuple(items) | MontyNode::NamedTuple { values: items, .. } => {
            let parts = items
                .iter()
                .map(|item| node_key(unstable::child(key, *item), inf_nan))
                .collect::<RunResult<Vec<_>>>()?;
            Ok(parts.join(","))
        }
        MontyNode::List(_) => Err(surface.invalid_key("list")),
        MontyNode::Set(_) => Err(surface.invalid_key("set")),
        MontyNode::FrozenSet(_) => Err(surface.invalid_key("frozenset")),
        MontyNode::Dict(_) => Err(surface.invalid_key("dict")),
        other => Ok(node_text(key, other)),
    }
}

/// Which pydantic entry point is rendering JSON-mode output, which decides
/// how an error met on the way is raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    /// `dump_python(mode='json')`: the error as it is.
    Python,
    /// `dump_json`: a `PydanticSerializationError`.
    DumpJson,
    /// `ValidationError.json()`: a `ValueError`.
    ErrorJson,
}

impl Surface {
    /// `err` as raised here: past `mode='json'` an exception becomes
    /// `{prefix}{Type}: {message}`. Resource errors and pydantic's own
    /// serialization errors pass through.
    pub(crate) fn wrap(self, err: RunError) -> RunError {
        match err {
            RunError::Exc(raise)
                if self != Self::Python && raise.exc.exc_type() != ExcType::PydanticSerializationError =>
            {
                self.serde_error(format_args!("{}: {}", raise.exc.exc_type(), raise.exc.py_str()))
            }
            err => err,
        }
    }

    /// A failure inside `serde_json`, which has no Python exception, after the surface's prefix.
    pub(crate) fn serde_error(self, message: impl fmt::Display) -> RunError {
        let (exc_type, prefix) = match self {
            Self::Python | Self::DumpJson => (ExcType::PydanticSerializationError, "Error serializing to JSON"),
            Self::ErrorJson => (ExcType::ValueError, "Error serializing ValidationError to JSON"),
        };
        SimpleException::new_msg(exc_type, format_args!("{prefix}: {message}")).into()
    }

    /// Bytes as UTF-8 text for a value: a document reports `serde_json`'s
    /// error, `mode='json'` Python's `UnicodeDecodeError`.
    pub(crate) fn utf8_value(self, bytes: &[u8]) -> RunResult<&str> {
        str::from_utf8(bytes).map_err(|err| match self {
            Self::Python => utf8_decode_error(bytes, err),
            Self::DumpJson | Self::ErrorJson => self.serde_error(err),
        })
    }

    /// Bytes as UTF-8 text for a key, which pydantic decodes through Python.
    pub(crate) fn utf8_key(self, bytes: &[u8]) -> RunResult<&str> {
        str::from_utf8(bytes).map_err(|err| self.wrap(utf8_decode_error(bytes, err)))
    }

    /// A big-int key's digits, under the `int` string limit that pydantic's
    /// `str()` of the key meets.
    pub(crate) fn bigint_key(self, big: &BigInt) -> RunResult<String> {
        check_bigint_str_digits_limit(big).map_err(|err| self.wrap(err))?;
        Ok(big.to_string())
    }

    /// pydantic's error for a container used as a key.
    pub(crate) fn invalid_key(self, type_name: &str) -> RunError {
        self.wrap(ExcType::type_error(format!("`{type_name}` not valid as object key")))
    }
}

/// The `UnicodeDecodeError` pydantic raises for bytes that are not UTF-8.
fn utf8_decode_error(bytes: &[u8], err: str::Utf8Error) -> RunError {
    let start = err.valid_up_to();
    ExcType::unicode_decode_error("utf-8", bytes, start, start + 1, "invalid utf-8")
}

/// `true` / `false`, a bool's text as a value and as a key.
pub(crate) fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// A float key: Python's `str()` of it, or `None` for a non-finite one when
/// such floats are written as `null`.
pub(crate) fn float_key(value: f64, inf_nan: InfNan) -> String {
    if value.is_finite() || inf_nan == InfNan::Constants {
        FormatFloat(value).to_string()
    } else {
        "None".to_owned()
    }
}

/// pydantic's text for a complex, its parts in Rust's float formatting:
/// `1+2j`, `-0j`, `NaN+infj`; a zero real part is left out.
pub(crate) fn complex_text(real: f64, imag: f64) -> String {
    if real == 0.0 {
        format!("{imag}j")
    } else if imag.is_nan() || imag.is_sign_positive() {
        format!("{real}+{imag}j")
    } else {
        format!("{real}{imag}j")
    }
}
