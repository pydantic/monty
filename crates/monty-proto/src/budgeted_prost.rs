//! The prost runtime path used by generated messages and the wire decoder.
//!
//! Only non-allocating decode primitives are re-exported. Generated vectors
//! use `BudgetVec`, whose growing operations are fallible. Maps, groups, `Bytes`,
//! generated boxes and repeated-enum accessors remain unsupported.

pub use prost::{DecodeError, Enumeration, Message, Oneof, UnknownEnumValue};

/// Owned types supported by the generated schema (not `Box` or maps).
pub mod alloc {
    pub use prost::alloc::string;

    /// Redirects prost's generated vector fields to fallible storage.
    pub mod vec {
        pub use crate::BudgetVec as Vec;
    }
}

/// Buffer traits needed by prost's derives; owned `Bytes` fields are unsupported.
pub mod bytes {
    pub use prost::bytes::{Buf, BufMut};
}

/// Wire-compatible encoders and fallible decoders over protocol collections.
pub mod encoding {
    pub use prost::encoding::{
        DecodeContext, decode_key, encode_key, encode_varint, encoded_len_varint, key_len, skip_field, wire_type,
    };
    use prost::{
        DecodeError, Message,
        bytes::Buf,
        encoding::{WireType, check_wire_type, decode_varint, merge_loop},
    };

    use crate::{BudgetVec, decode_budget::error};

    /// Generates the scalar adapter, including both packed and unpacked repeated fields.
    macro_rules! scalar {
        ($name:ident, $ty:ty, $wire:ident) => {
            #[doc = concat!("Fallible repeated `", stringify!($name), "` fields.")]
            pub mod $name {
                pub use prost::encoding::$name::{
                    encode, encode_packed, encode_repeated, encoded_len, encoded_len_packed, encoded_len_repeated,
                    merge,
                };

                use super::{BudgetVec, Buf, DecodeContext, DecodeError, WireType, check_wire_type, merge_loop};

                /// Decodes either protobuf representation with fallible insertion.
                pub fn merge_repeated(
                    wire_type: WireType,
                    values: &mut BudgetVec<$ty>,
                    buf: &mut impl Buf,
                    ctx: DecodeContext,
                ) -> Result<(), DecodeError> {
                    let merge_one = |values: &mut BudgetVec<$ty>, buf: &mut _, ctx| {
                        let mut value = <$ty>::default();
                        merge(WireType::$wire, &mut value, buf, ctx)?;
                        values.try_push(value)
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

    /// Repeated length-delimited values reserve a slot before decoding their payload.
    macro_rules! repeated {
        ($ty:ty) => {
            /// Decodes one repeated field into fallible storage.
            pub fn merge_repeated(
                wire_type: WireType,
                values: &mut BudgetVec<$ty>,
                buf: &mut impl Buf,
                ctx: DecodeContext,
            ) -> Result<(), DecodeError> {
                check_wire_type(WireType::LengthDelimited, wire_type)?;
                values.try_reserve_slot()?;
                let mut value = <$ty>::default();
                merge(wire_type, &mut value, buf, ctx)?;
                values.try_push(value)
            }
        };
    }

    /// UTF-8 strings backed by a fallible byte buffer during decoding.
    pub mod string {
        use std::mem;

        pub use prost::encoding::string::{encode, encode_repeated, encoded_len, encoded_len_repeated};

        use super::{BudgetVec, Buf, DecodeContext, DecodeError, WireType, check_wire_type, error};

        /// Reuses the string's allocation without exposing invalid UTF-8 on error or unwind.
        pub fn merge(
            wire_type: WireType,
            value: &mut String,
            buf: &mut impl Buf,
            ctx: DecodeContext,
        ) -> Result<(), DecodeError> {
            let mut bytes = BudgetVec::from(mem::take(value).into_bytes());
            super::bytes::merge(wire_type, &mut bytes, buf, ctx)?;
            *value = String::from_utf8(bytes.into_inner())
                .map_err(|_| error("invalid string value: data is not UTF-8 encoded"))?;
            Ok(())
        }

        repeated!(String);
    }

    /// Owned byte buffers; prost's sealed `BytesAdapter` cannot accept `BudgetVec`.
    pub mod bytes {
        use prost::bytes::BufMut;

        use super::{
            BudgetVec, Buf, DecodeContext, DecodeError, WireType, check_wire_type, decode_varint, encode_key,
            encode_varint, encoded_len_varint, error, key_len,
        };

        /// Encodes borrowed bytes without converting their owning collection.
        pub fn encode(tag: u32, value: &impl AsRef<[u8]>, buf: &mut impl BufMut) {
            let bytes = value.as_ref();
            encode_key(tag, WireType::LengthDelimited, buf);
            encode_varint(bytes.len() as u64, buf);
            buf.put_slice(bytes);
        }

        /// Encodes a sequence of byte buffers without intermediate allocations.
        pub fn encode_repeated(tag: u32, values: &[impl AsRef<[u8]>], buf: &mut impl BufMut) {
            for value in values {
                encode(tag, value, buf);
            }
        }

        /// Returns the byte-field length including its key and delimiter.
        pub fn encoded_len(tag: u32, value: &impl AsRef<[u8]>) -> usize {
            let len = value.as_ref().len();
            key_len(tag) + encoded_len_varint(len as u64) + len
        }

        /// Returns the encoded length of repeated byte buffers.
        pub fn encoded_len_repeated(tag: u32, values: &[impl AsRef<[u8]>]) -> usize {
            values.iter().map(|value| encoded_len(tag, value)).sum()
        }

        /// Checks the delimiter before the fallible buffer performs any growth.
        pub fn merge(
            wire_type: WireType,
            value: &mut BudgetVec<u8>,
            buf: &mut impl Buf,
            _ctx: DecodeContext,
        ) -> Result<(), DecodeError> {
            check_wire_type(WireType::LengthDelimited, wire_type)?;
            let len = decode_varint(buf)?;
            if len > buf.remaining() as u64 {
                return Err(error("buffer underflow"));
            }
            let len = usize::try_from(len).map_err(|_| error("buffer underflow"))?;
            value.try_replace(buf.take(len))
        }

        repeated!(BudgetVec<u8>);
    }

    /// Message recursion stays in prost; insertion into repeated fields is fallible.
    pub mod message {
        pub use prost::encoding::message::{encode, encode_repeated, encoded_len, encoded_len_repeated, merge};

        use super::{BudgetVec, Buf, DecodeContext, DecodeError, Message, WireType, check_wire_type};

        /// Reserves storage before decoding a potentially expensive message payload.
        pub fn merge_repeated(
            wire_type: WireType,
            values: &mut BudgetVec<impl Message + Default>,
            buf: &mut impl Buf,
            ctx: DecodeContext,
        ) -> Result<(), DecodeError> {
            check_wire_type(WireType::LengthDelimited, wire_type)?;
            values.try_reserve_slot()?;
            let mut value = Default::default();
            merge(wire_type, &mut value, buf, ctx)?;
            values.try_push(value)
        }
    }
}
