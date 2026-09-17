//! Logfire-style JSON encoding of arena values for telemetry attributes.
//!
//! Mirrors the Python logfire JSON encoder (`logfire/_internal/json_encoder.py`):
//! containers become JSON arrays/objects, dates use isoformat, timedeltas their
//! total seconds, and opaque objects fall back to their `repr`. Output is capped
//! at a byte limit so a huge value cannot blow up the telemetry pipeline.
//!
//! Values are rendered from the wire arena (`&[MontyNode]` plus a root id),
//! which the pool may not have validated yet. A child id out of range, or not
//! strictly lower than its holder, renders as a placeholder instead of being
//! followed, so a hostile arena cannot loop or index out of bounds; nesting
//! stops at [`MAX_JSON_DEPTH`].
//!
//! Divergences from the Python encoder: sets are encoded in storage order
//! (Python sorts them when comparable), integers beyond `i128` become their
//! digit string rather than a raw JSON number, non-string dict keys render as
//! their JSON encoding, and class instances and class objects mirror their
//! node shape (`{type, id, attrs}` and the class fields) so telemetry records
//! *which* object crossed the boundary, not just its attribute snapshot.

use std::{
    fmt::{self, Write as _},
    io::{self, Write},
};

use monty_types::{MontyDateTime, MontyNode, MontyTime, NodeId, bytes_repr};
use num_traits::ToPrimitive;
use serde::ser::{Error as _, Serialize, SerializeMap, Serializer};

/// Nesting past which a value renders as `"<too deep>"`: the encoder recurses
/// per level, and the arena's index guard alone allows one level per node.
const MAX_JSON_DEPTH: usize = 64;

/// Placeholder for an id that is out of range or not below its holder.
const INVALID_PLACEHOLDER: &str = "<invalid>";

/// Serializes the value rooted at `id` to logfire-style JSON (see the module
/// docs), capped at `limit` bytes. The bool is true when the cap cut
/// serialization short — the partial output is then no longer valid JSON, so
/// the caller should mark it truncated.
pub(crate) fn serialize_capped(nodes: &[MontyNode], id: NodeId, limit: usize) -> (String, bool) {
    capped(&JsonEncoded::root(nodes, id, limit), limit)
}

/// [`serialize_capped`] for borrowed root ids: a JSON array.
pub(crate) fn serialize_seq_capped(nodes: &[MontyNode], ids: &[NodeId], limit: usize) -> (String, bool) {
    capped(&JsonSeq { nodes, ids, limit }, limit)
}

/// [`serialize_capped`] for borrowed `(key, value)` root ids; non-string keys
/// render as their JSON encoding.
pub(crate) fn serialize_dict_capped(nodes: &[MontyNode], pairs: &[(NodeId, NodeId)], limit: usize) -> (String, bool) {
    capped(&JsonDict { nodes, pairs, limit }, limit)
}

/// [`serialize_capped`] for borrowed name → root pairs, as a JSON object.
#[cfg(test)]
pub(crate) fn serialize_named_capped(nodes: &[MontyNode], pairs: &[(&str, NodeId)], limit: usize) -> (String, bool) {
    serialize_named_iter_capped(nodes, pairs.iter().copied(), pairs.len(), limit)
}

/// [`serialize_capped`] for an iterator of borrowed name → root pairs.
pub(crate) fn serialize_named_iter_capped<'a>(
    nodes: &'a [MontyNode],
    pairs: impl Clone + Iterator<Item = (&'a str, NodeId)>,
    len: usize,
    limit: usize,
) -> (String, bool) {
    capped(
        &JsonNamed {
            nodes,
            pairs,
            len,
            limit,
        },
        limit,
    )
}

/// Serializes any serde value while bounding the generated JSON.
pub(crate) fn serialize_value_capped(value: &impl Serialize, limit: usize) -> (String, bool) {
    capped(value, limit)
}

/// Serializes into a [`CappedWriter`]; the bool reports the cap cutting
/// serialization short.
fn capped(value: &impl Serialize, limit: usize) -> (String, bool) {
    let mut writer = CappedWriter { buf: Vec::new(), limit };
    let cut = serde_json::to_writer(&mut writer, value).is_err();
    // the buffer holds whole serializer writes, so it is valid UTF-8;
    // from_utf8_lossy just avoids a panic path
    (String::from_utf8_lossy(&writer.buf).into_owned(), cut)
}

/// An in-memory writer that rejects any write taking it past `limit` bytes,
/// aborting serialization instead of building an unbounded string.
struct CappedWriter {
    buf: Vec<u8>,
    limit: usize,
}

impl Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len() + data.len() > self.limit {
            Err(io::Error::other("byte limit reached"))
        } else {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serializes one arena node with the logfire value mapping (rather than the
/// tagged-enum encoding of the derived `Serialize`, which is for dumps).
///
/// `limit` is carried down the value tree so a huge `bytes` leaf is escaped
/// only as far as the cap can keep — the writer alone cannot help, since it
/// sees the escaped string only once it is built. An `id` out of range or
/// equal to [`INVALID_ID`] renders as [`INVALID_PLACEHOLDER`].
struct JsonEncoded<'a> {
    nodes: &'a [MontyNode],
    id: NodeId,
    limit: usize,
    depth: usize,
}

/// Replaces a child id that is not below its holder; never in range, so it
/// renders as [`INVALID_PLACEHOLDER`].
const INVALID_ID: NodeId = NodeId(u32::MAX);

impl<'a> JsonEncoded<'a> {
    /// An encoder for a root named by the message.
    const fn root(nodes: &'a [MontyNode], id: NodeId, limit: usize) -> Self {
        Self {
            nodes,
            id,
            limit,
            depth: 0,
        }
    }

    /// An encoder for a child of this node. A child id not below its holder is
    /// invalid in a post-order arena and is not followed, so a hostile arena
    /// cannot make the encoder loop.
    const fn nested(&self, child: NodeId) -> Self {
        let id = if child.0 < self.id.0 { child } else { INVALID_ID };
        Self {
            nodes: self.nodes,
            id,
            limit: self.limit,
            depth: self.depth + 1,
        }
    }

    /// The node at `id`, or `None` when it is out of range.
    fn node(&self) -> Option<&'a MontyNode> {
        self.nodes.get(self.id.0 as usize)
    }
}

impl Serialize for JsonEncoded<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Some(node) = self.node() else {
            return s.serialize_str(INVALID_PLACEHOLDER);
        };
        if self.depth > MAX_JSON_DEPTH {
            return s.serialize_str("<too deep>");
        }
        match node {
            MontyNode::None => s.serialize_unit(),
            MontyNode::Bool(b) => s.serialize_bool(*b),
            MontyNode::Int(i) => s.serialize_i64(*i),
            // beyond i128 the digits become a string: a raw JSON number would
            // need serde_json's arbitrary-precision mode
            MontyNode::BigInt(b) => match b.to_i128() {
                Some(i) => s.serialize_i128(i),
                None => s.collect_str(b),
            },
            MontyNode::Float(f) if f.is_finite() => s.serialize_f64(*f),
            MontyNode::Float(f) => s.serialize_str(nonfinite_str(*f)),
            MontyNode::String(v) => s.serialize_str(v),
            // like logfire: the repr's escaped content without the b'' wrapper.
            // Escaping stops at `limit` input bytes — each escapes to at least
            // one character, so the rest would only inflate a huge payload to
            // produce output the cap discards
            MontyNode::Bytes(b) => {
                let repr = bytes_repr(&b[..b.len().min(self.limit)]);
                s.serialize_str(&repr[2..repr.len() - 1])
            }
            MontyNode::List(ids) | MontyNode::Tuple(ids) | MontyNode::Set(ids) | MontyNode::FrozenSet(ids) => {
                s.collect_seq(ids.iter().map(|id| self.nested(*id)))
            }
            // a namedtuple is a tuple in Python, so logfire encodes the values
            // as an array and drops the field names
            MontyNode::NamedTuple { values, .. } => s.collect_seq(values.iter().map(|id| self.nested(*id))),
            MontyNode::Dict(pairs) => serialize_pairs(self, pairs, s),
            // mirrors the node: the class, the instance id, then the eager
            // attrs in order (there are no declared field names — attrs ARE
            // the surface)
            MontyNode::ClassInstance {
                class_type,
                instance_id,
                attrs,
            } => {
                let mut map = s.serialize_map(Some(3))?;
                map.serialize_entry("type", &JsonClassType(self.nested(*class_type)))?;
                map.serialize_entry("id", &Displayed(instance_id))?;
                map.serialize_entry(
                    "attrs",
                    &JsonAttrs {
                        holder: self,
                        pairs: attrs,
                    },
                )?;
                map.end()
            }
            MontyNode::ClassType(_) => JsonClassType(Self { ..*self }).serialize(s),
            MontyNode::Type(builtin) => s.collect_str(&format_args!("<class '{builtin}'>")),
            MontyNode::Date(d) => s.collect_str(&format_args!("{:04}-{:02}-{:02}", d.year, d.month, d.day)),
            MontyNode::DateTime(dt) => s.serialize_str(&datetime_isoformat(dt)),
            MontyNode::Time(t) => s.serialize_str(&time_isoformat(t)),
            // total seconds, accumulated in f64 throughout: an extreme `days`
            // overflows the microseconds of the same sum in i64
            MontyNode::TimeDelta(td) => s.serialize_f64(
                f64::from(td.days).mul_add(86_400.0, f64::from(td.seconds)) + f64::from(td.microseconds) / 1e6,
            ),
            // like logfire: `str(exc)`, which in Python is the message alone
            MontyNode::Exception { arg, .. } => s.serialize_str(arg.as_deref().unwrap_or_default()),
            MontyNode::Path(p) => s.serialize_str(p),
            MontyNode::Cycle(_) => s.serialize_str("<circular reference>"),
            MontyNode::Repr(r) => s.serialize_str(r),
            MontyNode::Ellipsis => s.serialize_str("Ellipsis"),
            MontyNode::NotImplemented => s.serialize_str("NotImplemented"),
            MontyNode::BuiltinFunction(func) => s.collect_str(&format_args!("<built-in function {func}>")),
            MontyNode::Function { name, .. } => s.collect_str(&format_args!("<function '{name}' external>")),
            MontyNode::FileHandle(handle) => s.collect_str(handle),
            MontyNode::TimeZone(tz) => s.collect_str(&TimeZoneRepr(tz)),
        }
    }
}

/// [`JsonEncoded`] for borrowed root ids with no holding node: a JSON array.
struct JsonSeq<'a> {
    nodes: &'a [MontyNode],
    ids: &'a [NodeId],
    limit: usize,
}

impl Serialize for JsonSeq<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.ids.iter().map(|id| JsonEncoded::root(self.nodes, *id, self.limit)))
    }
}

/// [`JsonSeq`] for root pairs: a JSON object keyed like a dict node.
struct JsonDict<'a> {
    nodes: &'a [MontyNode],
    pairs: &'a [(NodeId, NodeId)],
    limit: usize,
}

impl Serialize for JsonDict<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // roots have no holder, so every id is a candidate: give the guard an
        // id above them all
        let holder = JsonEncoded::root(self.nodes, INVALID_ID, self.limit);
        serialize_pairs(&holder, self.pairs, s)
    }
}

/// [`JsonDict`] for a node's attrs, whose ids are children of `holder`.
struct JsonAttrs<'a> {
    holder: &'a JsonEncoded<'a>,
    pairs: &'a [(NodeId, NodeId)],
}

impl Serialize for JsonAttrs<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_pairs(self.holder, self.pairs, s)
    }
}

/// A class node as a JSON object mirroring its fields, `attrs` through the
/// capped dict encoding; anything but a class node renders as [`INVALID_PLACEHOLDER`].
struct JsonClassType<'a>(JsonEncoded<'a>);

impl Serialize for JsonClassType<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Some(MontyNode::ClassType(class)) = self.0.node() else {
            return s.serialize_str(INVALID_PLACEHOLDER);
        };
        let mut map = s.serialize_map(Some(5))?;
        map.serialize_entry("name", &class.name)?;
        map.serialize_entry("id", &Displayed(&class.id))?;
        map.serialize_entry("host_defined", &class.host_defined)?;
        map.serialize_entry("is_dataclass", &class.is_dataclass)?;
        map.serialize_entry(
            "attrs",
            &JsonAttrs {
                holder: &self.0,
                pairs: &class.attrs,
            },
        )?;
        map.end()
    }
}

/// [`JsonSeq`] for named roots: a JSON object of `name` → encoded value.
struct JsonNamed<'a, I> {
    nodes: &'a [MontyNode],
    pairs: I,
    len: usize,
    limit: usize,
}

impl<'a, I> Serialize for JsonNamed<'a, I>
where
    I: Clone + Iterator<Item = (&'a str, NodeId)>,
{
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.len))?;
        for (name, id) in self.pairs.clone() {
            map.serialize_entry(name, &JsonEncoded::root(self.nodes, id, self.limit))?;
        }
        map.end()
    }
}

/// Serializes key/value id pairs (children of `holder`) as a JSON object,
/// string keys verbatim and everything else by its JSON encoding (JSON has no
/// non-string keys).
fn serialize_pairs<S: Serializer>(
    holder: &JsonEncoded<'_>,
    pairs: &[(NodeId, NodeId)],
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut map = s.serialize_map(Some(pairs.len()))?;
    for (key, value) in pairs {
        let key = holder.nested(*key);
        let value = holder.nested(*value);
        if let Some(MontyNode::String(k)) = key.node() {
            map.serialize_entry(k, &value)?;
        } else {
            // a cut key fails the whole encoding: its rendered prefix might
            // fit the outer cap and hide the cut
            let (rendered, cut) = capped(&key, holder.limit);
            if cut {
                return Err(S::Error::custom("byte limit reached"));
            }
            map.serialize_entry(&rendered, &value)?;
        }
    }
    map.end()
}

/// Streams a `Display` rendering (a uuid) through serde's capped writer
/// rather than materializing it first.
struct Displayed<'a>(&'a dyn fmt::Display);

impl Serialize for Displayed<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self.0)
    }
}

/// The repr a timezone value would have had as a `MontyObject`.
struct TimeZoneRepr<'a>(&'a monty_types::MontyTimeZone);

impl fmt::Display for TimeZoneRepr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", monty_types::MontyObject::timezone(self.0.clone()).py_repr())
    }
}

/// Python's `str()` of the float values JSON has no representation for.
pub(crate) fn nonfinite_str(f: f64) -> &'static str {
    if f.is_nan() {
        "nan"
    } else if f > 0.0 {
        "inf"
    } else {
        "-inf"
    }
}

/// Python `datetime.isoformat()`: microseconds only when non-zero, and the
/// UTC offset (when aware) as `±HH:MM`, extended with `:SS` when needed.
fn datetime_isoformat(dt: &MontyDateTime) -> String {
    let mut iso = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    );
    if dt.microsecond != 0 {
        let _ = write!(iso, ".{:06}", dt.microsecond);
    }
    if let Some(offset) = dt.offset_seconds {
        write_utc_offset(&mut iso, offset);
    }
    iso
}

/// A `time` as its `isoformat()`, so it lands in telemetry as a string like
/// `date`/`datetime` rather than falling through to `repr()`. `fold` is not
/// representable in ISO 8601 and is dropped, as CPython's `isoformat()` does.
fn time_isoformat(t: &MontyTime) -> String {
    let mut iso = format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second);
    if t.microsecond != 0 {
        let _ = write!(iso, ".{:06}", t.microsecond);
    }
    if let Some(offset) = t.offset_seconds {
        write_utc_offset(&mut iso, offset);
    }
    iso
}

/// Appends a `±HH:MM[:SS]` UTC offset, the suffix shared by the aware `date`,
/// `datetime` and `time` renderings.
fn write_utc_offset(iso: &mut String, offset: i32) {
    let sign = if offset < 0 { '-' } else { '+' };
    let abs = offset.unsigned_abs();
    let _ = write!(iso, "{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60);
    if !abs.is_multiple_of(60) {
        let _ = write!(iso, ":{:02}", abs % 60);
    }
}

// tests live here because the encoders are crate-private: telemetry encoding
// is not part of the pool's public API
#[cfg(test)]
mod tests {
    use monty_types::{
        ExcType, MontyDate, MontyDateTime, MontyGraph, MontyNode, MontyObject, MontyTimeDelta, MontyType, MontyUuid,
        NodeId,
    };

    use super::{serialize_capped, serialize_dict_capped, serialize_named_capped, serialize_seq_capped};

    /// Encodes a value with a byte cap.
    fn capped(value: &MontyObject, limit: usize) -> (String, bool) {
        serialize_capped(value.graph.nodes(), value.root, limit)
    }

    /// Shorthand: encode with a byte cap nothing here reaches.
    fn json(obj: &MontyObject) -> String {
        let (json, cut) = capped(obj, usize::MAX);
        assert!(!cut);
        json
    }

    #[test]
    fn scalars_encode_as_json_scalars() {
        assert_eq!(json(&MontyObject::bool(true)), "true");
        assert_eq!(json(&MontyObject::int(-42)), "-42");
        assert_eq!(json(&MontyObject::float(1.5)), "1.5");
        assert_eq!(json(&MontyObject::string("hello".to_owned())), r#""hello""#);
        assert_eq!(json(&MontyObject::none()), "null");
    }

    #[test]
    fn nonfinite_floats_become_python_str() {
        assert_eq!(json(&MontyObject::float(f64::INFINITY)), r#""inf""#);
        assert_eq!(json(&MontyObject::float(f64::NEG_INFINITY)), r#""-inf""#);
        assert_eq!(json(&MontyObject::float(f64::NAN)), r#""nan""#);
    }

    #[test]
    fn containers_become_json() {
        let list = MontyObject::list([
            MontyObject::int(1),
            MontyObject::string("x".to_owned()),
            MontyObject::none(),
            MontyObject::float(f64::NAN),
        ]);
        assert_eq!(json(&list), r#"[1,"x",null,"nan"]"#);

        let tuple = MontyObject::tuple([MontyObject::bool(false), MontyObject::float(2.5)]);
        assert_eq!(json(&tuple), "[false,2.5]");

        let nested = MontyObject::list([MontyObject::list([MontyObject::int(1)])]);
        assert_eq!(json(&nested), "[[1]]");
    }

    #[test]
    fn dict_keys_are_strings_or_reprs() {
        let dict = MontyObject::dict([
            (MontyObject::string("a".to_owned()), MontyObject::int(1)),
            (MontyObject::int(2), MontyObject::string("b".to_owned())),
        ]);
        assert_eq!(json(&dict), r#"{"a":1,"2":"b"}"#);
    }

    /// Repr-based dictionary keys stream through the cap, including bytes
    /// nested in a container key whose ordinary repr would be much larger.
    #[test]
    fn oversize_dict_key_repr_is_capped() {
        let pairs = vec![(
            MontyObject::tuple([MontyObject::bytes(vec![0xff; 10_000])]),
            MontyObject::none(),
        )];
        let value = MontyObject::dict(pairs);
        let MontyNode::Dict(pairs) = value.root_node() else {
            panic!("expected a dict node");
        };
        let (json, cut) = serialize_dict_capped(value.graph.nodes(), pairs, 64);
        assert!(cut);
        assert!(json.len() <= 64);
    }

    #[test]
    fn bytes_use_repr_content() {
        let bytes = MontyObject::bytes(b"hi\xff".to_vec());
        assert_eq!(json(&bytes), r#""hi\\xff""#);
    }

    #[test]
    fn dates_use_isoformat() {
        let date = MontyObject::date(MontyDate {
            year: 2024,
            month: 3,
            day: 7,
        });
        assert_eq!(json(&date), r#""2024-03-07""#);

        let naive = MontyObject::datetime(MontyDateTime {
            year: 2024,
            month: 3,
            day: 7,
            hour: 1,
            minute: 2,
            second: 3,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
        });
        assert_eq!(json(&naive), r#""2024-03-07T01:02:03""#);

        let aware = MontyObject::datetime(MontyDateTime {
            year: 2024,
            month: 3,
            day: 7,
            hour: 1,
            minute: 2,
            second: 3,
            microsecond: 450,
            offset_seconds: Some(-5 * 3600),
            timezone_name: None,
        });
        assert_eq!(json(&aware), r#""2024-03-07T01:02:03.000450-05:00""#);
    }

    #[test]
    fn timedelta_is_total_seconds() {
        let delta = MontyObject::timedelta(MontyTimeDelta {
            days: 1,
            seconds: 1,
            microseconds: 500_000,
        });
        assert_eq!(json(&delta), "86401.5");
    }

    /// The extreme end of Python's `timedelta` range overflows an i64 of
    /// microseconds, so the total is accumulated in f64 instead.
    #[test]
    fn huge_timedelta_does_not_overflow() {
        let delta = MontyObject::timedelta(MontyTimeDelta {
            days: i32::MAX,
            seconds: 86_399,
            microseconds: 999_999,
        });
        assert_eq!(json(&delta), "185542587187200.0");
    }

    #[test]
    fn exception_is_its_message() {
        let exc = MontyObject::exception(ExcType::ValueError, Some("boom".to_owned()));
        assert_eq!(json(&exc), r#""boom""#);
    }

    #[test]
    fn namedtuple_is_a_plain_array() {
        let nt = MontyObject::named_tuple(
            "Point".to_owned(),
            vec!["x".to_owned(), "y".to_owned()],
            vec![MontyObject::int(1), MontyObject::int(2)],
        );
        assert_eq!(json(&nt), "[1,2]");
    }

    /// A minimal host class type for fixtures.
    fn test_class_type(
        name: &str,
        is_dataclass: bool,
        attrs: impl IntoIterator<Item = (MontyObject, MontyObject)>,
    ) -> MontyObject {
        MontyObject::class_type(name, MontyUuid::from_u128(1), true, is_dataclass, attrs)
    }

    /// A class instance mirrors its variant: the class, the instance id and
    /// the eager attrs.
    #[test]
    fn class_instance_mirrors_its_shape() {
        let ci = MontyObject::class_instance(
            test_class_type("Point", true, []),
            MontyUuid::from_u128(7),
            [
                (MontyObject::string("x".to_owned()), MontyObject::int(1)),
                (MontyObject::string("y".to_owned()), MontyObject::int(2)),
            ],
        );
        assert_eq!(
            json(&ci),
            r#"{"type":{"name":"Point","id":"00000000-0000-0000-0000-000000000001","host_defined":true,"is_dataclass":true,"attrs":{}},"id":"00000000-0000-0000-0000-000000000007","attrs":{"x":1,"y":2}}"#
        );
    }

    /// A class object mirrors its node; builtin types keep their repr.
    #[test]
    fn class_type_mirrors_its_fields() {
        let class_type = test_class_type(
            "Point",
            false,
            [(
                MontyObject::string("ORIGIN".to_owned()),
                MontyObject::tuple([MontyObject::int(0), MontyObject::int(0)]),
            )],
        );
        assert_eq!(
            json(&class_type),
            r#"{"name":"Point","id":"00000000-0000-0000-0000-000000000001","host_defined":true,"is_dataclass":false,"attrs":{"ORIGIN":[0,0]}}"#
        );
        assert_eq!(json(&MontyObject::type_object(MontyType::Int)), r#""<class 'int'>""#);
    }

    /// A wide attacker-controlled attrs mapping is cut by the byte cap rather
    /// than serialized in full.
    #[test]
    fn class_instance_attrs_are_capped() {
        let attrs = (0..2_000)
            .map(|index| (MontyObject::string(format!("extra_{index}")), MontyObject::none()))
            .collect::<Vec<_>>();
        let value = MontyObject::class_instance(test_class_type("Large", false, []), MontyUuid::from_u128(7), attrs);
        assert!(capped(&value, 64).1);
    }

    #[test]
    fn opaque_values_fall_back_to_repr() {
        assert_eq!(json(&MontyObject::ellipsis()), r#""Ellipsis""#);
        assert_eq!(json(&MontyObject::path("/mnt/data".to_owned())), r#""/mnt/data""#);
        assert_eq!(
            json(&MontyObject::cycle("[...]".to_owned())),
            r#""<circular reference>""#
        );
    }

    #[test]
    fn big_ints_encode_as_numbers() {
        let big = MontyObject::bigint("123456789012345678901234567890".parse().unwrap());
        assert_eq!(json(&big), "123456789012345678901234567890");
    }

    #[test]
    fn oversize_output_is_cut_off_and_flagged() {
        let value = MontyObject::list((0..1000).map(MontyObject::int));
        let (s, cut) = capped(&value, 20);
        assert!(cut);
        assert!(s.len() <= 20);
        assert!(s.starts_with("[0,1,"));
    }

    /// A non-string key that the cap cuts marks the whole encoding as cut,
    /// even when the part of it that was rendered fits the outer map.
    #[test]
    fn oversize_non_string_key_is_flagged() {
        let key = MontyObject::tuple([MontyObject::string("k".repeat(100))]);
        let value = MontyObject::dict([(key, MontyObject::int(1))]);
        let (s, cut) = capped(&value, 32);
        assert!(cut);
        assert!(s.len() <= 32);
    }

    /// A `bytes` leaf is escaped only as far as the byte cap can keep, so a
    /// huge payload is never quadrupled into a repr that is then thrown away.
    #[test]
    fn oversize_bytes_are_capped_before_escaping() {
        let value = MontyObject::list([MontyObject::bytes(vec![0xff; 10_000]), MontyObject::int(1)]);
        let (s, cut) = capped(&value, 64);
        assert!(cut);
        assert!(s.len() <= 64);
        // a payload the cap can hold is still encoded in full
        let value = MontyObject::bytes(vec![0xff; 4]);
        assert_eq!(json(&value), r#""\\xff\\xff\\xff\\xff""#);
    }

    /// The multi-root encoders (used so telemetry never copies a feed's
    /// inputs or a call's arguments out of their arena) match the
    /// single-root one.
    #[test]
    fn multi_root_values_encode_like_single_roots() {
        let mut graph = MontyGraph::new();
        let one = graph.push(MontyNode::Int(1));
        let x = graph.push(MontyNode::String("x".to_owned()));
        let a = graph.push(MontyNode::String("a".to_owned()));
        let two = graph.push(MontyNode::Int(2));
        let none = graph.push(MontyNode::None);
        let flag = graph.push(MontyNode::Bool(true));
        let list = graph.push(MontyNode::List(vec![flag]));
        let nodes = graph.nodes();
        assert_eq!(
            serialize_seq_capped(nodes, &[one, x], usize::MAX),
            (r#"[1,"x"]"#.to_owned(), false)
        );
        assert_eq!(
            serialize_dict_capped(nodes, &[(a, one), (two, none)], usize::MAX),
            (r#"{"a":1,"2":null}"#.to_owned(), false)
        );
        assert_eq!(
            serialize_named_capped(nodes, &[("x", one), ("y", list)], usize::MAX),
            (r#"{"x":1,"y":[true]}"#.to_owned(), false)
        );
    }

    /// An arena the pool has not validated cannot make the encoder loop or
    /// index out of range: a child at or above its holder, or past the end,
    /// renders as a placeholder.
    #[test]
    fn hostile_arenas_render_placeholders() {
        let nodes = vec![
            MontyNode::List(vec![NodeId(0), NodeId(7)]),
            MontyNode::List(vec![NodeId(0)]),
        ];
        assert_eq!(
            serialize_capped(&nodes, NodeId(1), usize::MAX),
            (r#"[["<invalid>","<invalid>"]]"#.to_owned(), false)
        );
        assert_eq!(
            serialize_capped(&nodes, NodeId(9), usize::MAX),
            (r#""<invalid>""#.to_owned(), false)
        );
    }
}
