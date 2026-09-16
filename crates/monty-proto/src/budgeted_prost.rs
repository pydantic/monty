//! The prost runtime path used by generated messages and the wire decoder.
//!
//! Only non-allocating decode primitives are re-exported. All buffer growth
//! goes through `decode_budget`; maps, groups, `Bytes` and generated boxes are
//! deliberately unsupported, so new allocation forms cannot silently bypass it.

pub use prost::{DecodeError, Enumeration, Message, Oneof, UnknownEnumValue};

/// Owned types supported by the generated schema (not `Box` or maps).
pub mod alloc {
    pub use prost::alloc::{string, vec};
}

/// Buffer traits needed by prost's derives; owned `Bytes` fields are unsupported.
pub mod bytes {
    pub use prost::bytes::{Buf, BufMut};
}

/// Wire-compatible encoders and allocation-budgeted decoders.
pub mod encoding {
    pub use prost::encoding::{
        DecodeContext, decode_key, encode_key, encode_varint, encoded_len_varint, key_len, skip_field, wire_type,
    };
    use prost::{
        DecodeError, Message,
        bytes::Buf,
        encoding::{WireType, check_wire_type, decode_varint, merge_loop},
    };

    use crate::decode_budget::{error, reserve, reserve_slot};

    /// Generates the scalar adapter, including both packed and unpacked repeated fields.
    macro_rules! scalar {
        ($name:ident, $ty:ty, $wire:ident) => {
            #[doc = concat!("Budgeted repeated `", stringify!($name), "` fields.")]
            pub mod $name {
                pub use prost::encoding::$name::{
                    encode, encode_packed, encode_repeated, encoded_len, encoded_len_packed, encoded_len_repeated,
                    merge,
                };

                use super::{Buf, DecodeContext, DecodeError, WireType, check_wire_type, merge_loop, reserve_slot};

                /// Decodes either protobuf representation, reserving before each new slot.
                pub fn merge_repeated(
                    wire_type: WireType,
                    values: &mut Vec<$ty>,
                    buf: &mut impl Buf,
                    ctx: DecodeContext,
                ) -> Result<(), DecodeError> {
                    let merge_one = |values: &mut Vec<$ty>, buf: &mut _, ctx| {
                        reserve_slot(values)?;
                        let mut value = <$ty>::default();
                        merge(WireType::$wire, &mut value, buf, ctx)?;
                        values.push(value);
                        Ok(())
                    };
                    if wire_type == WireType::LengthDelimited {
                        merge_loop(values, buf, ctx, merge_one)
                    } else {
                        check_wire_type(WireType::$wire, wire_type)?;
                        merge_one(values, buf, ctx)
                    }
                }
            }
        };
    }

    scalar!(bool, bool, Varint);
    scalar!(int32, i32, Varint);
    scalar!(int64, i64, Varint);
    scalar!(uint32, u32, Varint);
    scalar!(uint64, u64, Varint);
    scalar!(sint32, i32, Varint);
    scalar!(sint64, i64, Varint);
    scalar!(fixed32, u32, ThirtyTwoBit);
    scalar!(fixed64, u64, SixtyFourBit);
    scalar!(sfixed32, i32, ThirtyTwoBit);
    scalar!(sfixed64, i64, SixtyFourBit);
    scalar!(float, f32, ThirtyTwoBit);
    scalar!(double, f64, SixtyFourBit);

    /// Repeated length-delimited values all own their vector slots here.
    macro_rules! repeated {
        ($ty:ty) => {
            /// Decodes one repeated field, budgeting its slot before its payload.
            pub fn merge_repeated(
                wire_type: WireType,
                values: &mut Vec<$ty>,
                buf: &mut impl Buf,
                ctx: DecodeContext,
            ) -> Result<(), DecodeError> {
                check_wire_type(WireType::LengthDelimited, wire_type)?;
                reserve_slot(values)?;
                let mut value = <$ty>::default();
                merge(wire_type, &mut value, buf, ctx)?;
                values.push(value);
                Ok(())
            }
        };
    }

    /// UTF-8 strings with explicitly reserved backing buffers.
    pub mod string {
        use std::mem;

        pub use prost::encoding::string::{encode, encode_repeated, encoded_len, encoded_len_repeated};

        use super::{Buf, DecodeContext, DecodeError, WireType, check_wire_type, error, reserve_slot};

        /// Reuses the string's allocation without exposing invalid UTF-8 on error or unwind.
        pub fn merge(
            wire_type: WireType,
            value: &mut String,
            buf: &mut impl Buf,
            ctx: DecodeContext,
        ) -> Result<(), DecodeError> {
            let mut bytes = mem::take(value).into_bytes();
            super::bytes::merge(wire_type, &mut bytes, buf, ctx)?;
            *value = String::from_utf8(bytes).map_err(|_| error("invalid string value: data is not UTF-8 encoded"))?;
            Ok(())
        }

        repeated!(String);
    }

    /// Owned byte buffers; `Bytes` is intentionally not a supported field type.
    pub mod bytes {
        pub use prost::encoding::bytes::{encode, encode_repeated, encoded_len, encoded_len_repeated};

        use super::{
            Buf, DecodeContext, DecodeError, WireType, check_wire_type, decode_varint, error, reserve, reserve_slot,
        };

        /// Checks the delimiter before allocating, then copies directly from any `Buf`.
        pub fn merge(
            wire_type: WireType,
            value: &mut Vec<u8>,
            buf: &mut impl Buf,
            _ctx: DecodeContext,
        ) -> Result<(), DecodeError> {
            check_wire_type(WireType::LengthDelimited, wire_type)?;
            let len = decode_varint(buf)?;
            if len > buf.remaining() as u64 {
                return Err(error("buffer underflow"));
            }
            let len = usize::try_from(len).map_err(|_| error("buffer underflow"))?;
            reserve(value, len)?;
            value.clear();
            // Unlike prost's generic BytesAdapter path, this never creates a temporary copy.
            let mut field = buf.take(len);
            while field.has_remaining() {
                let chunk = field.chunk();
                value.extend_from_slice(chunk);
                let n = chunk.len();
                field.advance(n);
            }
            Ok(())
        }

        repeated!(Vec<u8>);
    }

    /// Message recursion stays in prost; repeated message storage is budgeted here.
    pub mod message {
        pub use prost::encoding::message::{encode, encode_repeated, encoded_len, encoded_len_repeated, merge};

        use super::{Buf, DecodeContext, DecodeError, Message, WireType, check_wire_type, reserve_slot};

        /// Reserves each repeated message's inline storage before decoding its fields.
        pub fn merge_repeated(
            wire_type: WireType,
            values: &mut Vec<impl Message + Default>,
            buf: &mut impl Buf,
            ctx: DecodeContext,
        ) -> Result<(), DecodeError> {
            check_wire_type(WireType::LengthDelimited, wire_type)?;
            reserve_slot(values)?;
            let mut value = Default::default();
            merge(wire_type, &mut value, buf, ctx)?;
            values.push(value);
            Ok(())
        }
    }
}
