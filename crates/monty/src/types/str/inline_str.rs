//! Fixed-capacity UTF-8 strings shared by values, views and iterators.

use std::{fmt, str};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{bytecode::VM, intern::StringId, value::Value};

/// UTF-8 text of at most 15 bytes.
/// The tag enum supplies niches so a containing `Value` remains 16 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InlineStr {
    tag: InlineTag,
    bytes: [u8; Self::CAPACITY],
}

impl InlineStr {
    pub(crate) const CAPACITY: usize = 15;

    /// Returns `None` when the text exceeds the 15-byte inline capacity.
    pub(crate) fn new(text: &str) -> Option<Self> {
        if text.len() > Self::CAPACITY {
            return None;
        }

        Self::with_ascii(text, text.is_ascii())
    }

    /// Uses ASCII status already established by the producing operation.
    pub(crate) fn with_ascii(text: &str, ascii: bool) -> Option<Self> {
        let tag = InlineTag::from_len(text.len(), ascii)?;
        debug_assert_eq!(ascii, text.is_ascii());
        let mut bytes = [0; Self::CAPACITY];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Some(Self { tag, bytes })
    }

    /// Copies fragments and their known ASCII status into a string value.
    ///
    /// # Panics
    /// Panics if the fragments total more than [`Self::CAPACITY`] bytes.
    pub(super) fn allocate_fragments<'a>(fragments: impl IntoIterator<Item = &'a str>, ascii: bool) -> Value {
        let mut bytes = [0; Self::CAPACITY];
        let mut len = 0;
        for fragment in fragments {
            let end = len + fragment.len();
            bytes[len..end].copy_from_slice(fragment.as_bytes());
            len = end;
        }
        debug_assert_eq!(ascii, bytes[..len].is_ascii());
        match len {
            0 => Value::InternString(StringId::EMPTY),
            1 => Value::InternString(StringId::from_ascii(bytes[0])),
            _ => Value::InlineString(Self {
                tag: InlineTag::from_len(len, ascii).expect("fragments fit inline"),
                bytes,
            }),
        }
    }

    #[inline]
    pub(crate) fn is_ascii(&self) -> bool {
        self.tag.is_ascii()
    }

    pub(crate) fn char_len(&self) -> usize {
        if self.is_ascii() {
            self.tag.byte_len()
        } else {
            self.as_str().chars().count()
        }
    }

    /// Compares without validating UTF-8; non-string operands return `None`.
    pub(crate) fn eq_value(&self, other: &Value, vm: &VM<'_>) -> Option<bool> {
        match other {
            Value::InlineString(other) => Some(self == other),
            _ => other.as_str(vm).map(|s| self.as_bytes() == s.as_bytes()),
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.tag.byte_len()]
    }

    #[expect(unsafe_code)]
    pub(crate) fn as_str(&self) -> &str {
        // SAFETY: Constructors copy whole `&str` fragments. Deserialization calls `new` with decoded text.
        unsafe { str::from_utf8_unchecked(self.as_bytes()) }
    }
}

impl Serialize for InlineStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for InlineStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(InlineStrVisitor)
    }
}

/// Copies decoded text directly into inline storage.
struct InlineStrVisitor;

impl de::Visitor<'_> for InlineStrVisitor {
    type Value = InlineStr;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        InlineStr::new(text).ok_or_else(|| E::custom("inline string exceeds capacity"))
    }

    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
        let text = str::from_utf8(bytes).map_err(|_| E::invalid_value(de::Unexpected::Bytes(bytes), &self))?;
        self.visit_str(text)
    }
}

/// Low four bits store byte length; bit four marks non-ASCII text.
/// Tags 32 and above provide niches for `Value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
enum InlineTag {
    A0,
    A1,
    A2,
    A3,
    A4,
    A5,
    A6,
    A7,
    A8,
    A9,
    A10,
    A11,
    A12,
    A13,
    A14,
    A15,

    // U0 and U1 are impossible: a non-ASCII UTF-8 character needs at least two bytes.
    U2 = 16 + 2,
    U3,
    U4,
    U5,
    U6,
    U7,
    U8,
    U9,
    U10,
    U11,
    U12,
    U13,
    U14,
    U15,
}

impl InlineTag {
    fn from_len(len: usize, ascii: bool) -> Option<Self> {
        Some(match (len, ascii) {
            (0, true) => Self::A0,
            (1, true) => Self::A1,
            (2, true) => Self::A2,
            (3, true) => Self::A3,
            (4, true) => Self::A4,
            (5, true) => Self::A5,
            (6, true) => Self::A6,
            (7, true) => Self::A7,
            (8, true) => Self::A8,
            (9, true) => Self::A9,
            (10, true) => Self::A10,
            (11, true) => Self::A11,
            (12, true) => Self::A12,
            (13, true) => Self::A13,
            (14, true) => Self::A14,
            (15, true) => Self::A15,
            (2, false) => Self::U2,
            (3, false) => Self::U3,
            (4, false) => Self::U4,
            (5, false) => Self::U5,
            (6, false) => Self::U6,
            (7, false) => Self::U7,
            (8, false) => Self::U8,
            (9, false) => Self::U9,
            (10, false) => Self::U10,
            (11, false) => Self::U11,
            (12, false) => Self::U12,
            (13, false) => Self::U13,
            (14, false) => Self::U14,
            (15, false) => Self::U15,
            _ => return None,
        })
    }

    #[inline]
    fn byte_len(self) -> usize {
        (self as usize) & 15
    }

    #[inline]
    fn is_ascii(self) -> bool {
        (self as u8) < 16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{types::StringIterator, value::Value};

    #[test]
    fn capacity_counts_utf8_bytes() {
        assert_eq!(size_of::<InlineStr>(), 16);
        assert_eq!(size_of::<Value>(), 16);
        for text in ["", "a", "é", "abcdefghijklmno", "éééééééa"] {
            assert_eq!(InlineStr::new(text).unwrap().as_str(), text);
        }
        assert!(InlineStr::new("abcdefghijklmnop").is_none());
        assert!(InlineStr::new("éééééééé").is_none());
    }

    #[test]
    fn ascii_tag_and_length_roundtrip() {
        for len in 0..=15 {
            let mut texts = vec!["x".repeat(len)];
            if len >= 2 {
                texts.push(format!("é{}", "x".repeat(len - 2)));
            }
            for text in texts {
                let value = InlineStr::new(&text).unwrap();
                let encoded = minicbor_serde::to_vec(value).unwrap();
                let restored: InlineStr = minicbor_serde::from_slice(&encoded).unwrap();
                for value in [value, restored] {
                    assert_eq!(value.as_bytes().len(), len);
                    assert_eq!(value.is_ascii(), text.is_ascii());
                    assert_eq!(value.char_len(), text.chars().count());
                    assert_eq!(value.as_str(), text);
                }
                assert_eq!(value, restored);
            }
        }
    }

    #[test]
    fn values_serialize_as_text() {
        for length in 0..=15 {
            let text = "x".repeat(length);
            let value = Value::InlineString(InlineStr::new(&text).unwrap());
            let expected = format!(r#"{{"S":"{text}"}}"#);
            assert_eq!(serde_json::to_string(&value).unwrap(), expected);
            let encoded = minicbor_serde::to_vec(&value).unwrap();
            let restored: Value = minicbor_serde::from_slice(&encoded).unwrap();
            assert_eq!(restored.inline_str(), Some(text.as_str()));
        }
    }

    #[test]
    fn deserialization_requires_a_short_string() {
        let long = minicbor_serde::to_vec("x".repeat(16)).unwrap();
        assert!(minicbor_serde::from_slice::<InlineStr>(&long).is_err());
        let wrong_type = minicbor_serde::to_vec(42).unwrap();
        assert!(minicbor_serde::from_slice::<InlineStr>(&wrong_type).is_err());
    }

    #[test]
    fn iterator_roundtrip_preserves_its_position() {
        let stored = serde_json::json!({
            "source": {"Inline": "aé"}, "byte_offset": 1, "ascii": false,
        });
        let iterator: StringIterator = serde_json::from_value(stored.clone()).unwrap();
        let encoded = minicbor_serde::to_vec(&iterator).unwrap();
        let restored: StringIterator = minicbor_serde::from_slice(&encoded).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), stored);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/inline_str_deserialization.rs"]
mod deserialization_tests;
