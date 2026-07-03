/// Python bytes type, wrapping a `Vec<u8>`.
///
/// This type provides Python bytes semantics with operations on ASCII bytes only.
/// Unlike str methods which operate on Unicode codepoints, bytes methods only
/// recognize ASCII characters (0-127) for case transformations and predicates.
///
/// # Implemented Methods
///
/// ## Encoding/Decoding
/// - `decode([encoding[, errors]])` - Decode to string (UTF-8 only)
/// - `hex([sep[, bytes_per_sep]])` - Return hex string representation
/// - `fromhex(string)` - Create bytes from hex string (classmethod)
///
/// ## Simple Transformations
/// - `lower()` - Convert ASCII uppercase to lowercase
/// - `upper()` - Convert ASCII lowercase to uppercase
/// - `capitalize()` - First byte uppercase, rest lowercase
/// - `title()` - Titlecase ASCII letters
/// - `swapcase()` - Swap ASCII case
///
/// ## Predicates
/// - `isalpha()` - All bytes are ASCII letters
/// - `isdigit()` - All bytes are ASCII digits
/// - `isalnum()` - All bytes are ASCII alphanumeric
/// - `isspace()` - All bytes are ASCII whitespace
/// - `islower()` - Has cased bytes, all lowercase
/// - `isupper()` - Has cased bytes, all uppercase
/// - `isascii()` - All bytes are ASCII (0-127)
/// - `istitle()` - Titlecased
///
/// ## Search Methods
/// - `count(sub[, start[, end]])` - Count non-overlapping occurrences
/// - `find(sub[, start[, end]])` - Find first occurrence (-1 if not found)
/// - `rfind(sub[, start[, end]])` - Find last occurrence (-1 if not found)
/// - `index(sub[, start[, end]])` - Find first occurrence (raises ValueError)
/// - `rindex(sub[, start[, end]])` - Find last occurrence (raises ValueError)
/// - `startswith(prefix[, start[, end]])` - Check if starts with prefix
/// - `endswith(suffix[, start[, end]])` - Check if ends with suffix
///
/// ## Strip/Trim Methods
/// - `strip([chars])` - Remove leading/trailing bytes
/// - `lstrip([chars])` - Remove leading bytes
/// - `rstrip([chars])` - Remove trailing bytes
/// - `removeprefix(prefix)` - Remove prefix if present
/// - `removesuffix(suffix)` - Remove suffix if present
///
/// ## Split Methods
/// - `split([sep[, maxsplit]])` - Split on separator
/// - `rsplit([sep[, maxsplit]])` - Split from right
/// - `splitlines([keepends])` - Split on line boundaries
/// - `partition(sep)` - Split into 3 parts at first sep
/// - `rpartition(sep)` - Split into 3 parts at last sep
///
/// ## Replace/Padding Methods
/// - `replace(old, new[, count])` - Replace occurrences
/// - `center(width[, fillbyte])` - Center with fill byte
/// - `ljust(width[, fillbyte])` - Left justify with fill byte
/// - `rjust(width[, fillbyte])` - Right justify with fill byte
/// - `zfill(width)` - Pad with zeros
///
/// ## Other Methods
/// - `join(iterable)` - Join bytes sequences
///
/// # Unimplemented Methods
/// - `expandtabs(tabsize=8)` - Tab expansion
/// - `translate(table[, delete])` - Character translation
/// - `maketrans(frm, to)` - Create translation table (staticmethod)
use std::{
    cell::Cell,
    cmp::Ordering,
    ffi::c_int,
    fmt::{self, Write},
    mem, ops, str,
};

use ahash::AHashSet;
use smallvec::smallvec;

use super::{MontyIter, PyTrait, Type};
use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    hash::{HashValue, hash_python_bytes},
    heap::{DropWithHeap, Heap, HeapData, HeapGuard, HeapId, HeapItem, HeapRead, heap_read_ref_as_field},
    intern::{StaticStrings, StringId},
    resource::{ResourceError, ResourceTracker, check_repeat_size, check_replace_size},
    types::{
        List,
        slice::{normalize_sequence_index, slice_collect_iterator},
    },
    value::{EitherStr, Value, eq_bytes},
};

// =============================================================================
// ASCII byte helper functions
// =============================================================================

/// Returns true if the byte is Python ASCII whitespace.
///
/// Python considers these bytes as whitespace: space, tab, newline, carriage return,
/// vertical tab (0x0b), and form feed (0x0c). Note: Rust's `is_ascii_whitespace()`
/// does not include vertical tab (0x0b).
#[inline]
fn is_py_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Gets the byte at a given index, handling negative indices.
///
/// Returns `None` if the index is out of bounds.
/// Negative indices count from the end: -1 is the last byte.
pub fn get_byte_at_index(bytes: &[u8], index: i64) -> Option<u8> {
    let len = i64::try_from(bytes.len()).ok()?;
    let normalized = if index < 0 { index + len } else { index };

    if normalized < 0 || normalized >= len {
        return None;
    }

    let idx = usize::try_from(normalized).ok()?;
    Some(bytes[idx])
}

/// Python bytes value stored on the heap.
///
/// Wraps a `Vec<u8>` and provides Python-compatible operations.
/// See the module-level documentation for implemented and unimplemented methods.
///
/// Carries an inline `cached_hash` field (skipped on serde) so a `Bytes` only
/// computes its Python hash once. See [`super::Str`] for the same pattern.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct Bytes(Vec<u8>, #[serde(skip)] Cell<Option<HashValue>>);

impl PartialEq for Bytes {
    /// Compares only the byte content — `cached_hash` is a pure optimisation.
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Bytes {
    /// Creates a new Bytes from a byte vector.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes, Cell::new(None))
    }

    /// Returns a reference to the inner byte slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Creates bytes from the `bytes()` constructor call.
    ///
    /// - `bytes()` with no args returns empty bytes
    /// - `bytes(int)` returns bytes of that length filled with zeros
    /// - `bytes(string)` encodes the string as UTF-8 (simplified, no encoding param)
    /// - `bytes(bytes)` returns a copy of the bytes
    ///
    /// Note: Full Python semantics for bytes() are more complex (encoding, errors params).
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let BytesInitArgs { source } = BytesInitArgs::from_args(args, vm)?;
        defer_drop!(source, vm);
        let new_data = match source {
            None => Vec::new(),
            Some(Value::Int(n)) => {
                if *n < 0 {
                    return Err(ExcType::value_error_negative_bytes_count());
                }
                let size = usize::try_from(*n).expect("bytes count validated non-negative");
                // Pre-check the requested size against resource limits before
                // touching the global allocator. Without this, `bytes(n)` for a
                // very large `n` would attempt the native allocation directly
                // and abort the host on failure rather than raising MemoryError.
                // Mirrors the guard already used by `bytes.ljust`/`zfill`/`*`.
                check_repeat_size(size, 1, vm.heap.tracker())?;
                vec![0u8; size]
            }
            Some(Value::InternString(string_id)) => {
                let s = vm.interns.get_str(*string_id);
                s.as_bytes().to_vec()
            }
            Some(Value::InternBytes(bytes_id)) => {
                let b = vm.interns.get_bytes(*bytes_id);
                b.to_vec()
            }
            Some(v @ Value::Ref(id)) => match vm.heap.get(*id) {
                HeapData::Str(s) => s.as_str().as_bytes().to_vec(),
                HeapData::Bytes(b) => b.as_slice().to_vec(),
                _ => return Err(ExcType::type_error_bytes_init(v.py_type(vm))),
            },
            Some(v) => return Err(ExcType::type_error_bytes_init(v.py_type(vm))),
        };
        let heap_id = vm.heap.allocate(HeapData::Bytes(Self::new(new_data)))?;
        Ok(Value::Ref(heap_id))
    }
}

/// Argument shape for `bytes(source=...)` — one optional pos-or-keyword arg
/// (`source` is the CPython kwarg name) interpreted as the type-specific
/// dispatch inside [`Bytes::init`].
#[derive(FromArgs)]
#[from_args(name = "bytes", style = c_named)]
struct BytesInitArgs {
    #[from_args(default)]
    source: Option<Value>,
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self::new(bytes.to_vec())
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}

impl ops::Deref for Bytes {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Bytes> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Bytes
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).0.len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            let b = self.get(vm.heap);
            let sliced_bytes = slice_collect_iterator(vm, slice, b.0.iter(), |b| *b)?;
            let heap_id = vm.heap.allocate(HeapData::Bytes(Bytes::new(sliced_bytes)))?;
            return Ok(Value::Ref(heap_id));
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(vm, Type::Bytes)?;

        // Use helper for byte indexing
        let b = self.get(vm.heap);
        let byte = get_byte_at_index(&b.0, index).ok_or_else(ExcType::bytes_index_error)?;
        Ok(Value::Int(i64::from(byte)))
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        // Heap bytes equal interned or heap bytes with the same content.
        Ok(eq_bytes(self.get(vm.heap).as_slice(), other, vm))
    }

    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        let b = self.get(vm.heap);
        if let Some(cached) = b.1.get() {
            return Ok(Some(cached));
        }
        // Delegates to the canonical helper used by both heap and intern paths;
        // an interned `b"foo"` and a heap `b"foo"` must hash identically for
        // dict lookup to work.
        let hash = hash_python_bytes(b.as_slice());
        b.1.set(Some(hash));
        Ok(Some(hash))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<Ordering>> {
        Ok(Some(self.get(vm.heap).0.cmp(&other.get(vm.heap).0)))
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).0.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        Ok(bytes_repr_fmt(&self.get(vm.heap).0, f)?)
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(vm);
            return Err(ExcType::attribute_error(Type::Bytes, attr.as_str(vm.interns)));
        };

        let field = heap_read_ref_as_field!(self, Bytes, 0);
        let bytes = field.as_slice(vm.heap);
        call_bytes_method_impl(&bytes, method, args, vm).map(CallResult::Value)
    }
}

impl HeapItem for Bytes {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.0.len()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No-op: bytes don't hold Value references
    }
}

/// Calls a bytes method on a byte slice by method name.
///
/// This is the entry point for bytes method calls from the VM on interned bytes.
/// Converts the `StringId` to `StaticStrings` and delegates to `call_bytes_method_impl`.
pub fn call_bytes_method(
    bytes: &[u8],
    method_id: StringId,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let Some(method) = StaticStrings::from_string_id(method_id) else {
        args.drop_with_heap(vm);
        return Err(ExcType::attribute_error(Type::Bytes, vm.interns.get_str(method_id)));
    };
    call_bytes_method_impl(&vm.heap.protect(bytes), method, args, vm)
}

/// Calls a bytes method on a byte slice.
///
/// This is the unified implementation for bytes method calls, used by both
/// heap-allocated `Bytes` (via `py_call_attr`) and interned bytes literals
/// (`Value::InternBytes`).
fn call_bytes_method_impl<'h>(
    bytes: &HeapRead<'h, [u8]>,
    method: StaticStrings,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    match method {
        // Decode method
        StaticStrings::Decode => bytes_decode(bytes, args, vm),
        // Simple transformations (no arguments)
        StaticStrings::Lower => {
            args.check_zero_args("bytes.lower", vm.heap)?;
            bytes_lower(bytes, vm)
        }
        StaticStrings::Upper => {
            args.check_zero_args("bytes.upper", vm.heap)?;
            bytes_upper(bytes, vm)
        }
        StaticStrings::Capitalize => {
            args.check_zero_args("bytes.capitalize", vm.heap)?;
            bytes_capitalize(bytes, vm)
        }
        StaticStrings::Title => {
            args.check_zero_args("bytes.title", vm.heap)?;
            bytes_title(bytes, vm)
        }
        StaticStrings::Swapcase => {
            args.check_zero_args("bytes.swapcase", vm.heap)?;
            bytes_swapcase(bytes, vm)
        }
        // Predicate methods (no arguments, return bool)
        StaticStrings::Isalpha => {
            args.check_zero_args("bytes.isalpha", vm.heap)?;
            Ok(Value::Bool(bytes_isalpha(bytes.get(vm.heap))))
        }
        StaticStrings::Isdigit => {
            args.check_zero_args("bytes.isdigit", vm.heap)?;
            Ok(Value::Bool(bytes_isdigit(bytes.get(vm.heap))))
        }
        StaticStrings::Isalnum => {
            args.check_zero_args("bytes.isalnum", vm.heap)?;
            Ok(Value::Bool(bytes_isalnum(bytes.get(vm.heap))))
        }
        StaticStrings::Isspace => {
            args.check_zero_args("bytes.isspace", vm.heap)?;
            Ok(Value::Bool(bytes_isspace(bytes.get(vm.heap))))
        }
        StaticStrings::Islower => {
            args.check_zero_args("bytes.islower", vm.heap)?;
            Ok(Value::Bool(bytes_islower(bytes.get(vm.heap))))
        }
        StaticStrings::Isupper => {
            args.check_zero_args("bytes.isupper", vm.heap)?;
            Ok(Value::Bool(bytes_isupper(bytes.get(vm.heap))))
        }
        StaticStrings::Isascii => {
            args.check_zero_args("bytes.isascii", vm.heap)?;
            Ok(Value::Bool(bytes.get(vm.heap).iter().all(|&b| b <= 127)))
        }
        StaticStrings::Istitle => {
            args.check_zero_args("bytes.istitle", vm.heap)?;
            Ok(Value::Bool(bytes_istitle(bytes.get(vm.heap))))
        }
        // Search methods
        StaticStrings::Count => bytes_count(bytes, args, vm),
        StaticStrings::Find => bytes_find(bytes, args, vm),
        StaticStrings::Rfind => bytes_rfind(bytes, args, vm),
        StaticStrings::Index => bytes_index(bytes, args, vm),
        StaticStrings::Rindex => bytes_rindex(bytes, args, vm),
        StaticStrings::Startswith => bytes_startswith(bytes, args, vm),
        StaticStrings::Endswith => bytes_endswith(bytes, args, vm),
        // Strip/trim methods
        StaticStrings::Strip => bytes_strip(bytes, args, vm),
        StaticStrings::Lstrip => bytes_lstrip(bytes, args, vm),
        StaticStrings::Rstrip => bytes_rstrip(bytes, args, vm),
        StaticStrings::Removeprefix => bytes_removeprefix(bytes, args, vm),
        StaticStrings::Removesuffix => bytes_removesuffix(bytes, args, vm),
        // Split methods
        StaticStrings::Split => bytes_split(bytes, args, vm),
        StaticStrings::Rsplit => bytes_rsplit(bytes, args, vm),
        StaticStrings::Splitlines => bytes_splitlines(bytes, args, vm),
        StaticStrings::Partition => bytes_partition(bytes, args, vm),
        StaticStrings::Rpartition => bytes_rpartition(bytes, args, vm),
        // Replace/padding methods
        StaticStrings::Replace => bytes_replace(bytes, args, vm),
        StaticStrings::Center => bytes_center(bytes, args, vm),
        StaticStrings::Ljust => bytes_ljust(bytes, args, vm),
        StaticStrings::Rjust => bytes_rjust(bytes, args, vm),
        StaticStrings::Zfill => bytes_zfill(bytes, args, vm),
        // Join method
        StaticStrings::Join => {
            let iterable = args.get_one_arg("bytes.join", vm.heap)?;
            bytes_join(bytes, iterable, vm)
        }
        // Hex method
        StaticStrings::Hex => bytes_hex(bytes, args, vm),
        // fromhex is a classmethod but also accessible on instances
        StaticStrings::Fromhex => bytes_fromhex(args, vm),
        _ => {
            args.drop_with_heap(vm.heap);
            Err(ExcType::attribute_error(Type::Bytes, method.into()))
        }
    }
}

/// Writes a CPython-compatible repr string for bytes to a formatter.
///
/// Format: `b'...'` or `b"..."` depending on content.
/// - Uses single quotes by default
/// - Switches to double quotes if bytes contain `'` but not `"`
/// - Escapes: `\\`, `\t`, `\n`, `\r`, `\xNN` for non-printable bytes
pub fn bytes_repr_fmt(bytes: &[u8], f: &mut impl Write) -> fmt::Result {
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
fn bytes_decode<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let BytesDecodeArgs { encoding, errors } = BytesDecodeArgs::from_args(args, vm)?;
    // `errors` is accepted for parity but ignored — UTF-8 decoding of valid
    // bytes has nothing to handle, and `lookup_error_unknown_error_handler`
    // would be the next layer once non-UTF-8 codecs land. The guard still
    // drops its heap reference.
    defer_drop!(errors, vm);
    defer_drop!(encoding, vm);
    let encoding = encoding.as_ref().map_or("utf-8", |e| e.as_str(vm));

    // Only support UTF-8 family
    if !(encoding.eq_ignore_ascii_case("utf-8")
        || encoding.eq_ignore_ascii_case("utf8")
        || encoding.eq_ignore_ascii_case("utf_8"))
    {
        return Err(ExcType::lookup_error_unknown_encoding(encoding));
    }

    // Decode as UTF-8
    match str::from_utf8(bytes.get(vm.heap)) {
        Ok(s) => Ok(super::str::allocate_string(s, vm.heap)?),
        Err(_) => Err(ExcType::unicode_decode_error_invalid_utf8()),
    }
}

/// Argument shape for `bytes.decode(encoding='utf-8', errors='strict')`.
///
/// `bad_arg_named` opts in to CPython's `_PyArg_BadArgument` named wording
/// (`decode() argument 'encoding' must be str, not <type>`) so wrong-type
/// errors match the C implementation. Both fields default to absent;
/// CPython rejects explicit `None` here with the bad-arg error, which falls
/// out naturally because `Option<StrArg>::from_value` delegates to
/// `StrArg::from_value` and rejects `Value::None`.
#[derive(FromArgs)]
#[from_args(name = "decode", at_most_total, bad_arg_named)]
struct BytesDecodeArgs {
    #[from_args(default)]
    encoding: Option<StrArg>,
    #[from_args(default)]
    errors: Option<StrArg>,
}

/// Implements Python's `bytes.count(sub[, start[, end]])` method.
///
/// Returns the number of non-overlapping occurrences of the subsequence.
fn bytes_count<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (sub, start, end) = parse_bytes_sub_args("bytes.count", len, args, vm)?;

    let slice = &bytes.get(vm.heap)[start..end];
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
fn bytes_find<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (sub, start, end) = parse_bytes_sub_args("bytes.find", len, args, vm)?;

    let slice = &bytes.get(vm.heap)[start..end];
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
fn bytes_index<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (sub, start, end) = parse_bytes_sub_args("bytes.index", len, args, vm)?;

    let slice = &bytes.get(vm.heap)[start..end];
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

/// Implements Python's `bytes.startswith(prefix[, start[, end]])` method.
///
/// Returns True if bytes starts with the specified prefix.
/// Accepts bytes or a tuple of bytes as prefix. If a tuple is given, returns True
/// if any of the prefixes match.
fn bytes_startswith<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (prefix_arg, start, end) = parse_bytes_prefix_suffix_args("bytes.startswith", len, args, vm)?;

    let slice = &bytes.get(vm.heap)[start..end];
    let result = match prefix_arg {
        PrefixSuffixArg::Single(prefix_bytes) => slice.starts_with(&prefix_bytes),
        PrefixSuffixArg::Multiple(prefixes) => prefixes.iter().any(|p| slice.starts_with(p)),
    };
    Ok(Value::Bool(result))
}

/// Implements Python's `bytes.endswith(suffix[, start[, end]])` method.
///
/// Returns True if bytes ends with the specified suffix.
/// Accepts bytes or a tuple of bytes as suffix. If a tuple is given, returns True
/// if any of the suffixes match.
fn bytes_endswith<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (suffix_arg, start, end) = parse_bytes_prefix_suffix_args("bytes.endswith", len, args, vm)?;

    let slice = &bytes.get(vm.heap)[start..end];
    let result = match suffix_arg {
        PrefixSuffixArg::Single(suffix_bytes) => slice.ends_with(&suffix_bytes),
        PrefixSuffixArg::Multiple(suffixes) => suffixes.iter().any(|s| slice.ends_with(s)),
    };
    Ok(Value::Bool(result))
}

/// Argument type for prefix/suffix matching methods.
///
/// Represents either a single bytes value or a tuple of bytes values
/// for matching in startswith/endswith.
enum PrefixSuffixArg {
    /// A single bytes value to match
    Single(Vec<u8>),
    /// Multiple bytes values to match (from a tuple)
    Multiple(Vec<Vec<u8>>),
}

/// Parses arguments for bytes.startswith/endswith methods.
///
/// Returns (prefix/suffix_arg, start, end) where start and end are normalized indices.
/// The prefix/suffix_arg can be a single bytes value or a tuple of bytes values.
/// Guarantees `start <= end` to prevent slice panics.
fn parse_bytes_prefix_suffix_args(
    method: &str,
    len: usize,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(PrefixSuffixArg, usize, usize)> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);

    let (prefix, start, end) = match pos.as_slice() {
        [prefix_value] => {
            let prefix = extract_bytes_for_prefix_suffix(prefix_value, method, vm)?;
            (prefix, 0, len)
        }
        [prefix_value, start_value] => {
            let prefix = extract_bytes_for_prefix_suffix(prefix_value, method, vm)?;
            let start = normalize_sequence_index(start_value.as_int(vm)?, len);
            (prefix, start, len)
        }
        [prefix_value, start_value, end_value] => {
            let prefix = extract_bytes_for_prefix_suffix(prefix_value, method, vm)?;
            let start = normalize_sequence_index(start_value.as_int(vm)?, len);
            let end = normalize_sequence_index(end_value.as_int(vm)?, len);
            (prefix, start, end)
        }
        [] => return Err(ExcType::type_error_at_least(method, 1, 0)),
        _ => return Err(ExcType::type_error_at_most(method, 3, pos.len())),
    };

    // Ensure start <= end to prevent slice panics
    Ok((prefix, start, end.max(start)))
}

/// Extracts bytes (or tuple of bytes) for startswith/endswith methods.
///
/// Returns `PrefixSuffixArg::Single` for a single bytes value, or
/// `PrefixSuffixArg::Multiple` for a tuple of bytes values.
fn extract_bytes_for_prefix_suffix(
    value: &Value,
    method: &str,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<PrefixSuffixArg> {
    // Extract the method name (e.g., "startswith" from "bytes.startswith")
    let method_name = method.strip_prefix("bytes.").unwrap_or(method);

    match value {
        Value::InternBytes(id) => Ok(PrefixSuffixArg::Single(vm.interns.get_bytes(*id).to_vec())),
        Value::InternString(_) => Err(ExcType::type_error(format!(
            "{method_name} first arg must be bytes or a tuple of bytes, not str"
        ))),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(b) => Ok(PrefixSuffixArg::Single(b.as_slice().to_vec())),
            HeapData::Str(_) => Err(ExcType::type_error(format!(
                "{method_name} first arg must be bytes or a tuple of bytes, not str"
            ))),
            HeapData::Tuple(tuple) => {
                // Extract each element as bytes
                let items = tuple.as_slice();
                let mut prefixes = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    if let Ok(b) = extract_single_bytes_for_prefix_suffix(item, vm) {
                        prefixes.push(b);
                    } else {
                        let item_type = item.py_type(vm);
                        return Err(ExcType::type_error(format!(
                            "{method_name} first arg must be bytes or a tuple of bytes, \
                             not tuple containing {item_type} at index {i}"
                        )));
                    }
                }
                Ok(PrefixSuffixArg::Multiple(prefixes))
            }
            _ => Err(ExcType::type_error(format!(
                "{method_name} first arg must be bytes or a tuple of bytes, not {}",
                value.py_type(vm)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "{method_name} first arg must be bytes or a tuple of bytes, not {}",
            value.py_type(vm)
        ))),
    }
}

/// Extracts a single bytes value for tuple element in startswith/endswith.
fn extract_single_bytes_for_prefix_suffix(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Vec<u8>> {
    match value {
        Value::InternBytes(id) => Ok(vm.interns.get_bytes(*id).to_vec()),
        Value::InternString(_) => Err(ExcType::type_error("expected bytes, not str")),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(b) => Ok(b.as_slice().to_vec()),
            _ => Err(ExcType::type_error("expected bytes")),
        },
        _ => Err(ExcType::type_error("expected bytes")),
    }
}

/// Extracts bytes from a Value (bytes only, NOT str - matches CPython behavior).
///
/// CPython raises `TypeError: a bytes-like object is required, not 'str'` when
/// a str is passed to bytes methods like find, count, index, startswith, endswith.
fn extract_bytes_only<'a>(value: &Value, vm: &'a VM<'_, impl ResourceTracker>) -> RunResult<&'a [u8]> {
    match value {
        Value::InternBytes(id) => Ok(vm.interns.get_bytes(*id)),
        Value::InternString(_) => Err(ExcType::type_error("a bytes-like object is required, not 'str'")),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(b) => Ok(b.as_slice()),
            HeapData::Str(_) => Err(ExcType::type_error("a bytes-like object is required, not 'str'")),
            _ => Err(ExcType::type_error("a bytes-like object is required")),
        },
        _ => Err(ExcType::type_error("a bytes-like object is required")),
    }
}

/// Parses arguments for bytes.find/count/index methods.
///
/// Returns (sub_bytes, start, end) where start and end are normalized indices.
/// Guarantees `start <= end` to prevent slice panics.
fn parse_bytes_sub_args(
    method: &str,
    len: usize,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(Vec<u8>, usize, usize)> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);

    let (sub, start, end) = match pos.as_slice() {
        [sub_value] => {
            let sub = extract_bytes_only(sub_value, vm)?;
            (sub, 0, len)
        }
        [sub_value, start_value] => {
            let sub = extract_bytes_only(sub_value, vm)?;
            let start = normalize_sequence_index(start_value.as_int(vm)?, len);
            (sub, start, len)
        }
        [sub_value, start_value, end_value] => {
            let sub = extract_bytes_only(sub_value, vm)?;
            let start = normalize_sequence_index(start_value.as_int(vm)?, len);
            let end = normalize_sequence_index(end_value.as_int(vm)?, len);
            (sub, start, end)
        }
        [] => return Err(ExcType::type_error_at_least(method, 1, 0)),
        _ => return Err(ExcType::type_error_at_most(method, 3, pos.len())),
    };

    // Ensure start <= end to prevent slice panics (Python treats start > end as empty slice)
    Ok((sub.to_owned(), start, end.max(start)))
}

// =============================================================================
// Simple transformations (no arguments)
// =============================================================================

/// Implements Python's `bytes.lower()` method.
///
/// Returns a copy of the bytes with all ASCII uppercase characters converted to lowercase.
fn bytes_lower<'h>(bytes: &HeapRead<'h, [u8]>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let result: Vec<u8> = bytes.get(vm.heap).iter().map(|&b| b.to_ascii_lowercase()).collect();
    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.upper()` method.
///
/// Returns a copy of the bytes with all ASCII lowercase characters converted to uppercase.
fn bytes_upper<'h>(bytes: &HeapRead<'h, [u8]>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let result: Vec<u8> = bytes.get(vm.heap).iter().map(|&b| b.to_ascii_uppercase()).collect();
    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.capitalize()` method.
///
/// Returns a copy of the bytes with the first byte capitalized (if ASCII) and
/// the rest lowercased.
fn bytes_capitalize<'h>(bytes: &HeapRead<'h, [u8]>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let bytes = bytes.get(vm.heap);
    let mut result = Vec::with_capacity(bytes.len());
    if let Some((&first, rest)) = bytes.split_first() {
        result.push(first.to_ascii_uppercase());
        for &b in rest {
            result.push(b.to_ascii_lowercase());
        }
    }
    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.title()` method.
///
/// Returns a titlecased version of the bytes where words start with an uppercase
/// ASCII character and the remaining characters are lowercase.
fn bytes_title<'h>(bytes: &HeapRead<'h, [u8]>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let bytes = bytes.get(vm.heap);
    let mut result = Vec::with_capacity(bytes.len());
    let mut prev_is_cased = false;

    for &b in bytes {
        if prev_is_cased {
            result.push(b.to_ascii_lowercase());
        } else {
            result.push(b.to_ascii_uppercase());
        }
        prev_is_cased = b.is_ascii_alphabetic();
    }

    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.swapcase()` method.
///
/// Returns a copy of the bytes with ASCII uppercase characters converted to
/// lowercase and vice versa.
fn bytes_swapcase<'h>(bytes: &HeapRead<'h, [u8]>, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let result: Vec<u8> = bytes
        .get(vm.heap)
        .iter()
        .map(|&b| {
            if b.is_ascii_uppercase() {
                b.to_ascii_lowercase()
            } else if b.is_ascii_lowercase() {
                b.to_ascii_uppercase()
            } else {
                b
            }
        })
        .collect();
    allocate_bytes(result, vm.heap)
}

// =============================================================================
// Predicate methods (no arguments, return bool)
// =============================================================================

/// Implements Python's `bytes.isalpha()` method.
///
/// Returns True if all bytes in the bytes are ASCII letters and there is at least one byte.
fn bytes_isalpha(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|&b| b.is_ascii_alphabetic())
}

/// Implements Python's `bytes.isdigit()` method.
///
/// Returns True if all bytes in the bytes are ASCII digits and there is at least one byte.
fn bytes_isdigit(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|&b| b.is_ascii_digit())
}

/// Implements Python's `bytes.isalnum()` method.
///
/// Returns True if all bytes in the bytes are ASCII alphanumeric and there is at least one byte.
fn bytes_isalnum(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|&b| b.is_ascii_alphanumeric())
}

/// Implements Python's `bytes.isspace()` method.
///
/// Returns True if all bytes in the bytes are ASCII whitespace and there is at least one byte.
fn bytes_isspace(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|&b| is_py_whitespace(b))
}

/// Implements Python's `bytes.islower()` method.
///
/// Returns True if all cased bytes are lowercase and there is at least one cased byte.
fn bytes_islower(bytes: &[u8]) -> bool {
    let mut has_cased = false;
    for &b in bytes {
        if b.is_ascii_uppercase() {
            return false;
        }
        if b.is_ascii_lowercase() {
            has_cased = true;
        }
    }
    has_cased
}

/// Implements Python's `bytes.isupper()` method.
///
/// Returns True if all cased bytes are uppercase and there is at least one cased byte.
fn bytes_isupper(bytes: &[u8]) -> bool {
    let mut has_cased = false;
    for &b in bytes {
        if b.is_ascii_lowercase() {
            return false;
        }
        if b.is_ascii_uppercase() {
            has_cased = true;
        }
    }
    has_cased
}

/// Implements Python's `bytes.istitle()` method.
///
/// Returns True if the bytes are titlecased: uppercase characters follow
/// uncased characters and lowercase characters follow cased characters.
fn bytes_istitle(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    let mut prev_cased = false;
    let mut has_cased = false;

    for &b in bytes {
        if b.is_ascii_uppercase() {
            if prev_cased {
                return false;
            }
            prev_cased = true;
            has_cased = true;
        } else if b.is_ascii_lowercase() {
            if !prev_cased {
                return false;
            }
            prev_cased = true;
            has_cased = true;
        } else {
            prev_cased = false;
        }
    }

    has_cased
}

// =============================================================================
// Search methods
// =============================================================================

/// Implements Python's `bytes.rfind(sub[, start[, end]])` method.
///
/// Returns the highest index where the subsequence is found, or -1 if not found.
fn bytes_rfind<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (sub, start, end) = parse_bytes_sub_args("bytes.rfind", len, args, vm)?;

    let slice = &bytes.get(vm.heap)[start..end];
    let result = if sub.is_empty() {
        // Empty subsequence: always found at end position
        Some(slice.len())
    } else {
        rfind_subsequence(slice, &sub)
    };

    let idx = match result {
        Some(i) => i64::try_from(start + i).expect("index exceeds i64::MAX"),
        None => -1,
    };
    Ok(Value::Int(idx))
}

/// Finds the last occurrence of needle in haystack.
fn rfind_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).rposition(|window| window == needle)
}

/// Implements Python's `bytes.rindex(sub[, start[, end]])` method.
///
/// Like rfind(), but raises ValueError if the subsequence is not found.
fn bytes_rindex<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (sub, start, end) = parse_bytes_sub_args("bytes.rindex", len, args, vm)?;

    let slice = &bytes.get(vm.heap)[start..end];
    let result = if sub.is_empty() {
        Some(slice.len())
    } else {
        rfind_subsequence(slice, &sub)
    };

    match result {
        Some(i) => {
            let idx = i64::try_from(start + i).expect("index exceeds i64::MAX");
            Ok(Value::Int(idx))
        }
        None => Err(ExcType::value_error_subsequence_not_found()),
    }
}

// =============================================================================
// Strip/trim methods
// =============================================================================

/// Implements Python's `bytes.strip([chars])` method.
///
/// Returns a copy of the bytes with leading and trailing bytes removed.
/// If chars is not specified, ASCII whitespace bytes are removed.
fn bytes_strip<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let value = args.get_zero_one_arg("bytes.strip", vm.heap)?;
    defer_drop!(value, vm);
    let result = match value {
        None | Some(Value::None) => bytes_strip_whitespace_both(bytes.get(vm.heap)),
        Some(v) => bytes_strip_both(bytes.get(vm.heap), extract_bytes_only(v, vm)?),
    };
    allocate_bytes(result.to_vec(), vm.heap)
}

/// Implements Python's `bytes.lstrip([chars])` method.
///
/// Returns a copy of the bytes with leading bytes removed.
fn bytes_lstrip<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let value = args.get_zero_one_arg("bytes.lstrip", vm.heap)?;
    defer_drop!(value, vm);
    let result = match value {
        None | Some(Value::None) => bytes_strip_whitespace_start(bytes.get(vm.heap)),
        Some(v) => bytes_strip_start(bytes.get(vm.heap), extract_bytes_only(v, vm)?),
    };
    allocate_bytes(result.to_vec(), vm.heap)
}

/// Implements Python's `bytes.rstrip([chars])` method.
///
/// Returns a copy of the bytes with trailing bytes removed.
fn bytes_rstrip<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let value = args.get_zero_one_arg("bytes.rstrip", vm.heap)?;
    defer_drop!(value, vm);
    let result = match value {
        None | Some(Value::None) => bytes_strip_whitespace_end(bytes.get(vm.heap)),
        Some(v) => bytes_strip_end(bytes.get(vm.heap), extract_bytes_only(v, vm)?),
    };
    allocate_bytes(result.to_vec(), vm.heap)
}

/// Strips bytes in `chars` from both ends of the byte slice.
fn bytes_strip_both<'a>(bytes: &'a [u8], chars: &[u8]) -> &'a [u8] {
    let start = bytes.iter().position(|b| !chars.contains(b)).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !chars.contains(b))
        .map_or(start, |pos| pos + 1);
    &bytes[start..end]
}

/// Strips bytes in `chars` from the start of the byte slice.
fn bytes_strip_start<'a>(bytes: &'a [u8], chars: &[u8]) -> &'a [u8] {
    let start = bytes.iter().position(|b| !chars.contains(b)).unwrap_or(bytes.len());
    &bytes[start..]
}

/// Strips bytes in `chars` from the end of the byte slice.
fn bytes_strip_end<'a>(bytes: &'a [u8], chars: &[u8]) -> &'a [u8] {
    let end = bytes.iter().rposition(|b| !chars.contains(b)).map_or(0, |pos| pos + 1);
    &bytes[..end]
}

/// Strips ASCII whitespace from both ends of the byte slice.
fn bytes_strip_whitespace_both(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !is_py_whitespace(*b)).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !is_py_whitespace(*b))
        .map_or(start, |pos| pos + 1);
    &bytes[start..end]
}

/// Strips ASCII whitespace from the start of the byte slice.
fn bytes_strip_whitespace_start(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !is_py_whitespace(*b)).unwrap_or(bytes.len());
    &bytes[start..]
}

/// Strips ASCII whitespace from the end of the byte slice.
fn bytes_strip_whitespace_end(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|b| !is_py_whitespace(*b))
        .map_or(0, |pos| pos + 1);
    &bytes[..end]
}

/// Implements Python's `bytes.removeprefix(prefix)` method.
///
/// If the bytes start with the prefix, return bytes[len(prefix):].
/// Otherwise, return a copy of the original bytes.
fn bytes_removeprefix<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let prefix_value = args.get_one_arg("bytes.removeprefix", vm.heap)?;
    defer_drop!(prefix_value, vm);
    let prefix = extract_bytes_only(prefix_value, vm)?;

    let bytes = bytes.get(vm.heap);
    let result = if bytes.starts_with(prefix) {
        bytes[prefix.len()..].to_vec()
    } else {
        bytes.to_vec()
    };
    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.removesuffix(suffix)` method.
///
/// If the bytes end with the suffix, return bytes[:-len(suffix)].
/// Otherwise, return a copy of the original bytes.
fn bytes_removesuffix<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let suffix_value = args.get_one_arg("bytes.removesuffix", vm.heap)?;
    defer_drop!(suffix_value, vm);
    let suffix = extract_bytes_only(suffix_value, vm)?;

    let bytes = bytes.get(vm.heap);
    let result = if bytes.ends_with(suffix) && !suffix.is_empty() {
        bytes[..bytes.len() - suffix.len()].to_vec()
    } else {
        bytes.to_vec()
    };
    allocate_bytes(result, vm.heap)
}

// =============================================================================
// Split methods
// =============================================================================

/// Implements Python's `bytes.split([sep[, maxsplit]])` method.
///
/// Returns a list of the bytes split by the separator.
fn bytes_split<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let BytesSplitArgs { sep, maxsplit } = BytesSplitArgs::from_args(args, vm)?;
    let (sep, maxsplit) = coerce_bytes_split_args(sep, maxsplit, vm)?;

    let bytes = bytes.get(vm.heap);
    let parts: Vec<&[u8]> = match &sep {
        Some(sep) => {
            if sep.is_empty() {
                return Err(ExcType::value_error_empty_separator());
            }
            if maxsplit < 0 {
                bytes_split_by_seq(bytes, sep)
            } else {
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                bytes_splitn_by_seq(bytes, sep, max + 1)
            }
        }
        None => {
            if maxsplit < 0 {
                bytes_split_whitespace(bytes)
            } else {
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                bytes_splitn_whitespace(bytes, max)
            }
        }
    };

    let mut list_items = Vec::with_capacity(parts.len());
    for part in parts {
        vm.heap.check_time()?;
        list_items.push(allocate_bytes(part.to_vec(), vm.heap)?);
    }

    let list = List::new(list_items);
    let heap_id = vm.heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `bytes.rsplit([sep[, maxsplit]])` method.
///
/// Returns a list of the bytes split by the separator, splitting from the right.
fn bytes_rsplit<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let BytesRsplitArgs { sep, maxsplit } = BytesRsplitArgs::from_args(args, vm)?;
    let (sep, maxsplit) = coerce_bytes_split_args(sep, maxsplit, vm)?;

    let bytes = bytes.get(vm.heap);
    let parts: Vec<&[u8]> = match &sep {
        Some(sep) => {
            if sep.is_empty() {
                return Err(ExcType::value_error_empty_separator());
            }
            if maxsplit < 0 {
                bytes_split_by_seq(bytes, sep)
            } else {
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                bytes_rsplitn_by_seq(bytes, sep, max + 1)
            }
        }
        None => {
            if maxsplit < 0 {
                bytes_split_whitespace(bytes)
            } else {
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                bytes_rsplitn_whitespace(bytes, max)
            }
        }
    };

    let mut list_items = Vec::with_capacity(parts.len());
    for part in parts {
        vm.heap.check_time()?;
        list_items.push(allocate_bytes(part.to_vec(), vm.heap)?);
    }

    let list = List::new(list_items);
    let heap_id = vm.heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Coerces extracted `sep` / `maxsplit` `Value`s into the runtime shape used
/// by `bytes.split` / `bytes.rsplit`.
///
/// `sep = None` is the documented "no separator" sentinel (split on
/// runs of whitespace); any other value must be a bytes-like via
/// `extract_bytes_only`. `maxsplit` is read as an `i64`. Both arguments are
/// dropped on every path so refcounts stay balanced.
fn coerce_bytes_split_args(
    sep: Value,
    maxsplit: Value,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(Option<Vec<u8>>, i64)> {
    defer_drop!(sep, vm);
    defer_drop!(maxsplit, vm);
    let sep = match sep {
        Value::None => None,
        _ => Some(extract_bytes_only(sep, vm)?.to_owned()),
    };
    let maxsplit_int = maxsplit.as_int(vm)?;
    Ok((sep, maxsplit_int))
}

/// Argument shape for `bytes.split(sep=None, maxsplit=-1)`.
#[derive(FromArgs)]
#[from_args(name = "split")]
struct BytesSplitArgs {
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::Int(-1))]
    maxsplit: Value,
}

/// Argument shape for `bytes.rsplit(sep=None, maxsplit=-1)`.
#[derive(FromArgs)]
#[from_args(name = "rsplit")]
struct BytesRsplitArgs {
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::Int(-1))]
    maxsplit: Value,
}

/// Splits bytes by a separator sequence.
fn bytes_split_by_seq<'a>(bytes: &'a [u8], sep: &[u8]) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut start = 0;

    while let Some(pos) = find_subsequence(&bytes[start..], sep) {
        parts.push(&bytes[start..start + pos]);
        start = start + pos + sep.len();
    }
    parts.push(&bytes[start..]);

    parts
}

/// Splits bytes by a separator sequence, returning at most n parts.
fn bytes_splitn_by_seq<'a>(bytes: &'a [u8], sep: &[u8], n: usize) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut count = 0;

    while count + 1 < n {
        if let Some(pos) = find_subsequence(&bytes[start..], sep) {
            parts.push(&bytes[start..start + pos]);
            start = start + pos + sep.len();
            count += 1;
        } else {
            break;
        }
    }
    parts.push(&bytes[start..]);

    parts
}

/// Splits bytes by a separator sequence from the right, returning at most n parts.
fn bytes_rsplitn_by_seq<'a>(bytes: &'a [u8], sep: &[u8], n: usize) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut end = bytes.len();
    let mut count = 0;

    while count + 1 < n {
        if let Some(pos) = rfind_subsequence(&bytes[..end], sep) {
            parts.push(&bytes[pos + sep.len()..end]);
            end = pos;
            count += 1;
        } else {
            break;
        }
    }
    parts.push(&bytes[..end]);
    parts.reverse();

    parts
}

/// Splits bytes by ASCII whitespace, filtering empty parts.
fn bytes_split_whitespace(bytes: &[u8]) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut start = None;

    for (i, &b) in bytes.iter().enumerate() {
        if is_py_whitespace(b) {
            if let Some(s) = start {
                parts.push(&bytes[s..i]);
                start = None;
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }

    if let Some(s) = start {
        parts.push(&bytes[s..]);
    }

    parts
}

/// Splits bytes by ASCII whitespace, returning at most maxsplit+1 parts.
fn bytes_splitn_whitespace(bytes: &[u8], maxsplit: usize) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut start = None;
    let mut count = 0;

    let trimmed = bytes_strip_whitespace_start(bytes);
    let offset = bytes.len() - trimmed.len();

    for (i, &b) in trimmed.iter().enumerate() {
        if is_py_whitespace(b) {
            if let Some(s) = start
                && count < maxsplit
            {
                parts.push(&bytes[offset + s..offset + i]);
                count += 1;
                start = None;
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }

    if let Some(s) = start {
        parts.push(&bytes[offset + s..]);
    }

    parts
}

/// Splits bytes by ASCII whitespace from the right, returning at most maxsplit+1 parts.
fn bytes_rsplitn_whitespace(bytes: &[u8], maxsplit: usize) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut end = None;
    let mut count = 0;

    let trimmed = bytes_strip_whitespace_end(bytes);

    for i in (0..trimmed.len()).rev() {
        let b = trimmed[i];
        if is_py_whitespace(b) {
            if let Some(e) = end
                && count < maxsplit
            {
                parts.push(&trimmed[i + 1..e]);
                count += 1;
                end = None;
            }
        } else if end.is_none() {
            end = Some(i + 1);
        }
    }

    if let Some(e) = end {
        parts.push(&trimmed[..e]);
    }

    parts.reverse();
    parts
}

/// Implements Python's `bytes.splitlines([keepends])` method.
///
/// Returns a list of the lines in the bytes, breaking at line boundaries.
fn bytes_splitlines<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let keepends = parse_bytes_splitlines_args(args, vm)?;

    let lines = Vec::new();
    let mut lines_guard = HeapGuard::new(lines, vm);
    let (lines, vm) = lines_guard.as_parts_mut();
    let mut start = 0;
    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    while start < len {
        vm.heap.check_time()?;

        let mut end = start;
        let mut line_end = start;

        while end < len {
            match bytes[end] {
                b'\n' => {
                    line_end = end;
                    end += 1;
                    break;
                }
                b'\r' => {
                    line_end = end;
                    end += 1;
                    if end < len && bytes[end] == b'\n' {
                        end += 1;
                    }
                    break;
                }
                _ => {
                    end += 1;
                    line_end = end;
                }
            }
        }

        let line = if keepends {
            &bytes[start..end]
        } else {
            &bytes[start..line_end]
        };
        lines.push(allocate_bytes(line.to_vec(), vm.heap)?);
        start = end;
    }

    let (lines, vm) = lines_guard.into_parts();
    let list = List::new(lines);
    let heap_id = vm.heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Parses arguments for bytes.splitlines method.
fn parse_bytes_splitlines_args(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<bool> {
    let BytesSplitlinesArgs { keepends } = BytesSplitlinesArgs::from_args(args, vm)?;
    let result = match keepends {
        None => false,
        Some(v) => {
            let r = v.py_bool(vm);
            v.drop_with_heap(vm.heap);
            r
        }
    };
    Ok(result)
}

/// Argument shape for `bytes.splitlines(keepends=False)`. CPython evaluates
/// `keepends` for truthiness rather than strict-typing, so the field stays as
/// a raw `Value` for `py_bool` to inspect.
#[derive(FromArgs)]
#[from_args(name = "splitlines")]
struct BytesSplitlinesArgs {
    #[from_args(default)]
    keepends: Option<Value>,
}

/// Implements Python's `bytes.partition(sep)` method.
///
/// Splits the bytes at the first occurrence of sep, and returns a 3-tuple.
fn bytes_partition<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let sep_value = args.get_one_arg("bytes.partition", vm.heap)?;
    defer_drop!(sep_value, vm);
    let sep = extract_bytes_only(sep_value, vm)?;

    if sep.is_empty() {
        return Err(ExcType::value_error_empty_separator());
    }

    let bytes = bytes.get(vm.heap);
    let (before, sep_found, after) = match find_subsequence(bytes, sep) {
        Some(pos) => (bytes[..pos].to_vec(), sep.to_vec(), bytes[pos + sep.len()..].to_vec()),
        None => (bytes.to_vec(), Vec::new(), Vec::new()),
    };

    let before_val = allocate_bytes(before, vm.heap)?;
    let sep_val = allocate_bytes(sep_found, vm.heap)?;
    let after_val = allocate_bytes(after, vm.heap)?;

    Ok(super::allocate_tuple(
        smallvec![before_val, sep_val, after_val],
        vm.heap,
    )?)
}

/// Implements Python's `bytes.rpartition(sep)` method.
///
/// Splits the bytes at the last occurrence of sep, and returns a 3-tuple.
fn bytes_rpartition<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let sep_value = args.get_one_arg("bytes.rpartition", vm.heap)?;
    defer_drop!(sep_value, vm);
    let sep = extract_bytes_only(sep_value, vm)?;

    if sep.is_empty() {
        return Err(ExcType::value_error_empty_separator());
    }

    let bytes = bytes.get(vm.heap);
    let (before, sep_found, after) = match rfind_subsequence(bytes, sep) {
        Some(pos) => (bytes[..pos].to_vec(), sep.to_vec(), bytes[pos + sep.len()..].to_vec()),
        None => (Vec::new(), Vec::new(), bytes.to_vec()),
    };

    let before_val = allocate_bytes(before, vm.heap)?;
    let sep_val = allocate_bytes(sep_found, vm.heap)?;
    let after_val = allocate_bytes(after, vm.heap)?;

    Ok(super::allocate_tuple(
        smallvec![before_val, sep_val, after_val],
        vm.heap,
    )?)
}

// =============================================================================
// Replace/padding methods
// =============================================================================

/// Implements Python's `bytes.replace(old, new[, count])` method.
///
/// Returns a copy with all occurrences of old replaced by new.
fn bytes_replace<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let (old, new, count) = parse_bytes_replace_args("bytes.replace", args, vm)?;

    let bytes = bytes.get(vm.heap);

    check_replace_size(bytes.len(), old.len(), new.len(), count, vm.heap.tracker())?;

    let result = if count < 0 {
        bytes_replace_all(bytes, &old, &new, vm.heap)?
    } else {
        let n = usize::try_from(count).unwrap_or(usize::MAX);
        bytes_replace_n(bytes, &old, &new, n, vm.heap)?
    };

    allocate_bytes(result, vm.heap)
}

/// Parses arguments for bytes.replace method.
fn parse_bytes_replace_args(
    _method: &str,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(Vec<u8>, Vec<u8>, i64)> {
    let BytesReplaceArgs { old, new, count } = BytesReplaceArgs::from_args(args, vm)?;
    defer_drop!(old, vm);
    defer_drop!(new, vm);
    defer_drop!(count, vm);

    let old_b = extract_bytes_only(old, vm)?.to_owned();
    let new_b = extract_bytes_only(new, vm)?.to_owned();
    let count_i = count.as_int(vm)?;
    Ok((old_b, new_b, count_i))
}

/// Argument shape for `bytes.replace(old, new, count=-1)`.
#[derive(FromArgs)]
#[from_args(name = "replace")]
struct BytesReplaceArgs {
    old: Value,
    new: Value,
    #[from_args(default = Value::Int(-1))]
    count: Value,
}

/// Replaces all occurrences of `old` with `new` in bytes.
///
/// Checks the time limit periodically to enforce `max_duration` during
/// potentially long replacement operations on large byte sequences.
fn bytes_replace_all(
    bytes: &[u8],
    old: &[u8],
    new: &[u8],
    heap: &Heap<impl ResourceTracker>,
) -> Result<Vec<u8>, ResourceError> {
    if old.is_empty() {
        // Empty pattern: insert new before each byte and at the end
        let mut result = Vec::with_capacity(bytes.len() + new.len() * (bytes.len() + 1));
        for &b in bytes {
            heap.check_time()?;
            result.extend_from_slice(new);
            result.push(b);
        }
        result.extend_from_slice(new);
        Ok(result)
    } else {
        let mut result = Vec::new();
        let mut start = 0;
        while let Some(pos) = find_subsequence(&bytes[start..], old) {
            heap.check_time()?;
            result.extend_from_slice(&bytes[start..start + pos]);
            result.extend_from_slice(new);
            start = start + pos + old.len();
        }
        result.extend_from_slice(&bytes[start..]);
        Ok(result)
    }
}

/// Replaces at most n occurrences of `old` with `new` in bytes.
///
/// Checks the time limit periodically to enforce `max_duration` during
/// potentially long replacement operations on large byte sequences.
fn bytes_replace_n(
    bytes: &[u8],
    old: &[u8],
    new: &[u8],
    n: usize,
    heap: &Heap<impl ResourceTracker>,
) -> Result<Vec<u8>, ResourceError> {
    if old.is_empty() {
        // Empty pattern: insert new before each byte (up to n times)
        let mut result = Vec::new();
        let mut count = 0;
        for &b in bytes {
            heap.check_time()?;
            if count < n {
                result.extend_from_slice(new);
                count += 1;
            }
            result.push(b);
        }
        if count < n {
            result.extend_from_slice(new);
        }
        Ok(result)
    } else {
        let mut result = Vec::new();
        let mut start = 0;
        let mut count = 0;
        while count < n {
            heap.check_time()?;
            if let Some(pos) = find_subsequence(&bytes[start..], old) {
                result.extend_from_slice(&bytes[start..start + pos]);
                result.extend_from_slice(new);
                start = start + pos + old.len();
                count += 1;
            } else {
                break;
            }
        }
        result.extend_from_slice(&bytes[start..]);
        Ok(result)
    }
}

/// Implements Python's `bytes.center(width[, fillbyte])` method.
///
/// Returns centered in a bytes of length width.
fn bytes_center<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let (width, fillbyte) = parse_bytes_justify_args("bytes.center", args, vm)?;

    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    let result = if width <= len {
        bytes.to_vec()
    } else {
        check_repeat_size(width, 1, vm.heap.tracker())?;
        let total_pad = width - len;
        let left_pad = total_pad / 2;
        let right_pad = total_pad - left_pad;
        let mut result = Vec::with_capacity(width);
        for _ in 0..left_pad {
            result.push(fillbyte);
        }
        result.extend_from_slice(bytes);
        for _ in 0..right_pad {
            result.push(fillbyte);
        }
        result
    };

    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.ljust(width[, fillbyte])` method.
///
/// Returns left-justified in a bytes of length width.
fn bytes_ljust<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let (width, fillbyte) = parse_bytes_justify_args("bytes.ljust", args, vm)?;

    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    let result = if width <= len {
        bytes.to_vec()
    } else {
        check_repeat_size(width, 1, vm.heap.tracker())?;
        let pad = width - len;
        let mut result = Vec::with_capacity(width);
        result.extend_from_slice(bytes);
        for _ in 0..pad {
            result.push(fillbyte);
        }
        result
    };

    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.rjust(width[, fillbyte])` method.
///
/// Returns right-justified in a bytes of length width.
fn bytes_rjust<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let (width, fillbyte) = parse_bytes_justify_args("bytes.rjust", args, vm)?;

    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    let result = if width <= len {
        bytes.to_vec()
    } else {
        check_repeat_size(width, 1, vm.heap.tracker())?;
        let pad = width - len;
        let mut result = Vec::with_capacity(width);
        for _ in 0..pad {
            result.push(fillbyte);
        }
        result.extend_from_slice(bytes);
        result
    };

    allocate_bytes(result, vm.heap)
}

/// Parses arguments for bytes justify methods (center, ljust, rjust).
fn parse_bytes_justify_args(
    method: &str,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(usize, u8)> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);

    let extract_width = |v: &Value| -> RunResult<usize> {
        let w = v.as_int(vm)?;
        Ok(if w < 0 {
            0
        } else {
            usize::try_from(w).unwrap_or(usize::MAX)
        })
    };

    let extract_fill = |v: &Value| -> RunResult<u8> {
        let fill_bytes = extract_bytes_only(v, vm)?;
        if fill_bytes.len() != 1 {
            return Err(ExcType::type_error(format!(
                "{method}() argument 2 must be a byte string of length 1, not bytes of length {}",
                fill_bytes.len()
            )));
        }
        Ok(fill_bytes[0])
    };

    match pos.as_slice() {
        [width_value] => Ok((extract_width(width_value)?, b' ')),
        [width_value, fillbyte_value] => Ok((extract_width(width_value)?, extract_fill(fillbyte_value)?)),
        [] => Err(ExcType::type_error_at_least(method, 1, 0)),
        _ => Err(ExcType::type_error_at_most(method, 2, pos.len())),
    }
}

/// Implements Python's `bytes.zfill(width)` method.
///
/// Returns a copy of the bytes left filled with ASCII '0' digits.
fn bytes_zfill<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let width_value = args.get_one_arg("bytes.zfill", vm.heap)?;
    defer_drop!(width_value, vm);
    let width_i64 = width_value.as_int(vm)?;

    let width = if width_i64 < 0 {
        0
    } else {
        usize::try_from(width_i64).unwrap_or(usize::MAX)
    };

    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    let result = if width <= len {
        bytes.to_vec()
    } else {
        check_repeat_size(width, 1, vm.heap.tracker())?;
        let pad = width - len;
        let mut result = Vec::with_capacity(width);

        // Handle sign prefix
        if !bytes.is_empty() && (bytes[0] == b'+' || bytes[0] == b'-') {
            result.push(bytes[0]);
            result.resize(pad + 1, b'0');
            result.extend_from_slice(&bytes[1..]);
        } else {
            result.resize(pad, b'0');
            result.extend_from_slice(bytes);
        }
        result
    };

    allocate_bytes(result, vm.heap)
}

// =============================================================================
// Join method
// =============================================================================

/// Implements Python's `bytes.join(iterable)` method.
///
/// Joins elements of the iterable with the separator bytes.
fn bytes_join<'h>(
    separator: &HeapRead<'h, [u8]>,
    iterable: Value,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let Ok(iter) = MontyIter::new(iterable, vm) else {
        return Err(ExcType::type_error_join_not_iterable());
    };
    defer_drop_mut!(iter, vm);

    let mut result = Vec::new();
    let mut index = 0usize;

    while let Some(item) = iter.for_next(vm)? {
        defer_drop!(item, vm);

        if index > 0 {
            result.extend_from_slice(separator.get(vm.heap));
        }

        // Check item is bytes and extract its content
        match item {
            Value::InternBytes(id) => {
                result.extend_from_slice(vm.interns.get_bytes(*id));
            }
            Value::Ref(heap_id) => {
                if let HeapData::Bytes(b) = vm.heap.get(*heap_id) {
                    result.extend_from_slice(b.as_slice());
                } else {
                    let t = item.py_type(vm);
                    return Err(ExcType::type_error(format!(
                        "sequence item {index}: expected a bytes-like object, {t} found"
                    )));
                }
            }
            _ => {
                let t = item.py_type(vm);
                return Err(ExcType::type_error(format!(
                    "sequence item {index}: expected a bytes-like object, {t} found"
                )));
            }
        }
        index += 1;
    }

    allocate_bytes(result, vm.heap)
}

// =============================================================================
// Hex method
// =============================================================================

/// Implements Python's `bytes.hex([sep[, bytes_per_sep]])` method.
///
/// Returns a string containing the hexadecimal representation of the bytes.
fn bytes_hex<'h>(
    bytes: &HeapRead<'h, [u8]>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let (sep, bytes_per_sep) = parse_bytes_hex_args(args, vm)?;

    let bytes = bytes.get(vm.heap);
    let hex_chars: Vec<char> = bytes
        .iter()
        .flat_map(|b| {
            let hi = (b >> 4) & 0xf;
            let lo = b & 0xf;
            let hi_char = if hi < 10 {
                (b'0' + hi) as char
            } else {
                (b'a' + hi - 10) as char
            };
            let lo_char = if lo < 10 {
                (b'0' + lo) as char
            } else {
                (b'a' + lo - 10) as char
            };
            [hi_char, lo_char]
        })
        .collect();

    let result = if let Some(sep) = sep {
        if bytes_per_sep == 0 || bytes.is_empty() {
            hex_chars.iter().collect()
        } else {
            // Insert separator every `bytes_per_sep` bytes (2*bytes_per_sep hex chars).
            // `saturating_mul` guards against overflow when `bytes_per_sep == i64::MIN`,
            // whose `unsigned_abs()` is `2^63` and would wrap `* 2` to zero, triggering
            // a panic in `chunks(0)` below.
            let chars_per_group = usize::try_from(bytes_per_sep.unsigned_abs())
                .unwrap_or(usize::MAX)
                .saturating_mul(2);
            let mut result = String::new();

            if bytes_per_sep > 0 {
                // Positive: count from right, so partial group is at the START
                let total_len = hex_chars.len();
                let first_chunk_len = total_len % chars_per_group;
                let first_chunk_len = if first_chunk_len == 0 {
                    chars_per_group
                } else {
                    first_chunk_len
                };

                result.extend(&hex_chars[..first_chunk_len]);
                for chunk in hex_chars[first_chunk_len..].chunks(chars_per_group) {
                    result.push(sep);
                    result.extend(chunk);
                }
            } else {
                // Negative: count from left, so partial group is at the END
                for (i, chunk) in hex_chars.chunks(chars_per_group).enumerate() {
                    if i > 0 {
                        result.push(sep);
                    }
                    result.extend(chunk);
                }
            }
            result
        }
    } else {
        hex_chars.iter().collect()
    };

    Ok(super::str::allocate_string(result, vm.heap)?)
}

/// Parses arguments for bytes.hex method.
fn parse_bytes_hex_args(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<(Option<char>, i64)> {
    let BytesHexArgs { sep, bytes_per_sep } = BytesHexArgs::from_args(args, vm)?;
    defer_drop!(sep, vm);
    defer_drop!(bytes_per_sep, vm);

    let Some(sep_value) = (match &sep {
        Value::None => None,
        v => Some(v),
    }) else {
        // CPython treats absent `sep` as "no separator", regardless of `bytes_per_sep`.
        return Ok((None, 1));
    };

    let sep_bytes = match sep_value {
        Value::InternString(id) => vm.interns.get_str(*id).as_bytes(),
        Value::InternBytes(id) => vm.interns.get_bytes(*id),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(s) => s.as_bytes(),
            HeapData::Bytes(b) => b.as_slice(),
            _ => return Err(ExcType::type_error("sep must be str or bytes")),
        },
        _ => return Err(ExcType::type_error("sep must be str or bytes")),
    };

    let sep_char = match sep_bytes {
        [b] if b.is_ascii() => *b as char,
        _ => return Err(SimpleException::new_msg(ExcType::ValueError, "sep must be a single ASCII character").into()),
    };

    let bytes_per_sep = match bytes_per_sep {
        Value::None => 1,
        bps_value => {
            // CPython parses `bytes_per_sep` with the `i` format (C int), so values outside
            // c_int range raise OverflowError before any computation happens.
            let raw = bps_value.as_int(vm)?;
            c_int::try_from(raw).map_err(|_| ExcType::overflow_c_int())?.into()
        }
    };

    Ok((Some(sep_char), bytes_per_sep))
}

/// `bytes.hex([sep[, bytes_per_sep]])` — CPython accepts `sep` and
/// `bytes_per_sep` as positional-or-keyword, but Monty has not threaded
/// kwarg dispatch through to the type-checking body yet.
/// `kwargs_not_supported_yet` rejects any kwarg with
/// `NotImplementedError: bytes.hex() does not yet support keyword
/// arguments` (replacing the previous `TypeError: bytes.hex() takes no
/// keyword arguments` from `into_pos_only`) while the macro takes over
/// arity validation, upgrading the too-many-args wording from
/// `bytes.hex expected at most 2 arguments, got N` to CPython's
/// `bytes.hex() takes at most 2 arguments (N given)`. Fields become real
/// kwargs and the flag goes away when the kwarg dispatch is plumbed
/// through.
#[derive(FromArgs)]
#[from_args(name = "bytes.hex", style = c_named, at_most_total, kwargs_not_supported_yet)]
struct BytesHexArgs {
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::None)]
    bytes_per_sep: Value,
}

// =============================================================================
// fromhex classmethod
// =============================================================================

/// Implements Python's `bytes.fromhex(string)` classmethod.
///
/// Creates bytes from a hexadecimal string. Whitespace is allowed between byte pairs,
/// but not between the two digits of a byte.
pub fn bytes_fromhex(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let hex_value = args.get_one_arg("bytes.fromhex", vm.heap)?;
    defer_drop!(hex_value, vm);

    let hex_str = match hex_value {
        Value::InternString(id) => vm.interns.get_str(*id),
        Value::Ref(heap_id) => {
            if let HeapData::Str(s) = vm.heap.get(*heap_id) {
                s.as_str()
            } else {
                return Err(ExcType::type_error("fromhex() argument must be str, not bytes"));
            }
        }
        _ => {
            let t = hex_value.py_type(vm);
            return Err(ExcType::type_error(format!("fromhex() argument must be str, not {t}")));
        }
    };

    // CPython allows whitespace BETWEEN byte pairs, but NOT within a pair.
    // - "de ad" is valid (whitespace between pairs)
    // - "d e" or "0 1" are NOT valid (whitespace within a pair)
    // - " 01 " is valid (whitespace before/after)
    //
    // Error messages:
    // - Invalid char (including whitespace in wrong place): "non-hexadecimal number found ... at position X"
    // - Odd number of valid hex digits: "must contain an even number of hexadecimal digits"

    let mut result = Vec::new();
    let mut chars = hex_str.chars().enumerate().peekable();

    loop {
        // Skip whitespace BETWEEN byte pairs (before the high nibble)
        while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
            chars.next();
        }

        // Get high nibble
        let Some((hi_pos, hi_char)) = chars.next() else {
            break; // End of string - we're done
        };

        let Some(hi_val) = hex_char_to_value(hi_char) else {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("non-hexadecimal number found in fromhex() arg at position {hi_pos}"),
            )
            .into());
        };

        // Get low nibble - must be IMMEDIATELY after high nibble (no whitespace)
        let Some((lo_pos, lo_char)) = chars.next() else {
            // End of string after high nibble = odd number of hex digits
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                "fromhex() arg must contain an even number of hexadecimal digits",
            )
            .into());
        };

        let Some(lo_val) = hex_char_to_value(lo_char) else {
            // Invalid character (including whitespace) in low nibble position
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!("non-hexadecimal number found in fromhex() arg at position {lo_pos}"),
            )
            .into());
        };

        result.push((hi_val << 4) | lo_val);
    }

    allocate_bytes(result, vm.heap)
}

/// Converts a hex character to its numeric value.
fn hex_char_to_value(c: char) -> Option<u8> {
    match c {
        '0'..='9' => Some(c as u8 - b'0'),
        'a'..='f' => Some(c as u8 - b'a' + 10),
        'A'..='F' => Some(c as u8 - b'A' + 10),
        _ => None,
    }
}

// =============================================================================
// Helper function for bytes allocation
// =============================================================================

/// Allocates bytes on the heap.
fn allocate_bytes(bytes: Vec<u8>, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    let heap_id = heap.allocate(HeapData::Bytes(Bytes::new(bytes)))?;
    Ok(Value::Ref(heap_id))
}
