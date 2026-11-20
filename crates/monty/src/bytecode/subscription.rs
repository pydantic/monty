//! Shared operands for fused subscriptions. Numeric tags are part of the bytecode format.

use super::op::{Opcode, Operand};

/// A key borrowed from a local slot or reconstructed from an embedded literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalSubscriptKey {
    Local(u8),
    String(u16),
    Integer(i16),
}

/// Checked operands for the local subscription encodings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalSubscript {
    object: u8,
    key: LocalSubscriptKey,
}

impl LocalSubscript {
    const STRING_FLAG: u8 = 0x80;
    const OBJECT_MASK: u8 = Self::STRING_FLAG - 1;

    /// Literal keys share the object byte with a tag, leaving seven bits for its slot.
    pub fn new(object: u16, key: LocalSubscriptKey) -> Option<Self> {
        let object = u8::try_from(object).ok()?;
        if !matches!(key, LocalSubscriptKey::Local(_)) && object > Self::OBJECT_MASK {
            None
        } else {
            Some(Self { object, key })
        }
    }

    /// Reconstructs the operands of `LoadSubscrConst` without creating a Python value.
    pub fn decode_constant(key: u16, object_kind: u8) -> Self {
        let key = if object_kind & Self::STRING_FLAG != 0 {
            LocalSubscriptKey::String(key)
        } else {
            LocalSubscriptKey::Integer(key.cast_signed())
        };
        Self {
            object: object_kind & Self::OBJECT_MASK,
            key,
        }
    }

    /// The local slot holding the container.
    pub fn object(self) -> u8 {
        self.object
    }

    /// The key's slot or literal payload.
    pub fn key(self) -> LocalSubscriptKey {
        self.key
    }

    /// Preserves the existing opcode and operand bytes for each key kind.
    pub fn encode(self) -> (Opcode, Operand<'static>) {
        match self.key {
            LocalSubscriptKey::Local(key) => (Opcode::LoadSubscrLocal, Operand::U8U8(self.object, key)),
            LocalSubscriptKey::String(key) => (
                Opcode::LoadSubscrConst,
                Operand::U16U8(key, self.object | Self::STRING_FLAG),
            ),
            LocalSubscriptKey::Integer(key) => (
                Opcode::LoadSubscrConst,
                Operand::U16U8(key.cast_unsigned(), self.object),
            ),
        }
    }
}

/// The key-load instruction retained after `LoadGlobalSubscr` for generic fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub(crate) enum GlobalSubscriptKey {
    Global = 0,
    Constant = 1,
    SmallInteger = 2,
    FixedLocal = 3,
    Local = 4,
    WideLocal = 5,
}

impl GlobalSubscriptKey {
    /// Matches the compiler's fixed, narrow and wide local-load instructions.
    pub fn local(slot: u16) -> Self {
        if slot < 4 {
            Self::FixedLocal
        } else if u8::try_from(slot).is_ok() {
            Self::Local
        } else {
            Self::WideLocal
        }
    }
}

/// A global slot and key-load tag packed into the existing 16-bit operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GlobalSubscript(u16);

impl GlobalSubscript {
    const KEY_SHIFT: u32 = 13;
    const SLOT_MASK: u16 = (1 << Self::KEY_SHIFT) - 1;

    /// Slots outside the packed range use an ordinary global load and subscription.
    pub fn new(slot: u16, key: GlobalSubscriptKey) -> Option<Self> {
        (slot <= Self::SLOT_MASK).then_some(Self(slot | ((key as u16) << Self::KEY_SHIFT)))
    }

    /// Keeps unknown key tags available to the generic fallback.
    pub fn decode(operand: u16) -> Self {
        Self(operand)
    }

    /// Returns the bytes' integer representation for the bytecode builder.
    pub fn encode(self) -> u16 {
        self.0
    }

    /// Extracts the global slot independently of the key-load tag.
    pub fn slot(self) -> u16 {
        self.0 & Self::SLOT_MASK
    }

    /// Unknown tags decline native lookup, matching the existing dispatch behavior.
    pub fn key(self) -> Option<GlobalSubscriptKey> {
        match self.0 >> Self::KEY_SHIFT {
            0 => Some(GlobalSubscriptKey::Global),
            1 => Some(GlobalSubscriptKey::Constant),
            2 => Some(GlobalSubscriptKey::SmallInteger),
            3 => Some(GlobalSubscriptKey::FixedLocal),
            4 => Some(GlobalSubscriptKey::Local),
            5 => Some(GlobalSubscriptKey::WideLocal),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GlobalSubscript, GlobalSubscriptKey, LocalSubscript, LocalSubscriptKey};
    use crate::{
        bytecode::{CodeBuilder, Opcode},
        parse::CodeRange,
    };

    #[test]
    fn local_encoding_keeps_existing_bytes_and_limits() {
        for (object, key, expected) in [
            (
                255,
                LocalSubscriptKey::Local(255),
                vec![Opcode::LoadSubscrLocal as u8, 255, 255],
            ),
            (
                127,
                LocalSubscriptKey::String(0x1234),
                vec![Opcode::LoadSubscrConst as u8, 0x34, 0x12, 255],
            ),
            (
                127,
                LocalSubscriptKey::Integer(i16::MIN),
                vec![Opcode::LoadSubscrConst as u8, 0, 128, 127],
            ),
            (
                127,
                LocalSubscriptKey::Integer(i16::MAX),
                vec![Opcode::LoadSubscrConst as u8, 255, 127, 127],
            ),
        ] {
            let operand = LocalSubscript::new(object, key).unwrap();
            let mut builder = CodeBuilder::new();
            builder.new_code_region(0);
            builder
                .emit_subscript_locals(operand, CodeRange::default(), CodeRange::default())
                .unwrap();
            let code = builder.build();
            assert_eq!(code.bytecode(), expected);
            if expected[0] == Opcode::LoadSubscrConst as u8 {
                let bytes = code.bytecode();
                assert_eq!(
                    LocalSubscript::decode_constant(u16::from_le_bytes([bytes[1], bytes[2]]), bytes[3]),
                    operand
                );
            }
        }
        assert!(LocalSubscript::new(256, LocalSubscriptKey::Local(0)).is_none());
        assert!(LocalSubscript::new(128, LocalSubscriptKey::String(0)).is_none());
        assert!(LocalSubscript::new(128, LocalSubscriptKey::Integer(0)).is_none());
    }

    #[test]
    fn global_encoding_keeps_existing_tags_and_limits() {
        for (key, encoded) in [
            (GlobalSubscriptKey::Global, 0x1fff),
            (GlobalSubscriptKey::Constant, 0x3fff),
            (GlobalSubscriptKey::SmallInteger, 0x5fff),
            (GlobalSubscriptKey::FixedLocal, 0x7fff),
            (GlobalSubscriptKey::Local, 0x9fff),
            (GlobalSubscriptKey::WideLocal, 0xbfff),
        ] {
            assert_eq!(GlobalSubscript::new(8191, key).unwrap().encode(), encoded);
            let decoded = GlobalSubscript::decode(encoded);
            assert_eq!(decoded.slot(), 8191);
            assert_eq!(decoded.key(), Some(key));
            assert!(GlobalSubscript::new(8192, key).is_none());
        }
        assert_eq!(GlobalSubscriptKey::local(3), GlobalSubscriptKey::FixedLocal);
        assert_eq!(GlobalSubscriptKey::local(4), GlobalSubscriptKey::Local);
        assert_eq!(GlobalSubscriptKey::local(255), GlobalSubscriptKey::Local);
        assert_eq!(GlobalSubscriptKey::local(256), GlobalSubscriptKey::WideLocal);
        assert_eq!(GlobalSubscript::decode(0xffff).key(), None);
    }
}
