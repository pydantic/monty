//! Text and byte strings used as regex sources and captures.

use std::fmt::{self, Write};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};

use crate::{
    heap::Heap,
    types::{
        bytes::{allocate_bytes, bytes_repr_fmt},
        str::{allocate_string, string_repr_fmt},
    },
    value::Value,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ReText<S = String, B = Vec<u8>> {
    Str(S),
    Bytes(B),
}

pub(crate) type ReInput<'a> = ReText<&'a str, &'a [u8]>;

impl ReText {
    pub(crate) fn as_input(&self) -> ReInput<'_> {
        match self {
            Self::Str(s) => ReInput::Str(s),
            Self::Bytes(b) => ReInput::Bytes(b),
        }
    }

    pub(crate) fn is_bytes(&self) -> bool {
        matches!(self, Self::Bytes(_))
    }

    pub(crate) fn to_value(&self, heap: &Heap) -> Value {
        match self {
            Self::Str(s) => allocate_string(s.as_str(), heap),
            Self::Bytes(b) => allocate_bytes(b.clone(), heap),
        }
    }

    pub(crate) fn repr_fmt(&self, f: &mut impl Write) -> fmt::Result {
        match self {
            Self::Str(s) => string_repr_fmt(s, f),
            Self::Bytes(b) => bytes_repr_fmt(b, f),
        }
    }
}

impl ReInput<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Str(s) => s.len(),
            Self::Bytes(b) => b.len(),
        }
    }

    /// Copies a capture using the engine's byte offsets into the subject.
    pub(crate) fn capture(&self, start: usize, end: usize) -> ReText {
        match self {
            Self::Str(s) => ReText::Str(s[start..end].to_owned()),
            Self::Bytes(b) => ReText::Bytes(b[start..end].to_vec()),
        }
    }
}

impl Serialize for ReText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Str(s) => serializer.serialize_str(s),
            Self::Bytes(b) => serializer.serialize_bytes(b),
        }
    }
}

impl<'de> Deserialize<'de> for ReText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // CBOR distinguishes text strings from byte strings even when the bytes are valid UTF-8.
        // Text strings keep the representation used by dumps before bytes regexes.
        struct ReTextVisitor;

        impl Visitor<'_> for ReTextVisitor {
            type Value = ReText;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a text string or byte string")
            }

            fn visit_str<E>(self, value: &str) -> Result<ReText, E> {
                Ok(ReText::Str(value.to_owned()))
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<ReText, E> {
                Ok(ReText::Bytes(value.to_vec()))
            }
        }

        deserializer.deserialize_any(ReTextVisitor)
    }
}
