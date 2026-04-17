//! Natural-form JSON serialization for [`MontyObject`].
//!
//! The derive on `MontyObject` produces an externally tagged format that is
//! lossless and round-trippable but verbose (e.g. `{"Int":42}`). That format
//! is appropriate for binary serde and for snapshot round-trips, but it is
//! awkward when `MontyObject` is merely being handed to an external system
//! as JSON output.
//!
//! [`JsonMontyObject`] is a serialize-only wrapper that produces the natural
//! mapping documented on `MontyObject` itself:
//!
//! - JSON-native Python values serialize bare (`Int` → `42`, `String` → `"hi"`,
//!   `List` → `[...]`, `None` → `null`, `Dict` → JSON object).
//! - Non-JSON-native values are wrapped in a single-key object with a
//!   `$`-prefixed discriminator (e.g. `Tuple` → `{"$tuple":[...]}`,
//!   `Bytes` → `{"$bytes":[...]}`, `Exception` → `{"$exception":{...}}`).
//! - `...` (Ellipsis) serializes as `{"$ellipsis": "..."}` so it's
//!   unambiguously distinguishable from a plain string `"..."` while
//!   staying consistent with the other `$`-tagged non-JSON-native shapes.
//! - Non-finite floats (`nan`, `inf`, `-inf`) serialize as
//!   `{"$float": "nan" | "inf" | "-inf"}` because plain JSON has no
//!   representation for them and `serde_json` would otherwise emit `null`,
//!   collapsing them with `None`.
//! - Dataclasses and namedtuples are emitted as two-key objects carrying
//!   both the instance's attribute/field data and its class name:
//!   `{"$dataclass": {"x": 1, "y": 2}, "name": "Point"}`.
//! - Dates and timezones serialize as structured objects so fields are
//!   accessible to consumers without parsing ISO strings.
//! - Dicts whose keys are all Python strings serialize as a normal JSON
//!   object. Any dict with a non-string key falls back to a tagged
//!   `{"$dict": [[k, v], ...]}` form so the real key type is preserved —
//!   e.g. `{1: "a", (1, 2): "b"}` → `{"$dict": [[1, "a"], [{"$tuple":[1,2]}, "b"]]}`.
//!
//! This format is intentionally **output-only**: several variants (`Tuple`,
//! `Bytes`, `Set`, dataclass instances, ...) cannot be unambiguously recovered
//! from their natural JSON shape. For round-trip serialization use
//! `serde_json::to_string(&monty_object)` with the derived format instead.

use std::fmt::Display;

use serde::{
    Serialize, Serializer,
    ser::{Error as _, SerializeMap, SerializeSeq},
};

use crate::{
    object::{DictPairs, MontyObject},
    types::Type,
};

/// Serialize-only wrapper around [`MontyObject`] that produces natural JSON.
///
/// See the module docs for the full mapping. Use with `serde_json`:
///
/// ```
/// use monty::{JsonMontyObject, MontyObject};
///
/// let obj = MontyObject::List(vec![MontyObject::Int(1), MontyObject::String("x".into())]);
/// let json = serde_json::to_string(&JsonMontyObject(&obj)).unwrap();
/// assert_eq!(json, r#"[1,"x"]"#);
/// ```
pub struct JsonMontyObject<'a>(pub &'a MontyObject);

impl Serialize for JsonMontyObject<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            MontyObject::None => serializer.serialize_unit(),
            MontyObject::Bool(b) => serializer.serialize_bool(*b),
            MontyObject::Int(i) => serializer.serialize_i64(*i),
            MontyObject::BigInt(bi) => {
                // Emit as a raw JSON number regardless of magnitude. For values that
                // fit in i64/u64 the fast path hands the integer directly to the
                // serializer; anything larger goes via `serde_json::Number`, which —
                // with the crate's `arbitrary_precision` feature — round-trips
                // arbitrary-size integers as bare JSON numbers.
                //
                // Note: JSON consumers that parse numbers as f64 (e.g. the default
                // JavaScript `JSON.parse`) will silently truncate values beyond 2^53.
                // That's a consumer-side concern; the JSON document itself is valid.
                if let Ok(i) = i64::try_from(bi) {
                    serializer.serialize_i64(i)
                } else if let Ok(u) = u64::try_from(bi) {
                    serializer.serialize_u64(u)
                } else {
                    let n: serde_json::Number = bi
                        .to_string()
                        .parse()
                        .map_err(|e| S::Error::custom(format!("failed to encode bigint: {e}")))?;
                    n.serialize(serializer)
                }
            }
            MontyObject::Float(f) => {
                // `serde_json` emits non-finite f64s as JSON `null`, which
                // would make `nan` / `inf` indistinguishable from `None`.
                // Wrap them in a `$float` tag with `"nan"` / `"inf"` /
                // `"-inf"` so the real value survives round-trip.
                if f.is_finite() {
                    serializer.serialize_f64(*f)
                } else {
                    let s = if f.is_nan() {
                        "nan"
                    } else if *f > 0.0 {
                        "inf"
                    } else {
                        "-inf"
                    };
                    serialize_tagged(serializer, "$float", &s)
                }
            }
            MontyObject::String(s) => serializer.serialize_str(s),
            MontyObject::List(items) => serialize_seq(serializer, items),
            MontyObject::Dict(pairs) => serialize_dict(serializer, pairs),
            // Date/time types already derive Serialize with the documented
            // natural field layout (year/month/day, ...), so forward to the derive.
            MontyObject::Date(d) => d.serialize(serializer),
            MontyObject::DateTime(dt) => dt.serialize(serializer),
            MontyObject::TimeDelta(td) => td.serialize(serializer),
            MontyObject::TimeZone(tz) => tz.serialize(serializer),
            MontyObject::Ellipsis => serialize_tagged(serializer, "$ellipsis", &"..."),
            MontyObject::Tuple(items) => serialize_tagged_seq(serializer, "$tuple", items),
            MontyObject::Set(items) => serialize_tagged_seq(serializer, "$set", items),
            MontyObject::FrozenSet(items) => serialize_tagged_seq(serializer, "$frozenset", items),
            MontyObject::Bytes(bytes) => serialize_tagged(serializer, "$bytes", bytes),
            MontyObject::NamedTuple {
                type_name,
                field_names,
                values,
            } => serialize_named(
                serializer,
                "$namedtuple",
                &FieldsBody { field_names, values },
                type_name,
            ),
            MontyObject::Exception { exc_type, arg } => {
                let type_str: &'static str = exc_type.into();
                serialize_tagged(
                    serializer,
                    "$exception",
                    &ExceptionBody {
                        exc_type: type_str,
                        arg: arg.as_deref(),
                    },
                )
            }
            MontyObject::Path(p) => serialize_tagged(serializer, "$path", p),
            MontyObject::Dataclass { name, attrs, .. } => {
                serialize_named(serializer, "$dataclass", &AttrsBody(attrs), name)
            }
            MontyObject::Type(t) => serialize_tagged(serializer, "$type", &TypeName(*t)),
            MontyObject::BuiltinFunction(f) => serialize_tagged(serializer, "$builtin", &DisplayAsStr(f)),
            MontyObject::Function { name, .. } => serialize_tagged(serializer, "$function", name),
            MontyObject::Repr(s) => serialize_tagged(serializer, "$repr", s),
            MontyObject::Cycle(_, placeholder) => serialize_tagged(serializer, "$cycle", placeholder),
        }
    }
}

/// Serialize a slice of `MontyObject`s as a JSON array, recursively wrapping
/// each element in `JsonMontyObject` so natural-form rules propagate.
fn serialize_seq<S: Serializer>(serializer: S, items: &[MontyObject]) -> Result<S::Ok, S::Error> {
    let mut seq = serializer.serialize_seq(Some(items.len()))?;
    for item in items {
        seq.serialize_element(&JsonMontyObject(item))?;
    }
    seq.end()
}

/// Emit a single-key object `{"<tag>": <body>}`. Used for every non-JSON-native
/// variant; centralizes the tagging convention so it stays consistent.
fn serialize_tagged<S: Serializer>(serializer: S, tag: &'static str, body: &impl Serialize) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry(tag, body)?;
    map.end()
}

/// Emit `{"<tag>": [items...]}` where each item is wrapped in `JsonMontyObject`.
fn serialize_tagged_seq<S: Serializer>(
    serializer: S,
    tag: &'static str,
    items: &[MontyObject],
) -> Result<S::Ok, S::Error> {
    serialize_tagged(serializer, tag, &JsonMontyArray(items))
}

/// Serialize a `Dict`. If every key is a Python string, emit a normal JSON
/// object (the common case). Otherwise fall back to a tagged
/// `{"$dict": [[k, v], ...]}` form so non-string keys preserve their real
/// type — a bare JSON object can only use string keys, so stringifying
/// them would be lossy.
fn serialize_dict<S: Serializer>(serializer: S, pairs: &DictPairs) -> Result<S::Ok, S::Error> {
    // The output shape depends on whether every key is a string, so we
    // pre-scan before committing to a format — opening `serialize_map` and
    // then discovering a non-string key midway would leave a half-built
    // JSON object that can't be recovered into the `$dict` shape. The
    // scan is a pointer-chase over the enum discriminant only; the heavy
    // per-value serialization happens in the subsequent pass. For dicts
    // with all-string keys (the common case) this is effectively O(n)
    // work plus an O(n) discriminant scan.
    let is_all_strings = pairs.into_iter().all(|(k, _)| matches!(k, MontyObject::String(_)));
    if is_all_strings {
        let mut map = serializer.serialize_map(Some(pairs.len()))?;
        for (k, v) in pairs {
            let MontyObject::String(key) = k else { unreachable!() };
            map.serialize_entry(key, &JsonMontyObject(v))?;
        }
        map.end()
    } else {
        serialize_tagged(serializer, "$dict", &DictPairsBody(pairs))
    }
}

/// `DictPairs`-equivalent of `PairsArrayBody`; mirrors the body shape used
/// by `JsonMontyPairs` but borrows from the opaque `DictPairs` newtype
/// rather than a raw slice.
struct DictPairsBody<'a>(&'a DictPairs);

impl Serialize for DictPairsBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for (k, v) in self.0 {
            seq.serialize_element(&[JsonMontyObject(k), JsonMontyObject(v)])?;
        }
        seq.end()
    }
}

/// Emit a two-key object `{"<tag>": <body>, "name": "<name>"}`. Used by
/// dataclass and namedtuple to pair the instance's data with its class name
/// at the same nesting level rather than burying `name` inside the body.
fn serialize_named<S: Serializer>(
    serializer: S,
    tag: &'static str,
    body: &impl Serialize,
    name: &str,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(2))?;
    map.serialize_entry(tag, body)?;
    map.serialize_entry("name", name)?;
    map.end()
}

/// Serialize-only wrapper around a slice of [`MontyObject`]s that produces a
/// JSON array in the natural form (e.g. `[1, "two", {"$tuple": [3, 4]}]`).
///
/// Use this when you have a raw slice and don't want to transiently
/// construct a `MontyObject::List` just to borrow it: it's equivalent to
/// wrapping the slice in a list, but avoids the clone.
pub struct JsonMontyArray<'a>(pub &'a [MontyObject]);

impl Serialize for JsonMontyArray<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_seq(serializer, self.0)
    }
}

/// Serialize-only wrapper around a slice of `(key, value)` pairs that
/// produces the same shape as a `MontyObject::Dict`: a plain JSON object
/// when every key is a Python string, else a `{"$dict": [[k, v], ...]}`
/// fallback that preserves key types.
///
/// Intended for pair-list structures stored as `Vec<(MontyObject, MontyObject)>`
/// (e.g. the `kwargs` on a function snapshot) so they can be serialized
/// without constructing a `DictPairs` or a full `MontyObject::Dict`.
pub struct JsonMontyPairs<'a>(pub &'a [(MontyObject, MontyObject)]);

impl Serialize for JsonMontyPairs<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // See `serialize_dict` for why the pre-scan is needed: the choice
        // between `{"k": v, ...}` and `{"$dict": [[k, v], ...]}` must be
        // made before opening the outer map/object.
        if self.0.iter().all(|(k, _)| matches!(k, MontyObject::String(_))) {
            let mut map = serializer.serialize_map(Some(self.0.len()))?;
            for (k, v) in self.0 {
                let MontyObject::String(key) = k else { unreachable!() };
                map.serialize_entry(key, &JsonMontyObject(v))?;
            }
            map.end()
        } else {
            serialize_tagged(serializer, "$dict", &PairsArrayBody(self.0))
        }
    }
}

/// Body of a `$dict` tag — each pair becomes a two-element JSON array
/// `[key, value]` with both sides recursively serialized via
/// `JsonMontyObject`, so non-string keys retain their real type.
struct PairsArrayBody<'a>(&'a [(MontyObject, MontyObject)]);

impl Serialize for PairsArrayBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for (k, v) in self.0 {
            seq.serialize_element(&[JsonMontyObject(k), JsonMontyObject(v)])?;
        }
        seq.end()
    }
}

/// Serializes a namedtuple's body as a JSON object mapping each field name
/// to the corresponding value (e.g. `{"x": 1, "y": 2}`). Field names are
/// always valid Python identifiers so no escaping/repr is needed.
///
/// A well-formed `MontyObject::NamedTuple` invariantly has
/// `field_names.len() == values.len()`. A mismatch indicates a bug upstream
/// (handwritten construction, corrupted deserialization, ...) — rather
/// than silently truncating one side with `zip`, we surface the mismatch
/// as a serde error so the problem is loud.
struct FieldsBody<'a> {
    field_names: &'a [String],
    values: &'a [MontyObject],
}

impl Serialize for FieldsBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.field_names.len() != self.values.len() {
            return Err(S::Error::custom(format!(
                "namedtuple field/value length mismatch: {} field names vs {} values",
                self.field_names.len(),
                self.values.len(),
            )));
        }
        let mut map = serializer.serialize_map(Some(self.values.len()))?;
        for (name, value) in self.field_names.iter().zip(self.values) {
            map.serialize_entry(name, &JsonMontyObject(value))?;
        }
        map.end()
    }
}

/// Serializes a dataclass's `attrs` as a JSON object. Attr keys are always
/// interned strings (Python attribute names), so any non-string key here is
/// a bug elsewhere in the pipeline and surfaces as a serde error.
struct AttrsBody<'a>(&'a DictPairs);

impl Serialize for AttrsBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            let MontyObject::String(key) = k else {
                return Err(S::Error::custom("dataclass attrs must have string keys"));
            };
            map.serialize_entry(key, &JsonMontyObject(v))?;
        }
        map.end()
    }
}

/// Body of an `$exception` tag — omits `arg` when `None` for a tighter shape.
#[derive(Serialize)]
struct ExceptionBody<'a> {
    #[serde(rename = "type")]
    exc_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    arg: Option<&'a str>,
}

/// Wraps a `Type` so it serializes as its `Display` string (e.g. `"int"`).
struct TypeName(Type);

impl Serialize for TypeName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

/// Adapter that serializes any `Display` value as its string form without
/// allocating a `String` first (`collect_str` streams via `fmt::Write`).
struct DisplayAsStr<T>(T);

impl<T: Display> Serialize for DisplayAsStr<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}
