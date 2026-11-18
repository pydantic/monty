//! Dump encoding of a [`Code`](super::Code)'s location table.
//!
//! The table is written as one CBOR byte string of delta-encoded varints instead of
//! a map per entry, which is about seven times smaller and is written in one call.
//! The string starts with a format byte: a layout change takes a new format byte and
//! keeps the decoder for every older one, since serde's field evolution cannot see
//! inside the string. Dumps written before this encoding hold a sequence of entry
//! maps, which [`deserialize`] still reads.

use std::fmt;

use serde::{
    Deserializer, Serializer,
    de::{Error, SeqAccess, Visitor},
};

use super::LocationEntry;
use crate::{intern::StringId, parse::CodeRange};

/// The current layout. After the format byte, each entry is LEB128 varints:
///
/// - `(offset_delta << FLAG_BITS) | flags`, the offset relative to the previous entry's
/// - the range's filename, if [`NEW_FILENAME`]
/// - the range's start as a zigzag delta from the previous entry's start, then its width
/// - if [`HAS_FOCUS`]: its filename if [`FOCUS_FILENAME`], its start as a zigzag delta
///   from the range's start, then its width
///
/// Differences wrap, so every table round-trips even if its offsets are unsorted.
const FORMAT_V1: u8 = 1;

/// The entry has a focus range.
const HAS_FOCUS: u64 = 1 << 0;
/// The range's filename differs from the previous entry's (always set on the first).
const NEW_FILENAME: u64 = 1 << 1;
/// The focus's filename differs from its range's.
const FOCUS_FILENAME: u64 = 1 << 2;
/// Low bits of an entry header holding its flags.
const FLAG_BITS: u32 = 3;

/// Writes `table` as one byte string in the [`FORMAT_V1`] layout.
pub(super) fn serialize<S: Serializer>(table: &[LocationEntry], serializer: S) -> Result<S::Ok, S::Error> {
    // a typical entry takes 3 or 4 bytes
    let mut bytes = Vec::with_capacity(1 + table.len() * 4);
    bytes.push(FORMAT_V1);
    let mut previous = Previous::default();
    for entry in table {
        encode_entry(entry, &mut previous, &mut bytes);
    }
    serializer.serialize_bytes(&bytes)
}

/// Reads a table in any layout a dump may hold: a byte string (by its format byte),
/// or the per-entry maps older dumps wrote.
pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<LocationEntry>, D::Error> {
    deserializer.deserialize_any(TableVisitor)
}

/// The previous entry's values, which the next entry is encoded relative to.
#[derive(Default)]
struct Previous {
    offset: u32,
    start: u32,
    filename: Option<StringId>,
}

/// Appends one entry to `bytes`, then makes it the base for the next.
fn encode_entry(entry: &LocationEntry, previous: &mut Previous, bytes: &mut Vec<u8>) {
    let LocationEntry {
        bytecode_offset,
        range,
        focus,
    } = *entry;
    let new_filename = previous.filename != Some(range.filename);
    let focus_filename = focus.is_some_and(|focus| focus.filename != range.filename);
    let mut flags = 0;
    if focus.is_some() {
        flags |= HAS_FOCUS;
    }
    if new_filename {
        flags |= NEW_FILENAME;
    }
    if focus_filename {
        flags |= FOCUS_FILENAME;
    }
    let offset_delta = bytecode_offset.wrapping_sub(previous.offset);
    write_varint(bytes, (u64::from(offset_delta) << FLAG_BITS) | flags);
    if new_filename {
        write_varint(bytes, u64::from(range.filename.raw()));
    }
    write_range(bytes, range, previous.start);
    if let Some(focus) = focus {
        if focus_filename {
            write_varint(bytes, u64::from(focus.filename.raw()));
        }
        write_range(bytes, focus, range.start_byte);
    }
    *previous = Previous {
        offset: bytecode_offset,
        start: range.start_byte,
        filename: Some(range.filename),
    };
}

/// Writes a range's start relative to `base` and its width; its filename is written separately.
fn write_range(bytes: &mut Vec<u8>, range: CodeRange, base: u32) {
    write_varint(bytes, zigzag(range.start_byte.wrapping_sub(base)));
    write_varint(bytes, u64::from(range.end_byte.wrapping_sub(range.start_byte)));
}

/// Writes `value` as LEB128: seven bits per byte, the high bit set on all but the last.
fn write_varint(bytes: &mut Vec<u8>, mut value: u64) {
    // the low byte carries the next seven bits; the high bit marks a continuation
    while value >= 0x80 {
        bytes.push(value.to_le_bytes()[0] | 0x80);
        value >>= 7;
    }
    bytes.push(value.to_le_bytes()[0]);
}

/// Maps a wrapped difference to an unsigned value small for small moves either way.
fn zigzag(delta: u32) -> u64 {
    let signed = delta.cast_signed();
    u64::from(((signed << 1) ^ (signed >> 31)).cast_unsigned())
}

/// Inverts [`zigzag`].
fn unzigzag(value: u32) -> u32 {
    (value >> 1) ^ (value & 1).wrapping_neg()
}

/// Accepts the byte string this module writes or the sequence of entry maps older dumps hold.
struct TableVisitor;

impl<'de> Visitor<'de> for TableVisitor {
    type Value = Vec<LocationEntry>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a location table byte string or a sequence of location entries")
    }

    fn visit_bytes<E: Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
        decode(bytes).map_err(E::custom)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut table = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(entry) = seq.next_element()? {
            table.push(entry);
        }
        Ok(table)
    }
}

/// Decodes a byte string written by [`serialize`] in any format this build knows.
fn decode(bytes: &[u8]) -> Result<Vec<LocationEntry>, &'static str> {
    let (&format, body) = bytes.split_first().ok_or("empty location table")?;
    match format {
        FORMAT_V1 => decode_v1(body),
        _ => Err("unknown location table format"),
    }
}

/// Decodes the entries of a [`FORMAT_V1`] table.
fn decode_v1(body: &[u8]) -> Result<Vec<LocationEntry>, &'static str> {
    let mut reader = Reader(body);
    // every entry takes at least three bytes
    let mut table = Vec::with_capacity(body.len() / 3);
    let mut previous = Previous::default();
    while !reader.0.is_empty() {
        let header = reader.varint()?;
        let offset_delta = u32::try_from(header >> FLAG_BITS).map_err(|_| "location offset overflow")?;
        let filename = if header & NEW_FILENAME == 0 {
            previous.filename.ok_or("first location entry has no filename")?
        } else {
            StringId::from_raw(reader.u32()?)
        };
        let range = reader.range(filename, previous.start)?;
        let focus = if header & HAS_FOCUS == 0 {
            None
        } else {
            let focus_filename = if header & FOCUS_FILENAME == 0 {
                filename
            } else {
                StringId::from_raw(reader.u32()?)
            };
            Some(reader.range(focus_filename, range.start_byte)?)
        };
        let bytecode_offset = previous.offset.wrapping_add(offset_delta);
        table.push(LocationEntry::new(bytecode_offset, range, focus));
        previous = Previous {
            offset: bytecode_offset,
            start: range.start_byte,
            filename: Some(filename),
        };
    }
    Ok(table)
}

/// The unread rest of a [`FORMAT_V1`] table.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    /// Reads a range written by [`write_range`] against `base`.
    fn range(&mut self, filename: StringId, base: u32) -> Result<CodeRange, &'static str> {
        let start_byte = base.wrapping_add(unzigzag(self.u32()?));
        let end_byte = start_byte.wrapping_add(self.u32()?);
        Ok(CodeRange {
            filename,
            start_byte,
            end_byte,
        })
    }

    /// Reads a varint that must fit a `u32`.
    fn u32(&mut self) -> Result<u32, &'static str> {
        u32::try_from(self.varint()?).map_err(|_| "location value overflow")
    }

    /// Reads a LEB128 varint written by [`write_varint`].
    fn varint(&mut self) -> Result<u64, &'static str> {
        let mut value = 0;
        let mut shift = 0;
        loop {
            let (&byte, rest) = self.0.split_first().ok_or("truncated location table")?;
            self.0 = rest;
            if shift > 63 {
                return Err("location varint too long");
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/location_table.rs"]
mod tests;
