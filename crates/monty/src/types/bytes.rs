/// Python bytes type, wrapping a `Vec<u8>`.
///
/// This type provides Python bytes semantics including searching, decoding,
/// and prefix/suffix operations.
///
/// # Implemented Methods
/// - `decode([encoding[, errors]])` - Decode to string (UTF-8 only)
/// - `count(sub[, start[, end]])` - Count non-overlapping occurrences
/// - `find(sub[, start[, end]])` - Find first occurrence (-1 if not found)
/// - `index(sub[, start[, end]])` - Find first occurrence (raises ValueError)
/// - `startswith(prefix)` - Check if starts with prefix
/// - `endswith(suffix)` - Check if ends with suffix
///
/// # Unimplemented Methods
/// - `capitalize()`, `center()`, `expandtabs()`, `ljust()`, `lower()`,
///   `lstrip()`, `rjust()`, `rstrip()`, `strip()`, `swapcase()`, `title()`,
///   `upper()`, `zfill()` - Case/whitespace transformations
/// - `hex()` - Hex string representation
/// - `isalnum()`, `isalpha()`, `isascii()`, `isdigit()`, `islower()`,
///   `isspace()`, `istitle()`, `isupper()` - Character class tests
/// - `join()` - Join bytes sequences
/// - `partition()`, `rpartition()` - Split into 3 parts
/// - `replace()` - Replace occurrences
/// - `removeprefix()`, `removesuffix()` - Remove prefix/suffix
/// - `rfind()`, `rindex()` - Find from right
/// - `split()`, `rsplit()`, `splitlines()` - Split into list
/// - `translate()` - Character translation
/// - `fromhex()` - Create from hex string (classmethod)
/// - `maketrans()` - Create translation table (staticmethod)
use std::fmt::Write;

use ahash::AHashSet;

use super::{PyTrait, Type, str::Str};
use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StringId, attr},
    resource::ResourceTracker,
    value::{Attr, Value},
};

/// Python bytes value stored on the heap.
///
/// Wraps a `Vec<u8>` and provides Python-compatible operations.
/// See the module-level documentation for implemented and unimplemented methods.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Bytes(Vec<u8>);

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
        let value = args.get_zero_one_arg("bytes", heap)?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::Bytes(Self::new(Vec::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let result = match &v {
                    Value::Int(n) => {
                        if *n < 0 {
                            return Err(ExcType::value_error_negative_bytes_count());
                        }
                        let size = usize::try_from(*n).expect("bytes count validated non-negative");
                        let bytes = vec![0u8; size];
                        heap.allocate(HeapData::Bytes(Self::new(bytes)))
                    }
                    Value::InternString(string_id) => {
                        let s = interns.get_str(*string_id);
                        heap.allocate(HeapData::Bytes(Self::new(s.as_bytes().to_vec())))
                    }
                    Value::InternBytes(bytes_id) => {
                        let b = interns.get_bytes(*bytes_id);
                        heap.allocate(HeapData::Bytes(Self::new(b.to_vec())))
                    }
                    Value::Ref(id) => match heap.get(*id) {
                        HeapData::Str(s) => heap.allocate(HeapData::Bytes(Self::new(s.as_str().as_bytes().to_vec()))),
                        HeapData::Bytes(b) => heap.allocate(HeapData::Bytes(Self::new(b.as_slice().to_vec()))),
                        _ => {
                            let err = ExcType::type_error_bytes_init(v.py_type(heap));
                            v.drop_with_heap(heap);
                            return Err(err);
                        }
                    },
                    _ => {
                        let err = ExcType::type_error_bytes_init(v.py_type(heap));
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
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
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

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(attr_id) = attr.string_id() else {
            args.drop_with_heap(heap);
            return Err(ExcType::attribute_error(Type::Bytes, attr.as_str(interns)));
        };

        call_bytes_method(self.as_slice(), attr_id, args, heap, interns)
    }
}

/// Calls a bytes method on a byte slice.
///
/// This is the unified entry point for bytes method calls, used by both
/// heap-allocated `Bytes` (via `py_call_attr`) and interned bytes literals
/// (`Value::InternBytes`).
pub fn call_bytes_method(
    bytes: &[u8],
    method_id: StringId,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    match method_id {
        attr::DECODE => bytes_decode(bytes, args, heap, interns),
        attr::COUNT => bytes_count(bytes, args, heap, interns),
        attr::FIND => bytes_find(bytes, args, heap, interns),
        attr::INDEX => bytes_index(bytes, args, heap, interns),
        attr::STARTSWITH => bytes_startswith(bytes, args, heap, interns),
        attr::ENDSWITH => bytes_endswith(bytes, args, heap, interns),
        _ => {
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error(Type::Bytes, interns.get_str(method_id)))
        }
    }
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

/// Implements Python's `bytes.decode([encoding[, errors]])` method.
///
/// Converts bytes to a string. Currently only supports UTF-8 encoding.
fn bytes_decode(
    bytes: &[u8],
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (encoding, errors) = args.get_zero_one_two_args("bytes.decode", heap)?;

    // Check encoding (default UTF-8)
    let encoding_str = if let Some(enc) = encoding {
        let result = get_encoding_str(&enc, heap, interns);
        enc.drop_with_heap(heap);
        match result {
            Ok(s) => s,
            Err(e) => {
                if let Some(err) = errors {
                    err.drop_with_heap(heap);
                }
                return Err(e);
            }
        }
    } else {
        "utf-8".to_owned()
    };

    // Drop the errors argument (we don't use it yet)
    if let Some(err) = errors {
        err.drop_with_heap(heap);
    }

    // Only support UTF-8 family
    let normalized = encoding_str.to_lowercase();
    if !matches!(normalized.as_str(), "utf-8" | "utf8" | "utf_8") {
        return Err(ExcType::lookup_error_unknown_encoding(&encoding_str));
    }

    // Decode as UTF-8
    match std::str::from_utf8(bytes) {
        Ok(s) => {
            let heap_id = heap.allocate(HeapData::Str(Str::from(s.to_owned())))?;
            Ok(Value::Ref(heap_id))
        }
        Err(_) => Err(ExcType::unicode_decode_error_invalid_utf8()),
    }
}

/// Helper function to extract encoding string from a value.
fn get_encoding_str(encoding: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<String> {
    match encoding {
        Value::InternString(id) => Ok(interns.get_str(*id).to_owned()),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(s) => Ok(s.as_str().to_owned()),
            _ => Err(ExcType::type_error(
                "decode() argument 'encoding' must be str, not bytes",
            )),
        },
        _ => Err(ExcType::type_error("decode() argument 'encoding' must be str, not int")),
    }
}

/// Implements Python's `bytes.count(sub[, start[, end]])` method.
///
/// Returns the number of non-overlapping occurrences of the subsequence.
fn bytes_count(
    bytes: &[u8],
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (sub, start, end) = parse_bytes_sub_args("bytes.count", bytes.len(), args, heap, interns)?;

    let slice = &bytes[start..end];
    let count = if sub.is_empty() {
        // Empty subsequence: count positions between each byte plus 1
        slice.len() + 1
    } else {
        count_non_overlapping(slice, &sub)
    };

    let count_i64 = i64::try_from(count).expect("count exceeds i64::MAX");
    Ok(Value::Int(count_i64))
}

/// Counts non-overlapping occurrences of needle in haystack.
fn count_non_overlapping(haystack: &[u8], needle: &[u8]) -> usize {
    let mut count = 0;
    let mut pos = 0;
    while pos + needle.len() <= haystack.len() {
        if &haystack[pos..pos + needle.len()] == needle {
            count += 1;
            pos += needle.len();
        } else {
            pos += 1;
        }
    }
    count
}

/// Implements Python's `bytes.find(sub[, start[, end]])` method.
///
/// Returns the lowest index where the subsequence is found, or -1 if not found.
fn bytes_find(
    bytes: &[u8],
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (sub, start, end) = parse_bytes_sub_args("bytes.find", bytes.len(), args, heap, interns)?;

    let slice = &bytes[start..end];
    let result = if sub.is_empty() {
        // Empty subsequence: always found at start position
        Some(0)
    } else {
        find_subsequence(slice, &sub)
    };

    let idx = match result {
        Some(i) => i64::try_from(start + i).expect("index exceeds i64::MAX"),
        None => -1,
    };
    Ok(Value::Int(idx))
}

/// Finds the first occurrence of needle in haystack.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Implements Python's `bytes.index(sub[, start[, end]])` method.
///
/// Like find(), but raises ValueError if the subsequence is not found.
fn bytes_index(
    bytes: &[u8],
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (sub, start, end) = parse_bytes_sub_args("bytes.index", bytes.len(), args, heap, interns)?;

    let slice = &bytes[start..end];
    let result = if sub.is_empty() {
        // Empty subsequence: always found at start position
        Some(0)
    } else {
        find_subsequence(slice, &sub)
    };

    match result {
        Some(i) => {
            let idx = i64::try_from(start + i).expect("index exceeds i64::MAX");
            Ok(Value::Int(idx))
        }
        None => Err(ExcType::value_error_subsequence_not_found()),
    }
}

/// Implements Python's `bytes.startswith(prefix)` method.
///
/// Returns True if bytes starts with the specified prefix.
fn bytes_startswith(
    bytes: &[u8],
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let prefix = args.get_one_arg("bytes.startswith", heap)?;

    let prefix_bytes = extract_bytes_arg(&prefix, heap, interns)?;
    prefix.drop_with_heap(heap);

    let result = bytes.starts_with(&prefix_bytes);
    Ok(Value::Bool(result))
}

/// Implements Python's `bytes.endswith(suffix)` method.
///
/// Returns True if bytes ends with the specified suffix.
fn bytes_endswith(
    bytes: &[u8],
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let suffix = args.get_one_arg("bytes.endswith", heap)?;

    let suffix_bytes = extract_bytes_arg(&suffix, heap, interns)?;
    suffix.drop_with_heap(heap);

    let result = bytes.ends_with(&suffix_bytes);
    Ok(Value::Bool(result))
}

/// Extracts bytes from a Value (bytes or str).
fn extract_bytes_arg(value: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Vec<u8>> {
    match value {
        Value::InternBytes(id) => Ok(interns.get_bytes(*id).to_vec()),
        Value::InternString(id) => Ok(interns.get_str(*id).as_bytes().to_vec()),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Bytes(b) => Ok(b.as_slice().to_vec()),
            HeapData::Str(s) => Ok(s.as_str().as_bytes().to_vec()),
            _ => Err(ExcType::type_error("a bytes-like object is required")),
        },
        _ => Err(ExcType::type_error("a bytes-like object is required")),
    }
}

/// Parses arguments for bytes.find/count/index methods.
///
/// Returns (sub_bytes, start, end) where start and end are normalized indices.
fn parse_bytes_sub_args(
    method: &str,
    len: usize,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(Vec<u8>, usize, usize)> {
    let (pos, kwargs) = args.into_parts();
    if !kwargs.is_empty() {
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_no_kwargs(method));
    }

    let mut pos_iter = pos;
    let sub_value = pos_iter
        .next()
        .ok_or_else(|| ExcType::type_error_at_least(method, 1, 0))?;
    let start_value = pos_iter.next();
    let end_value = pos_iter.next();

    // Check no extra arguments
    if pos_iter.next().is_some() {
        for v in pos_iter {
            v.drop_with_heap(heap);
        }
        sub_value.drop_with_heap(heap);
        if let Some(v) = start_value {
            v.drop_with_heap(heap);
        }
        if let Some(v) = end_value {
            v.drop_with_heap(heap);
        }
        return Err(ExcType::type_error_at_most(method, 3, 4));
    }

    // Extract sub bytes
    let sub = match extract_bytes_arg(&sub_value, heap, interns) {
        Ok(b) => b,
        Err(e) => {
            sub_value.drop_with_heap(heap);
            if let Some(v) = start_value {
                v.drop_with_heap(heap);
            }
            if let Some(v) = end_value {
                v.drop_with_heap(heap);
            }
            return Err(e);
        }
    };
    sub_value.drop_with_heap(heap);

    // Extract start (default 0)
    let start = if let Some(v) = start_value {
        let result = v.as_int(heap);
        v.drop_with_heap(heap);
        match result {
            Ok(i) => normalize_bytes_index(i, len),
            Err(e) => {
                if let Some(ev) = end_value {
                    ev.drop_with_heap(heap);
                }
                return Err(e);
            }
        }
    } else {
        0
    };

    // Extract end (default len)
    let end = if let Some(v) = end_value {
        let result = v.as_int(heap);
        v.drop_with_heap(heap);
        normalize_bytes_index(result?, len)
    } else {
        len
    };

    Ok((sub, start, end))
}

/// Normalizes a Python-style bytes index to a valid index in range [0, len].
fn normalize_bytes_index(index: i64, len: usize) -> usize {
    if index < 0 {
        let abs_index = usize::try_from(-index).unwrap_or(usize::MAX);
        len.saturating_sub(abs_index)
    } else {
        usize::try_from(index).unwrap_or(len).min(len)
    }
}
