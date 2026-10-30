//! Fixed-capacity UTF-8 strings shared by values, views and iterators.

use std::str;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

use crate::{bytecode::VM, heap::HeapData, value::Value};

/// UTF-8 text of at most 15 bytes.
/// The length enum supplies niches so a containing `Value` remains 16 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InlineStr {
    len: InlineLen,
    bytes: [u8; 15],
}

impl InlineStr {
    pub(crate) fn new(text: &str) -> Option<Self> {
        let len = InlineLen::from_len(text.len())?;
        let mut bytes = [0; 15];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Some(Self { len, bytes })
    }

    /// Compares without validating UTF-8; non-string operands return `None`.
    pub(crate) fn eq_value(&self, other: &Value, vm: &VM<'_>) -> Option<bool> {
        match other {
            Value::InlineString(other) => Some(self == other),
            Value::InternString(id) => Some(self.as_bytes() == vm.interns.get_str(*id).as_bytes()),
            Value::Ref(id) if let HeapData::Str(other) = vm.heap.get(*id) => {
                Some(self.as_bytes() == other.as_str().as_bytes())
            }
            _ => None,
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len.get()]
    }

    pub(crate) fn as_str(&self) -> &str {
        str::from_utf8(self.as_bytes()).expect("inline string contains valid UTF-8")
    }
}

impl Serialize for InlineStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for InlineStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(&text).ok_or_else(|| D::Error::custom("inline string exceeds capacity"))
    }
}

/// Byte length for an inline UTF-8 string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
enum InlineLen {
    L0,
    L1,
    L2,
    L3,
    L4,
    L5,
    L6,
    L7,
    L8,
    L9,
    L10,
    L11,
    L12,
    L13,
    L14,
    L15,
}

impl InlineLen {
    fn from_len(len: usize) -> Option<Self> {
        Some(match len {
            0 => Self::L0,
            1 => Self::L1,
            2 => Self::L2,
            3 => Self::L3,
            4 => Self::L4,
            5 => Self::L5,
            6 => Self::L6,
            7 => Self::L7,
            8 => Self::L8,
            9 => Self::L9,
            10 => Self::L10,
            11 => Self::L11,
            12 => Self::L12,
            13 => Self::L13,
            14 => Self::L14,
            15 => Self::L15,
            _ => return None,
        })
    }

    fn get(self) -> usize {
        self as usize
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
