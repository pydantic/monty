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
        use std::str;

        pub use prost::encoding::string::{encode, encode_repeated, encoded_len, encoded_len_repeated};

        use super::{BudgetVec, Buf, DecodeContext, DecodeError, WireType, check_wire_type, decode_varint, error};
        use crate::decode_budget::{charge, exhausted};

        /// Window size for validating and copying a string payload.
        const UTF8_WINDOW: usize = 4 * 1024 * 1024;

        /// Appends a contiguous UTF-8 payload while validating it in cache-sized windows.
        fn append_utf8(value: &mut String, src: &[u8]) -> Result<(), DecodeError> {
            let mut pos = 0;
            while pos < src.len() {
                let mut end = src.len().min(pos + UTF8_WINDOW);
                for _ in 0..3 {
                    if end > pos && end < src.len() && src[end] & 0xC0 == 0x80 {
                        end -= 1;
                    }
                }
                value.push_str(
                    str::from_utf8(&src[pos..end])
                        .map_err(|_| error("invalid string value: data is not UTF-8 encoded"))?,
                );
                pos = end;
            }
            Ok(())
        }

        /// Validates and copies a string payload in cache-sized windows.
        pub fn merge(
            wire_type: WireType,
            value: &mut String,
            buf: &mut impl Buf,
            _ctx: DecodeContext,
        ) -> Result<(), DecodeError> {
            check_wire_type(WireType::LengthDelimited, wire_type)?;
            let len = decode_varint(buf)?;
            let len = usize::try_from(len)
                .ok()
                .filter(|len| *len <= buf.remaining())
                .ok_or_else(|| error("buffer underflow"))?;

            // A singular protobuf string replaces the previous value. A
            // larger replacement is charged in full, matching BudgetVec's
            // replacement allocation accounting. Allocate a fresh exact
            // buffer here: String growth may round small allocations up.
            if len > value.capacity() {
                let mut replacement = String::new();
                charge(len)?;
                replacement.try_reserve_exact(len).map_err(|_| exhausted())?;
                *value = replacement;
            } else {
                value.clear();
            }
            if buf.chunk().len() >= len {
                append_utf8(value, &buf.chunk()[..len])?;
                buf.advance(len);
                Ok(())
            } else {
                // A Buf can expose a length-delimited payload in pieces. Gather
                // that uncommon path before validating it as one string.
                let mut bytes = vec![0u8; len];
                buf.copy_to_slice(&mut bytes);
                *value =
                    String::from_utf8(bytes).map_err(|_| error("invalid string value: data is not UTF-8 encoded"))?;
                Ok(())
            }
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
