//! Logfire-style JSON encoding of [`MontyObject`]s for telemetry attributes.
//!
//! Mirrors the Python logfire JSON encoder (`logfire/_internal/json_encoder.py`):
//! containers become JSON arrays/objects, dates use isoformat, timedeltas their
//! total seconds, and opaque objects fall back to their `repr`. Output is capped
//! at a byte limit so a huge value cannot blow up the telemetry pipeline.
//!
//! Divergences from the Python encoder: sets are encoded in storage order
//! (Python sorts them when comparable), integers beyond `i128` become their
//! digit string rather than a raw JSON number, and class instances and class
//! objects mirror their `MontyObject` shape (`{type, id, attrs}` and the
//! `MontyClassType` fields) so telemetry records *which* object crossed the
//! boundary, not just its attribute snapshot.

use std::{
    fmt::{self, Write as _},
    io::{self, Write},
};

use monty_types::{
    DictPairs, MontyClassInstance, MontyClassType, MontyDateTime, MontyObject, MontyTime, MontyType, bytes_repr,
};
use num_traits::ToPrimitive;
use serde::ser::{Serialize, SerializeMap, Serializer};

/// Serializes `value` to logfire-style JSON (see the module docs), capped at
/// `limit` bytes. The bool is true when the cap cut serialization short — the
/// partial output is then no longer valid JSON, so the caller should mark it
/// truncated.
pub(crate) fn serialize_capped(value: &MontyObject, limit: usize) -> (String, bool) {
    capped(&JsonEncoded { value, limit }, limit)
}

/// [`serialize_capped`] for a borrowed list; this and the two variants below
/// exist to avoid cloning values into an owned [`MontyObject`] container.
pub(crate) fn serialize_seq_capped(items: &[MontyObject], limit: usize) -> (String, bool) {
    capped(&JsonSeq { items, limit }, limit)
}

/// [`serialize_capped`] for borrowed dict pairs; non-string keys render as
/// their repr.
pub(crate) fn serialize_dict_capped(pairs: &[(MontyObject, MontyObject)], limit: usize) -> (String, bool) {
    capped(&JsonDict { pairs, limit }, limit)
}

/// [`serialize_capped`] for borrowed name → value pairs, as a JSON object.
#[cfg(test)]
pub(crate) fn serialize_named_capped(pairs: &[(&str, &MontyObject)], limit: usize) -> (String, bool) {
    serialize_named_iter_capped(pairs.iter().copied(), pairs.len(), limit)
}

/// [`serialize_capped`] for an iterator of borrowed name → value pairs.
pub(crate) fn serialize_named_iter_capped<'a>(
    pairs: impl Clone + Iterator<Item = (&'a str, &'a MontyObject)>,
    len: usize,
    limit: usize,
) -> (String, bool) {
    capped(&JsonNamed { pairs, len, limit }, limit)
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

/// Serializes a [`MontyObject`] with the logfire value mapping (rather than
/// the tagged-enum encoding of the derived `Serialize`, which is for the wire).
///
/// `limit` is carried down the value tree so a huge `bytes` leaf is escaped
/// only as far as the cap can keep — the writer alone cannot help, since it
/// sees the escaped string only once it is built.
struct JsonEncoded<'a> {
    value: &'a MontyObject,
    limit: usize,
}

impl<'a> JsonEncoded<'a> {
    /// An encoder for a child value, carrying `limit` down the tree.
    const fn nested(&self, value: &'a MontyObject) -> Self {
        Self {
            value,
            limit: self.limit,
        }
    }
}

impl Serialize for JsonEncoded<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.value {
            MontyObject::None => s.serialize_unit(),
            MontyObject::Bool(b) => s.serialize_bool(*b),
            MontyObject::Int(i) => s.serialize_i64(*i),
            // beyond i128 the digits become a string: a raw JSON number would
            // need serde_json's arbitrary-precision mode
            MontyObject::BigInt(b) => match b.to_i128() {
                Some(i) => s.serialize_i128(i),
                None => s.collect_str(b),
            },
            MontyObject::Float(f) if f.is_finite() => s.serialize_f64(*f),
            MontyObject::Float(f) => s.serialize_str(nonfinite_str(*f)),
            MontyObject::String(v) => s.serialize_str(v),
            // like logfire: the repr's escaped content without the b'' wrapper.
            // Escaping stops at `limit` input bytes — each escapes to at least
            // one character, so the rest would only inflate a huge payload to
            // produce output the cap discards
            MontyObject::Bytes(b) => {
                let repr = bytes_repr(&b[..b.len().min(self.limit)]);
                s.serialize_str(&repr[2..repr.len() - 1])
            }
            MontyObject::List(items)
            | MontyObject::Tuple(items)
            | MontyObject::Set(items)
            | MontyObject::FrozenSet(items) => s.collect_seq(items.iter().map(|v| self.nested(v))),
            // a namedtuple is a tuple in Python, so logfire encodes the values
            // as an array and drops the field names
            MontyObject::NamedTuple { values, .. } => s.collect_seq(values.iter().map(|v| self.nested(v))),
            MontyObject::Dict(pairs) => {
                serialize_pairs(pairs.into_iter().map(|(k, v)| (k, v)), pairs.len(), self.limit, s)
            }
            // mirrors the variant: the class, the instance id, then the eager
            // attrs in order (there are no declared field names — attrs ARE
            // the surface)
            MontyObject::ClassInstance(instance) => {
                let MontyClassInstance {
                    class_type,
                    instance_id,
                    attrs,
                } = instance.as_ref();
                let mut map = s.serialize_map(Some(3))?;
                map.serialize_entry("type", &JsonClassType::new(class_type, self.limit))?;
                map.serialize_entry("id", &Displayed(instance_id))?;
                map.serialize_entry(
                    "attrs",
                    &JsonAttrs {
                        attrs,
                        limit: self.limit,
                    },
                )?;
                map.end()
            }
            MontyObject::Type(monty_type) => JsonType {
                monty_type,
                limit: self.limit,
            }
            .serialize(s),
            MontyObject::Date(d) => s.collect_str(&format_args!("{:04}-{:02}-{:02}", d.year, d.month, d.day)),
            MontyObject::DateTime(dt) => s.serialize_str(&datetime_isoformat(dt)),
            MontyObject::Time(t) => s.serialize_str(&time_isoformat(t)),
            // total seconds, accumulated in f64 throughout: an extreme `days`
            // overflows the microseconds of the same sum in i64
            MontyObject::TimeDelta(td) => s.serialize_f64(
                f64::from(td.days).mul_add(86_400.0, f64::from(td.seconds)) + f64::from(td.microseconds) / 1e6,
            ),
            // like logfire: `str(exc)`, which in Python is the message alone
            MontyObject::Exception { arg, .. } => s.serialize_str(arg.as_deref().unwrap_or_default()),
            MontyObject::Path(p) => s.serialize_str(p),
            MontyObject::Cycle(..) => s.serialize_str("<circular reference>"),
            MontyObject::Repr(r) => s.serialize_str(r),
            // `collect_str` streams the repr into the capped JSON writer rather
            // than first materializing an attacker-sized intermediate string.
            other => s.collect_str(other),
        }
    }
}

/// [`JsonEncoded`] for borrowed items with no owning [`MontyObject`]
/// container: a JSON array.
struct JsonSeq<'a> {
    items: &'a [MontyObject],
    limit: usize,
}

impl Serialize for JsonSeq<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.items.iter().map(|value| JsonEncoded {
            value,
            limit: self.limit,
        }))
    }
}

/// [`JsonSeq`] for dict pairs: a JSON object keyed like [`MontyObject::Dict`].
struct JsonDict<'a> {
    pairs: &'a [(MontyObject, MontyObject)],
    limit: usize,
}

impl Serialize for JsonDict<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_pairs(self.pairs.iter().map(|(k, v)| (k, v)), self.pairs.len(), self.limit, s)
    }
}

/// [`JsonDict`] for a value's attrs, which live in a [`DictPairs`].
struct JsonAttrs<'a> {
    attrs: &'a DictPairs,
    limit: usize,
}

impl Serialize for JsonAttrs<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_pairs(self.attrs.iter().map(|(k, v)| (k, v)), self.attrs.len(), self.limit, s)
    }
}

/// A [`MontyType`]: a class mirrors its [`MontyClassType`] fields, a builtin keeps
/// the `<class 'int'>` repr the value would have had.
struct JsonType<'a> {
    monty_type: &'a MontyType,
    limit: usize,
}

impl Serialize for JsonType<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.monty_type {
            MontyType::Instance(class_type) => JsonClassType::new(class_type, self.limit).serialize(s),
            builtin => s.collect_str(&format_args!("<class '{builtin}'>")),
        }
    }
}

/// A [`MontyClassType`] as a JSON object mirroring its fields, `attrs` through
/// the capped dict encoding.
struct JsonClassType<'a> {
    class_type: &'a MontyClassType,
    limit: usize,
}

impl<'a> JsonClassType<'a> {
    /// An encoder for a value's class, carrying `limit` down the tree.
    const fn new(class_type: &'a MontyClassType, limit: usize) -> Self {
        Self { class_type, limit }
    }
}

impl Serialize for JsonClassType<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let ct = self.class_type;
        let mut map = s.serialize_map(Some(5))?;
        map.serialize_entry("name", &ct.name)?;
        map.serialize_entry("id", &Displayed(&ct.id))?;
        map.serialize_entry("host_defined", &ct.host_defined)?;
        map.serialize_entry("is_dataclass", &ct.is_dataclass)?;
        map.serialize_entry(
            "attrs",
            &JsonAttrs {
                attrs: &ct.attrs,
                limit: self.limit,
            },
        )?;
        map.end()
    }
}

/// [`JsonSeq`] for named values: a JSON object of `name` → encoded value.
struct JsonNamed<I> {
    pairs: I,
    len: usize,
    limit: usize,
}

impl<'a, I> Serialize for JsonNamed<I>
where
    I: Clone + Iterator<Item = (&'a str, &'a MontyObject)>,
{
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.len))?;
        for (name, value) in self.pairs.clone() {
            map.serialize_entry(
                name,
                &JsonEncoded {
                    value,
                    limit: self.limit,
                },
            )?;
        }
        map.end()
    }
}

/// Serializes key/value pairs as a JSON object, string keys verbatim and
/// everything else by its Python repr (JSON has no non-string keys).
fn serialize_pairs<'a, S: Serializer>(
    pairs: impl Iterator<Item = (&'a MontyObject, &'a MontyObject)>,
    len: usize,
    limit: usize,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut map = s.serialize_map(Some(len))?;
    for (key, value) in pairs {
        let value = JsonEncoded { value, limit };
        match key {
            MontyObject::String(k) => map.serialize_entry(k, &value)?,
            other => map.serialize_entry(&Displayed(other), &value)?,
        }
    }
    map.end()
}

/// Streams a `Display` rendering (a non-string dictionary key's repr, a uuid)
/// through serde's capped writer rather than materializing it first.
struct Displayed<'a>(&'a dyn fmt::Display);

impl Serialize for Displayed<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self.0)
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
        DictPairs, ExcType, MontyClassInstance, MontyClassType, MontyDate, MontyDateTime, MontyObject, MontyTimeDelta,
        MontyType, MontyUuid,
    };

    use super::{serialize_capped, serialize_dict_capped, serialize_named_capped, serialize_seq_capped};

    /// Shorthand: encode with a byte cap nothing here reaches.
    fn json(obj: &MontyObject) -> String {
        let (json, cut) = serialize_capped(obj, usize::MAX);
        assert!(!cut);
        json
    }

    #[test]
    fn scalars_encode_as_json_scalars() {
        assert_eq!(json(&MontyObject::Bool(true)), "true");
        assert_eq!(json(&MontyObject::Int(-42)), "-42");
        assert_eq!(json(&MontyObject::Float(1.5)), "1.5");
        assert_eq!(json(&MontyObject::String("hello".to_owned())), r#""hello""#);
        assert_eq!(json(&MontyObject::None), "null");
    }

    #[test]
    fn nonfinite_floats_become_python_str() {
        assert_eq!(json(&MontyObject::Float(f64::INFINITY)), r#""inf""#);
        assert_eq!(json(&MontyObject::Float(f64::NEG_INFINITY)), r#""-inf""#);
        assert_eq!(json(&MontyObject::Float(f64::NAN)), r#""nan""#);
    }

    #[test]
    fn containers_become_json() {
        let list = MontyObject::List(vec![
            MontyObject::Int(1),
            MontyObject::String("x".to_owned()),
            MontyObject::None,
            MontyObject::Float(f64::NAN),
        ]);
        assert_eq!(json(&list), r#"[1,"x",null,"nan"]"#);

        let tuple = MontyObject::Tuple(vec![MontyObject::Bool(false), MontyObject::Float(2.5)]);
        assert_eq!(json(&tuple), "[false,2.5]");

        let nested = MontyObject::List(vec![MontyObject::List(vec![MontyObject::Int(1)])]);
        assert_eq!(json(&nested), "[[1]]");
    }

    #[test]
    fn dict_keys_are_strings_or_reprs() {
        let dict = MontyObject::dict(vec![
            (MontyObject::String("a".to_owned()), MontyObject::Int(1)),
            (MontyObject::Int(2), MontyObject::String("b".to_owned())),
        ]);
        assert_eq!(json(&dict), r#"{"a":1,"2":"b"}"#);
    }

    /// Repr-based dictionary keys stream through the cap, including bytes
    /// nested in a container key whose ordinary repr would be much larger.
    #[test]
    fn oversize_dict_key_repr_is_capped() {
        let pairs = vec![(
            MontyObject::Tuple(vec![MontyObject::Bytes(vec![0xff; 10_000])]),
            MontyObject::None,
        )];
        let (json, cut) = serialize_dict_capped(&pairs, 64);
        assert!(cut);
        assert!(json.len() <= 64);
    }

    #[test]
    fn bytes_use_repr_content() {
        let bytes = MontyObject::Bytes(b"hi\xff".to_vec());
        assert_eq!(json(&bytes), r#""hi\\xff""#);
    }

    #[test]
    fn dates_use_isoformat() {
        let date = MontyObject::Date(MontyDate {
            year: 2024,
            month: 3,
            day: 7,
        });
        assert_eq!(json(&date), r#""2024-03-07""#);

        let naive = MontyObject::DateTime(MontyDateTime {
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

        let aware = MontyObject::DateTime(MontyDateTime {
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
        let delta = MontyObject::TimeDelta(MontyTimeDelta {
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
        let delta = MontyObject::TimeDelta(MontyTimeDelta {
            days: i32::MAX,
            seconds: 86_399,
            microseconds: 999_999,
        });
        assert_eq!(json(&delta), "185542587187200.0");
    }

    #[test]
    fn exception_is_its_message() {
        let exc = MontyObject::Exception {
            exc_type: ExcType::ValueError,
            arg: Some("boom".to_owned()),
        };
        assert_eq!(json(&exc), r#""boom""#);
    }

    #[test]
    fn namedtuple_is_a_plain_array() {
        let nt = MontyObject::NamedTuple {
            type_name: "Point".to_owned(),
            field_names: vec!["x".to_owned(), "y".to_owned()],
            values: vec![MontyObject::Int(1), MontyObject::Int(2)],
        };
        assert_eq!(json(&nt), "[1,2]");
    }

    /// A minimal host class type for fixtures.
    fn test_class_type(name: &str, is_dataclass: bool) -> MontyClassType {
        MontyClassType {
            name: name.to_owned(),
            id: MontyUuid::from_u128(1),
            host_defined: true,
            is_dataclass,
            attrs: DictPairs::default(),
        }
    }

    /// A class instance mirrors its variant: the class, the instance id and
    /// the eager attrs.
    #[test]
    fn class_instance_mirrors_its_shape() {
        let ci = MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: test_class_type("Point", true),
            instance_id: MontyUuid::from_u128(7),
            attrs: DictPairs::from(vec![
                (MontyObject::String("x".to_owned()), MontyObject::Int(1)),
                (MontyObject::String("y".to_owned()), MontyObject::Int(2)),
            ]),
        }));
        assert_eq!(
            json(&ci),
            r#"{"type":{"name":"Point","id":"00000000-0000-0000-0000-000000000001","host_defined":true,"is_dataclass":true,"attrs":{}},"id":"00000000-0000-0000-0000-000000000007","attrs":{"x":1,"y":2}}"#
        );
    }

    /// A class object mirrors `MontyClassType`; builtin types keep their repr.
    #[test]
    fn class_type_mirrors_its_fields() {
        let mut class_type = test_class_type("Point", false);
        class_type.attrs = DictPairs::from(vec![(
            MontyObject::String("ORIGIN".to_owned()),
            MontyObject::Tuple(vec![MontyObject::Int(0), MontyObject::Int(0)]),
        )]);
        assert_eq!(
            json(&MontyObject::Type(MontyType::Instance(Box::new(class_type)))),
            r#"{"name":"Point","id":"00000000-0000-0000-0000-000000000001","host_defined":true,"is_dataclass":false,"attrs":{"ORIGIN":[0,0]}}"#
        );
        assert_eq!(json(&MontyObject::Type(MontyType::Int)), r#""<class 'int'>""#);
    }

    /// A wide attacker-controlled attrs mapping is cut by the byte cap rather
    /// than serialized in full.
    #[test]
    fn class_instance_attrs_are_capped() {
        let attrs = (0..2_000)
            .map(|index| (MontyObject::String(format!("extra_{index}")), MontyObject::None))
            .collect::<Vec<_>>();
        let value = MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: test_class_type("Large", false),
            instance_id: MontyUuid::from_u128(7),
            attrs: DictPairs::from(attrs),
        }));
        assert!(serialize_capped(&value, 64).1);
    }

    #[test]
    fn opaque_values_fall_back_to_repr() {
        assert_eq!(json(&MontyObject::Ellipsis), r#""Ellipsis""#);
        assert_eq!(json(&MontyObject::Path("/mnt/data".to_owned())), r#""/mnt/data""#);
        assert_eq!(
            json(&MontyObject::Cycle(0, "[...]".to_owned())),
            r#""<circular reference>""#
        );
    }

    #[test]
    fn big_ints_encode_as_numbers() {
        let big = MontyObject::BigInt("123456789012345678901234567890".parse().unwrap());
        assert_eq!(json(&big), "123456789012345678901234567890");
    }

    #[test]
    fn oversize_output_is_cut_off_and_flagged() {
        let value = MontyObject::List((0..1000).map(MontyObject::Int).collect());
        let (s, cut) = serialize_capped(&value, 20);
        assert!(cut);
        assert!(s.len() <= 20);
        assert!(s.starts_with("[0,1,"));
    }

    /// A `bytes` leaf is escaped only as far as the byte cap can keep, so a
    /// huge payload is never quadrupled into a repr that is then thrown away.
    #[test]
    fn oversize_bytes_are_capped_before_escaping() {
        let value = MontyObject::List(vec![MontyObject::Bytes(vec![0xff; 10_000]), MontyObject::Int(1)]);
        let (s, cut) = serialize_capped(&value, 64);
        assert!(cut);
        assert!(s.len() <= 64);
        // a payload the cap can hold is still encoded in full
        let value = MontyObject::Bytes(vec![0xff; 4]);
        assert_eq!(json(&value), r#""\\xff\\xff\\xff\\xff""#);
    }

    /// The borrowed-value encoders (used so telemetry never deep-clones a
    /// feed's inputs or a call's arguments) match their owned counterparts.
    #[test]
    fn borrowed_values_encode_like_owned_ones() {
        let items = vec![MontyObject::Int(1), MontyObject::String("x".to_owned())];
        assert_eq!(
            serialize_seq_capped(&items, usize::MAX),
            (r#"[1,"x"]"#.to_owned(), false)
        );

        let pairs = vec![
            (MontyObject::String("a".to_owned()), MontyObject::Int(1)),
            (MontyObject::Int(2), MontyObject::None),
        ];
        assert_eq!(
            serialize_dict_capped(&pairs, usize::MAX),
            (r#"{"a":1,"2":null}"#.to_owned(), false)
        );

        let one = MontyObject::Int(1);
        let two = MontyObject::List(vec![MontyObject::Bool(true)]);
        assert_eq!(
            serialize_named_capped(&[("x", &one), ("y", &two)], usize::MAX),
            (r#"{"x":1,"y":[true]}"#.to_owned(), false)
        );
    }
}
