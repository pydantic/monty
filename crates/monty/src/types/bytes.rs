/// Python bytes type, wrapping a `Vec<u8>`.
///
/// This type provides Python bytes semantics. Currently supports basic
/// operations like length and equality comparison.
use std::fmt::Write;

use ahash::AHashSet;

use super::{PyTrait, Type};
use crate::{
    args::ArgValues,
    exception_private::ExcType,
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    value::Value,
};

/// Python bytes value stored on the heap.
///
/// Wraps a `Vec<u8>` and provides Python-compatible operations.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Bytes(Vec<u8>);

impl Bytes {
    /// Creates a new Bytes from a byte vector.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns a reference to the inner byte slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Returns a mutable reference to the inner byte vector.
    pub fn as_vec_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }

    /// Creates bytes from the `bytes()` constructor call.
    ///
    /// - `bytes()` with no args returns empty bytes
    /// - `bytes(int)` returns bytes of that length filled with zeros
    /// - `bytes(string)` encodes the string as UTF-8 (simplified, no encoding param)
    /// - `bytes(bytes)` returns a copy of the bytes
    ///
    /// Note: Full Python semantics for bytes() are more complex (encoding, errors params).
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("bytes")?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::Bytes(Bytes::new(Vec::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let result = match &v {
                    Value::Int(n) => {
                        if *n < 0 {
                            return Err(ExcType::value_error_negative_bytes_count());
                        }
                        let bytes = vec![0u8; *n as usize];
                        heap.allocate(HeapData::Bytes(Bytes::new(bytes)))
                    }
                    Value::InternString(string_id) => {
                        let s = interns.get_str(*string_id);
                        heap.allocate(HeapData::Bytes(Bytes::new(s.as_bytes().to_vec())))
                    }
                    Value::InternBytes(bytes_id) => {
                        let b = interns.get_bytes(*bytes_id);
                        heap.allocate(HeapData::Bytes(Bytes::new(b.to_vec())))
                    }
                    Value::Ref(id) => match heap.get(*id) {
                        HeapData::Str(s) => heap.allocate(HeapData::Bytes(Bytes::new(s.as_str().as_bytes().to_vec()))),
                        HeapData::Bytes(b) => heap.allocate(HeapData::Bytes(Bytes::new(b.as_slice().to_vec()))),
                        _ => {
                            let err = ExcType::type_error_bytes_init(v.py_type(Some(heap)));
                            v.drop_with_heap(heap);
                            return Err(err);
                        }
                    },
                    _ => {
                        let err = ExcType::type_error_bytes_init(v.py_type(Some(heap)));
                        v.drop_with_heap(heap);
                        return Err(err);
                    }
                };
                v.drop_with_heap(heap);
                Ok(Value::Ref(result?))
            }
        }
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}

impl std::ops::Deref for Bytes {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PyTrait for Bytes {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::Bytes
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.0.len()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.0.len())
    }

    fn py_eq(&self, other: &Self, _heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        self.0 == other.0
    }

    /// Bytes don't contain nested heap references.
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No-op: bytes don't hold Value references
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.0.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _interns: &Interns,
    ) -> std::fmt::Result {
        bytes_repr_fmt(&self.0, f)
    }
    // py_call_attr uses default implementation which returns AttributeError
}

/// Writes a CPython-compatible repr string for bytes to a formatter.
///
/// Format: `b'...'` or `b"..."` depending on content.
/// - Uses single quotes by default
/// - Switches to double quotes if bytes contain `'` but not `"`
/// - Escapes: `\\`, `\t`, `\n`, `\r`, `\xNN` for non-printable bytes
pub fn bytes_repr_fmt(bytes: &[u8], f: &mut impl Write) -> std::fmt::Result {
    // Determine quote character: use double quotes if single quote present but not double
    let has_single = bytes.contains(&b'\'');
    let has_double = bytes.contains(&b'"');
    let quote = if has_single && !has_double { '"' } else { '\'' };

    f.write_char('b')?;
    f.write_char(quote)?;

    for &byte in bytes {
        match byte {
            b'\\' => f.write_str("\\\\")?,
            b'\t' => f.write_str("\\t")?,
            b'\n' => f.write_str("\\n")?,
            b'\r' => f.write_str("\\r")?,
            b'\'' if quote == '\'' => f.write_str("\\'")?,
            b'"' if quote == '"' => f.write_str("\\\"")?,
            // Printable ASCII (32-126)
            0x20..=0x7e => f.write_char(byte as char)?,
            // Non-printable: use \xNN format
            _ => write!(f, "\\x{byte:02x}")?,
        }
    }

    f.write_char(quote)
}

/// Returns a CPython-compatible repr string for bytes.
///
/// Convenience wrapper around `bytes_repr_fmt` that returns an owned String.
#[must_use]
pub fn bytes_repr(bytes: &[u8]) -> String {
    let mut result = String::new();
    // Writing to String never fails
    bytes_repr_fmt(bytes, &mut result).unwrap();
    result
}
