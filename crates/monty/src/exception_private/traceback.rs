//! Reads flat traceback arrays and the linked frame maps written by older dumps.

use std::fmt;

use serde::{
    Deserialize, Deserializer,
    de::{DeserializeSeed, Error, IgnoredAny, MapAccess, SeqAccess, Visitor, value::SeqAccessDeserializer},
};

use super::RawStackFrame;
use crate::{intern::StringId, parse::CodeRange};

/// The serialized `frame` field used to be an optional linked frame; new dumps write an array.
pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<RawStackFrame>, D::Error> {
    deserializer.deserialize_any(TracebackVisitor)
}

/// Distinguishes the array encoding from legacy maps and empty tracebacks.
struct TracebackVisitor;

impl<'de> Visitor<'de> for TracebackVisitor {
    type Value = Vec<RawStackFrame>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a traceback array or a legacy frame")
    }

    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(Vec::new())
    }

    fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<Self::Value, A::Error> {
        Vec::deserialize(SeqAccessDeserializer::new(sequence))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let mut frames = Vec::new();
        LegacyFrames(&mut frames).visit_map(map)?;
        frames
            .into_iter()
            .map(|frame| {
                Ok(RawStackFrame {
                    position: frame.position.ok_or_else(|| A::Error::missing_field("position"))?,
                    frame_name: frame.frame_name,
                    hide_caret: frame.hide_caret.ok_or_else(|| A::Error::missing_field("hide_caret"))?,
                })
            })
            .collect()
    }
}

/// A legacy frame's fields can occur before or after its parent map.
#[derive(Default)]
struct LegacyFrameFields {
    position: Option<CodeRange>,
    frame_name: Option<StringId>,
    hide_caret: Option<bool>,
}

/// Collects legacy frames directly into one buffer instead of allocating a linked tree.
struct LegacyFrames<'a>(&'a mut Vec<LegacyFrameFields>);

impl<'de> DeserializeSeed<'de> for LegacyFrames<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for LegacyFrames<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a legacy frame or null")
    }

    fn visit_unit<E: Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_none<E: Error>(self) -> Result<(), E> {
        self.visit_unit()
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let index = self.0.len();
        self.0.push(LegacyFrameFields::default());
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "position" => self.0[index].position = Some(map.next_value()?),
                "frame_name" => self.0[index].frame_name = map.next_value()?,
                "hide_caret" => self.0[index].hide_caret = Some(map.next_value()?),
                "parent" => map.next_value_seed(LegacyFrames(self.0))?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}
