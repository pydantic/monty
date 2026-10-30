//! A JSON writer matching pydantic-core's output byte for byte.
//!
//! pydantic-core serialises through `serde_json`, so this writer follows its
//! rules rather than the `json` module's: compact separators (`,` and `:`),
//! `serde_json`'s pretty layout under `indent`, non-ASCII text left as UTF-8,
//! control characters escaped as `\uXXXX` save the short forms `\b \f \n \r \t`,
//! floats in `zmij`'s shortest form and `null` for an infinite or NaN float.
//! Output grows through a [`StringBuilder`], so an oversized document fails
//! softly at the resource limit.

use std::mem;

use monty_types::{
    MontyObject, ResourceTracker,
    unstable::{self, MontyNode, NodeId},
};
use num_bigint::BigInt;

use crate::{exception_private::RunResult, modules::pydantic::iso, string_builder::StringBuilder};

/// How a non-finite float is written: `ValidationError.json()` uses the
/// `NaN` / `Infinity` constants, `dump_json` writes `null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InfNan {
    Null,
    Constants,
}

/// Accumulates one JSON document.
pub(crate) struct JsonWriter<'t> {
    out: StringBuilder<'t>,
    /// Spaces per nesting level, `None` for compact output.
    indent: Option<usize>,
    inf_nan: InfNan,
    /// The current nesting level, for indentation.
    depth: usize,
    /// Whether the innermost open container has had a member written.
    has_member: Vec<bool>,
}

impl<'t> JsonWriter<'t> {
    pub(crate) fn new(indent: Option<usize>, inf_nan: InfNan, tracker: &'t ResourceTracker) -> Self {
        Self {
            out: StringBuilder::new(tracker),
            indent,
            inf_nan,
            depth: 0,
            has_member: Vec::new(),
        }
    }

    /// The finished document.
    pub(crate) fn finish(self) -> RunResult<String> {
        self.out.finish_raw()
    }

    pub(crate) fn null(&mut self) -> RunResult<()> {
        Ok(self.out.push_str("null")?)
    }

    pub(crate) fn bool(&mut self, value: bool) -> RunResult<()> {
        Ok(self.out.push_str(if value { "true" } else { "false" })?)
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
        for (index, byte) in value.bytes().enumerate() {
            let escape: &str = match byte {
                b'"' => "\\\"",
                b'\\' => "\\\\",
                b'\n' => "\\n",
                b'\r' => "\\r",
                b'\t' => "\\t",
                0x08 => "\\b",
                0x0c => "\\f",
                0x00..=0x1f => "",
                _ => continue,
            };
            self.out.push_str(&value[start..index])?;
            if escape.is_empty() {
                self.out.push_str("\\u00")?;
                self.out.push(HEX[usize::from(byte >> 4)] as char)?;
                self.out.push(HEX[usize::from(byte & 0xf)] as char)?;
            } else {
                self.out.push_str(escape)?;
            }
            start = index + 1;
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

    fn begin(&mut self, open: char) -> RunResult<()> {
        self.out.push(open)?;
        self.depth += 1;
        self.has_member.push(false);
        Ok(())
    }

    fn end(&mut self, close: char) -> RunResult<()> {
        self.depth -= 1;
        let had_member = self.has_member.pop().unwrap_or(false);
        if had_member {
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
            for _ in 0..self.depth * indent {
                self.out.push(' ')?;
            }
        }
        Ok(())
    }

    /// Writes a snapshot as pydantic serialises inferred values in JSON mode:
    /// temporal values as ISO strings, bytes as text, sets and tuples as arrays.
    pub(crate) fn object(&mut self, value: &MontyObject) -> RunResult<()> {
        let (graph, root) = unstable::graph_parts(value);
        self.node(graph.nodes(), root)
    }

    fn node(&mut self, nodes: &[MontyNode], id: NodeId) -> RunResult<()> {
        match &nodes[id.index()] {
            MontyNode::None | MontyNode::Ellipsis | MontyNode::NotImplemented => self.null(),
            MontyNode::Bool(b) => self.bool(*b),
            MontyNode::Int(i) => self.int(*i),
            MontyNode::BigInt(b) => self.bigint(b),
            MontyNode::Float(f) => self.float(*f),
            MontyNode::String(s) | MontyNode::Repr(s) | MontyNode::Cycle(s) | MontyNode::Path(s) => self.string(s),
            MontyNode::Bytes(b) => self.string(&String::from_utf8_lossy(b)),
            MontyNode::Decimal(d) => self.string(d),
            MontyNode::Date(d) => self.string(&iso::date(d)),
            MontyNode::DateTime(dt) => self.string(&iso::datetime(dt)),
            MontyNode::Time(t) => self.string(&iso::time(t)),
            MontyNode::TimeDelta(td) => self.string(&iso::timedelta(td)),
            MontyNode::List(items) | MontyNode::Tuple(items) | MontyNode::Set(items) | MontyNode::FrozenSet(items) => {
                self.array(nodes, items)
            }
            MontyNode::NamedTuple { values, .. } => self.array(nodes, values),
            MontyNode::Dict(pairs) => {
                self.begin_object()?;
                for (key, value) in pairs {
                    let key_text = json_key(&nodes[key.index()]);
                    self.key(&key_text)?;
                    self.node(nodes, *value)?;
                }
                self.end_object()
            }
            other => self.string(&unstable::object_from_node(other.clone()).py_repr()),
        }
    }

    fn array(&mut self, nodes: &[MontyNode], items: &[NodeId]) -> RunResult<()> {
        self.begin_array()?;
        for item in items {
            self.element()?;
            self.node(nodes, *item)?;
        }
        self.end_array()
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// A dict key as a JSON object key: strings verbatim, the rest as pydantic
/// renders them (`None`, `true`, a number's digits, a date's ISO form).
fn json_key(node: &MontyNode) -> String {
    match node {
        MontyNode::String(s) | MontyNode::Repr(s) => s.clone(),
        MontyNode::None => "None".to_owned(),
        MontyNode::Bool(b) => if *b { "true" } else { "false" }.to_owned(),
        MontyNode::Float(f) => zmij::Buffer::new().format_finite(*f).to_owned(),
        MontyNode::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        MontyNode::Date(d) => iso::date(d),
        MontyNode::DateTime(dt) => iso::datetime(dt),
        MontyNode::Time(t) => iso::time(t),
        other => unstable::object_from_node(other.clone()).py_repr(),
    }
}
