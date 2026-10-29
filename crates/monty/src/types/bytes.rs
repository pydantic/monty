use std::{cell::Cell, ffi::c_int, fmt::Write, ops, ops::Range, str};

/// Python bytes type, wrapping a `Vec<u8>`.
///
/// This type provides Python bytes semantics with operations on ASCII bytes only.
/// Unlike str methods which operate on Unicode codepoints, bytes methods only
/// recognize ASCII characters (0-127) for case transformations and predicates.
///
/// # Implemented Methods
///
/// ## Encoding/Decoding
/// - `decode([encoding[, errors]])` - Decode to string (UTF-8, ASCII, UTF-16/32)
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
use monty_types::ResourceError;
pub use monty_types::{bytes_repr, bytes_repr_fmt};

use super::{CmpOrder, LazyHeapSet, PyTrait, Type};
use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    codecs::Codec,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::{HashValue, hash_python_bytes},
    heap::{
        DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapView, heap_read_ref_as_field,
    },
    identity::Identity,
    intern::{BytesId, StaticStrings},
    percent_format::{copy_bytes_template, percent_format_bytes},
    resource_checks::{check_repeat_size, check_replace_size},
    types::{
        List,
        long_int::repeat_count,
        py_trait::PyObjectIdentity,
        slice::{SearchRange, search_range, slice_collect_iterator},
        str_like::{self, ArgRole, Direction, SearchOp, StrLike, StripSide, find_subsequence, find_with, finder_for},
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
pub(crate) struct Bytes(
    #[serde(with = "serde_bytes")] Vec<u8>,
    #[serde(skip)] Cell<Option<HashValue>>,
);

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
    /// - `bytes(string, encoding, errors)` encodes via the codec registry —
    ///   like CPython, a str source *requires* an encoding
    /// - `bytes(bytes)` returns a copy of the bytes
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let BytesInitArgs {
            source,
            encoding,
            errors,
        } = BytesInitArgs::from_args(args, vm)?;
        defer_drop!(source, vm);
        defer_drop!(encoding, vm);
        defer_drop!(errors, vm);
        let source_str = match source {
            Some(v) if v.is_str(vm.heap) => Some(v),
            _ => None,
        };
        // CPython's `bytes_new` ordering: an encoding demands a str source, a
        // str source demands an encoding, and a lone `errors` is never valid.
        if let Some(encoding) = encoding {
            let Some(v) = source_str else {
                return Err(ExcType::type_error_encoding_without_string());
            };
            let s = v.to_str(vm)?;
            let errors = errors.as_ref().map_or("strict", |e| e.as_str(vm));
            let encoding = encoding.as_str(vm);
            let codec = Codec::find(encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(encoding))?;
            let encoded = codec.encode(s, errors, &vm.heap.tracker)?;
            let heap_id = vm.heap.allocate(HeapData::Bytes(Self::new(encoded)));
            return Ok(Value::Ref(heap_id));
        }
        if source_str.is_some() {
            return Err(ExcType::type_error_string_without_encoding());
        }
        if errors.is_some() {
            return Err(ExcType::type_error_errors_without_string());
        }
        let new_data = match source {
            None => Vec::new(),
            Some(Value::Int(n)) => {
                if *n < 0 {
                    return Err(ExcType::value_error_negative_bytes_count());
                }
                // Fallible on a 32-bit target (`wasm32-wasip1`), where `bytes(2**40)`
                // is a count no `usize` can hold. On 64-bit the conversion always
                // succeeds and the size check below rejects it with `MemoryError`.
                let size = usize::try_from(*n).map_err(|_| ExcType::overflow_index_sized_int())?;
                // Pre-check the requested size against resource limits before
                // touching the global allocator. Without this, `bytes(n)` for a
                // very large `n` would attempt the native allocation directly
                // and abort the host on failure rather than raising MemoryError.
                // Mirrors the guard already used by `bytes.ljust`/`zfill`/`*`.
                check_repeat_size(size, 1, &vm.heap.tracker)?;
                vec![0u8; size]
            }
            Some(Value::InternBytes(bytes_id)) => {
                let b = vm.interns.get_bytes(*bytes_id);
                b.to_vec()
            }
            Some(v @ Value::Ref(id)) => match vm.heap.get(*id) {
                HeapData::Bytes(b) => b.as_slice().to_vec(),
                _ => return Err(ExcType::type_error_bytes_init(&v.py_type_name(vm))),
            },
            Some(v) => return Err(ExcType::type_error_bytes_init(&v.py_type_name(vm))),
        };
        let heap_id = vm.heap.allocate(HeapData::Bytes(Self::new(new_data)));
        Ok(Value::Ref(heap_id))
    }
}

/// Argument shape for `bytes(source=..., encoding=..., errors=...)`, all
/// pos-or-keyword. Unlike `str()`, CPython's `bytes_new` reports wrong-typed
/// `encoding`/`errors` with `_PyArg_BadArgument`'s wording (`must be str, not
/// None` for `None`), which is exactly the macro's `bad_arg_named` form.
#[derive(FromArgs)]
#[from_args(name = "bytes", at_most_total, bad_arg_named)]
struct BytesInitArgs {
    #[from_args(default)]
    source: Option<Value>,
    #[from_args(default)]
    encoding: Option<StrArg>,
    #[from_args(default)]
    errors: Option<StrArg>,
}

/// Concatenates two byte strings into a tracked heap value.
pub(crate) fn concat_bytes(lhs: &[u8], rhs: &[u8], heap: &Heap) -> Result<Value, ResourceError> {
    let result_len = lhs.len().saturating_add(rhs.len());
    check_repeat_size(result_len, 1, &heap.tracker)?;
    let mut result = Vec::with_capacity(result_len);
    result.extend_from_slice(lhs);
    result.extend_from_slice(rhs);
    Ok(Value::Ref(heap.allocate(HeapData::Bytes(result.into()))))
}

/// Repeats bytes after validating the allocation against resource limits.
fn repeat_bytes(value: &[u8], count: &Value, vm: &VM<'_>) -> RunResult<Option<Value>> {
    let Some(count) = repeat_count(count, vm)? else {
        return Ok(None);
    };
    check_repeat_size(value.len(), count, &vm.heap.tracker)?;
    Ok(Some(Value::Ref(
        vm.heap.allocate(HeapData::Bytes(value.repeat(count).into())),
    )))
}

/// An allocation-free bytes view retaining its interned or heap identity.
/// Heap handles can remain live across `__index__` calls without borrowing the buffer.
pub(crate) enum PyBytes<'h> {
    Interned(BytesId),
    Heap(HeapObjectRead<'h, Bytes>),
}

impl<'h> PyBytes<'h> {
    /// Extracts bytes without copying their contents.
    pub(crate) fn from_value(value: &Value, vm: &VM<'h>) -> Option<Self> {
        match value {
            Value::InternBytes(id) => Some(Self::Interned(*id)),
            Value::Ref(id) => vm.heap.read_as(*id).map(Self::Heap),
            _ => None,
        }
    }

    /// Borrows the buffer only for the duration of the heap borrow.
    pub(crate) fn as_slice<'a>(&'a self, vm: &'a VM<'h>) -> &'a [u8] {
        match self {
            Self::Interned(id) => vm.interns.get_bytes(*id),
            Self::Heap(value) => value.get(vm.heap).as_slice(),
        }
    }
}

impl PyObjectIdentity for PyBytes<'_> {
    fn py_identity(&self) -> Identity {
        match self {
            Self::Interned(id) => Identity::InternBytes(id.index()),
            Self::Heap(value) => value.py_identity(),
        }
    }
}

impl<'h> PyTrait<'h> for PyBytes<'h> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Bytes
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        match self {
            Self::Interned(id) => Value::InternBytes(*id).py_eq_impl(other, vm),
            Self::Heap(value) => value.py_eq_impl(other, vm),
        }
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.as_slice(vm).len())
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(!self.as_slice(vm).is_empty())
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(bytes_repr_fmt(self.as_slice(vm), f)?)
    }

    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        bytes_contains(self.as_slice(vm), item, vm).map(Some)
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        repeat_bytes(self.as_slice(vm), other, vm)
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        bytes_add(self.as_slice(vm), other, vm)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        match self {
            Self::Interned(id) => bytes_getitem(&*vm.heap.protect(vm.interns.get_bytes(*id)), key, vm),
            Self::Heap(value) => value.py_getitem(key, vm),
        }
    }

    fn py_mod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let template = copy_bytes_template(self.as_slice(vm), &vm.heap.tracker)?;
        percent_format_bytes(&template, other, vm).map(Some)
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match self {
            Self::Interned(id) => {
                let Some(method) = attr.static_string(vm.interns) else {
                    args.drop_with(vm);
                    return Err(ExcType::attribute_error(Type::Bytes, attr.as_str(vm.interns)));
                };
                let bytes = vm.heap.protect(vm.interns.get_bytes(*id));
                call_bytes_method_impl(&*bytes, method, args, vm).map(CallResult::Value)
            }
            Self::Heap(value) => value.py_call_attr(vm, attr, args),
        }
    }
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

/// Bytes are their own units, so every index is a byte offset.
impl StrLike for [u8] {
    type Unit = u8;

    const CHECKS_SUB_FIRST: bool = false;

    fn as_bytes(&self) -> &[u8] {
        self
    }

    fn py_len(&self) -> usize {
        self.len()
    }

    fn byte_range(&self, start: usize, end: usize) -> Range<usize> {
        start..end
    }

    fn unit_index(&self, byte_offset: usize) -> usize {
        byte_offset
    }

    fn units(&self) -> impl DoubleEndedIterator<Item = (usize, u8)> {
        self.iter().copied().enumerate()
    }

    fn unit_len(_: u8) -> usize {
        1
    }

    fn is_space(unit: u8) -> bool {
        is_py_whitespace(unit)
    }

    fn contains_unit(&self, unit: u8) -> bool {
        self.contains(&unit)
    }

    fn extract_arg<'a>(value: &Value, vm: &'a VM<'_>) -> Option<&'a Self> {
        match value {
            Value::InternBytes(id) => Some(vm.interns.get_bytes(*id)),
            Value::Ref(id) if let HeapData::Bytes(b) = vm.heap.get(*id) => Some(b.as_slice()),
            _ => None,
        }
    }

    fn arg_type_error(role: ArgRole, _method: &str, value: &Value, vm: &VM<'_>) -> RunError {
        let type_name = value.py_type_name(vm);
        match role {
            ArgRole::Sub => ExcType::type_error(format!(
                "argument should be integer or bytes-like object, not '{type_name}'"
            )),
            _ => ExcType::type_error_bytes_like_required(&type_name),
        }
    }

    fn byte_needle(value: &Value, method: &str, vm: &mut VM<'_>) -> RunResult<Option<u8>> {
        if Self::extract_arg(value, vm).is_some() {
            return Ok(None);
        }
        let Some(index) = value.py_index_impl(vm)? else {
            return Err(Self::arg_type_error(ArgRole::Sub, method, value, vm));
        };
        defer_drop!(index, vm);
        // A `LongInt` is never a byte: it exists because the value overflowed `i64`.
        let byte = match index {
            Value::Int(i) => u8::try_from(*i).ok(),
            Value::Bool(b) => Some(u8::from(*b)),
            _ => None,
        };
        byte.map(Some).ok_or_else(ExcType::value_error_byte_range)
    }

    fn allocate(&self, heap: &Heap) -> Value {
        allocate_bytes(self.to_vec(), heap)
    }

    fn not_found_error() -> RunError {
        ExcType::value_error_subsequence_not_found()
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Bytes> {
    /// One-sided implementation of Python membership (`__contains__`).
    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        bytes_contains(self.get(vm.heap).as_slice(), item, vm).map(Some)
    }

    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Bytes
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(BytesIterator::from_heap(self.id(), vm)))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).0.len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let bytes = heap_read_ref_as_field!(self, Bytes, 0);
        bytes_getitem(&bytes.as_slice(), key, vm)
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // Heap bytes equal interned or heap bytes with the same content.
        Ok(eq_bytes(self.get(vm.heap).as_slice(), other, vm))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
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

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        Ok(CmpOrder::Ordered(self.get(vm.heap).0.cmp(&other.get(vm.heap).0)))
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(!self.get(vm.heap).0.is_empty())
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(bytes_repr_fmt(&self.get(vm.heap).0, f)?)
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        bytes_add(self.get(vm.heap).as_slice(), other, vm)
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        repeat_bytes(self.get(vm.heap).as_slice(), other, vm)
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    /// `bytes % args` is printf-style formatting, see `percent_format`.
    fn py_mod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let template = copy_bytes_template(self.get(vm.heap).as_slice(), &vm.heap.tracker)?;
        percent_format_bytes(&template, other, vm).map(Some)
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let Some(method) = attr.static_string(vm.interns) else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::Bytes, attr.as_str(vm.interns)));
        };

        let field = heap_read_ref_as_field!(self, Bytes, 0);
        let bytes = field.as_slice();
        call_bytes_method_impl(&bytes, method, args, vm).map(CallResult::Value)
    }
}

/// Concatenation shared by logical bytes views and concrete heap dispatch.
fn bytes_add(bytes: &[u8], other: &Value, vm: &VM<'_>) -> RunResult<Option<Value>> {
    match PyBytes::from_value(other, vm) {
        Some(other) => Ok(Some(concat_bytes(bytes, other.as_slice(vm), vm.heap)?)),
        None => Ok(None),
    }
}

/// Indexes a protected buffer, allowing `__index__` to run before borrowing it.
fn bytes_getitem<'h>(bytes: &impl HeapView<'h, [u8]>, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
    if let Value::Ref(id) = key
        && let HeapData::Slice(slice) = vm.heap.get(*id)
    {
        let result = slice_collect_iterator(vm, slice, bytes.get(vm.heap).iter(), |b| *b)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::Bytes(Bytes::new(result)))))
    } else {
        let index = key.as_index(vm, Type::Bytes)?;
        let byte = get_byte_at_index(bytes.get(vm.heap), index).ok_or_else(ExcType::bytes_index_error)?;
        Ok(Value::Int(i64::from(byte)))
    }
}

impl HeapItem for Bytes {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No-op: bytes don't hold Value references
    }
}

/// Calls a bytes method on a byte slice.
///
/// This is the unified implementation for bytes method calls, used by both
/// heap-allocated `Bytes` (via `py_call_attr`) and interned bytes literals
/// (`Value::InternBytes`).
fn call_bytes_method_impl<'h>(
    bytes: &impl HeapView<'h, [u8]>,
    method: StaticStrings,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    match method {
        // Decode method
        StaticStrings::Decode => bytes_decode(bytes, args, vm),
        // Simple transformations (no arguments)
        StaticStrings::Lower => {
            args.check_zero_args("bytes.lower", vm.heap)?;
            Ok(bytes_lower(bytes, vm))
        }
        StaticStrings::Upper => {
            args.check_zero_args("bytes.upper", vm.heap)?;
            Ok(bytes_upper(bytes, vm))
        }
        StaticStrings::Capitalize => {
            args.check_zero_args("bytes.capitalize", vm.heap)?;
            Ok(bytes_capitalize(bytes, vm))
        }
        StaticStrings::Title => {
            args.check_zero_args("bytes.title", vm.heap)?;
            Ok(bytes_title(bytes, vm))
        }
        StaticStrings::Swapcase => {
            args.check_zero_args("bytes.swapcase", vm.heap)?;
            Ok(bytes_swapcase(bytes, vm))
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
        StaticStrings::Count => str_like::search(bytes, SearchOp::Count, "bytes.count", args, vm),
        StaticStrings::Find => str_like::search(bytes, SearchOp::Find, "bytes.find", args, vm),
        StaticStrings::Rfind => str_like::search(bytes, SearchOp::Rfind, "bytes.rfind", args, vm),
        StaticStrings::Index => str_like::search(bytes, SearchOp::Index, "bytes.index", args, vm),
        StaticStrings::Rindex => str_like::search(bytes, SearchOp::Rindex, "bytes.rindex", args, vm),
        StaticStrings::Startswith => bytes_startswith(bytes, args, vm),
        StaticStrings::Endswith => bytes_endswith(bytes, args, vm),
        // Strip/trim methods
        StaticStrings::Strip => str_like::strip(bytes, StripSide::Both, "bytes.strip", args, vm),
        StaticStrings::Lstrip => str_like::strip(bytes, StripSide::Start, "bytes.lstrip", args, vm),
        StaticStrings::Rstrip => str_like::strip(bytes, StripSide::End, "bytes.rstrip", args, vm),
        StaticStrings::Removeprefix => str_like::remove_prefix(bytes, "bytes.removeprefix", args, vm),
        StaticStrings::Removesuffix => str_like::remove_suffix(bytes, "bytes.removesuffix", args, vm),
        // Split methods
        StaticStrings::Split => str_like::split(bytes, Direction::Forward, args, vm),
        StaticStrings::Rsplit => str_like::split(bytes, Direction::Reverse, args, vm),
        StaticStrings::Splitlines => bytes_splitlines(bytes, args, vm),
        StaticStrings::Partition => str_like::partition(bytes, Direction::Forward, "bytes.partition", args, vm),
        StaticStrings::Rpartition => str_like::partition(bytes, Direction::Reverse, "bytes.rpartition", args, vm),
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
            args.drop_with(vm.heap);
            Err(ExcType::attribute_error(Type::Bytes, method.into()))
        }
    }
}

/// Implements Python's `bytes.decode([encoding[, errors]])` method.
///
/// Converts bytes to a string. Encoding-name resolution and the codec
/// implementations (UTF-8, ASCII, UTF-16/32 families) live in
/// [`crate::codecs`]. Like CPython, the error handler name is only looked up
/// (and validated) when a byte actually needs handling — see
/// `limitations/encoding.md` for the handful of decode divergences (the
/// surrogate-producing handlers raise `NotImplementedError`).
fn bytes_decode<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let BytesDecodeArgs { encoding, errors } = BytesDecodeArgs::from_args(args, vm)?;
    defer_drop!(errors, vm);
    defer_drop!(encoding, vm);
    let encoding = encoding.as_ref().map_or("utf-8", |e| e.as_str(vm));
    let errors = errors.as_ref().map_or("strict", |e| e.as_str(vm));

    let codec = Codec::find(encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(encoding))?;
    let s = codec.decode(bytes.get(vm.heap), errors)?;
    Ok(super::str::allocate_string(s, vm.heap))
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

// =============================================================================
// Prefix/suffix matching
// =============================================================================

/// Implements Python's `bytes.startswith(prefix[, start[, end]])` method.
///
/// Returns True if bytes starts with the specified prefix.
/// Accepts bytes or a tuple of bytes as prefix. If a tuple is given, returns True
/// if any of the prefixes match.
fn bytes_startswith<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (prefix_arg, range) = parse_bytes_prefix_suffix_args("bytes.startswith", len, args, vm)?;
    let Some((start, end)) = range else {
        // `start` past `end` matches nothing: `b'abc'.startswith(b'', 4) == False`
        return Ok(Value::Bool(false));
    };

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
fn bytes_endswith<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let len = bytes.get(vm.heap).len();
    let (suffix_arg, range) = parse_bytes_prefix_suffix_args("bytes.endswith", len, args, vm)?;
    let Some((start, end)) = range else {
        // `start` past `end` matches nothing: `b'abc'.endswith(b'', 4) == False`
        return Ok(Value::Bool(false));
    };

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
/// Returns the prefix/suffix_arg and the `(start, end)` range to match in, or
/// `None` for a `start` past `end`, where nothing matches (see [`search_range`]).
/// The prefix/suffix_arg can be a single bytes value or a tuple of bytes values.
fn parse_bytes_prefix_suffix_args(
    method: &str,
    len: usize,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<(PrefixSuffixArg, SearchRange)> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);

    match pos.as_slice() {
        [prefix_value, bounds @ ..] if bounds.len() <= 2 => {
            let prefix = extract_bytes_for_prefix_suffix(prefix_value, method, vm)?;
            let range = search_range(bounds.first(), bounds.get(1), len, vm)?;
            Ok((prefix, range))
        }
        [] => Err(ExcType::type_error_at_least(method, 1, 0)),
        _ => Err(ExcType::type_error_at_most(method, 3, pos.len())),
    }
}

/// Extracts bytes (or tuple of bytes) for startswith/endswith methods.
///
/// Returns `PrefixSuffixArg::Single` for a single bytes value, or
/// `PrefixSuffixArg::Multiple` for a tuple of bytes values.
fn extract_bytes_for_prefix_suffix(value: &Value, method: &str, vm: &VM<'_>) -> RunResult<PrefixSuffixArg> {
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
                        let item_type = item.py_type_name(vm);
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
                value.py_type_name(vm)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "{method_name} first arg must be bytes or a tuple of bytes, not {}",
            value.py_type_name(vm)
        ))),
    }
}

/// Extracts a single bytes value for tuple element in startswith/endswith.
fn extract_single_bytes_for_prefix_suffix(value: &Value, vm: &VM<'_>) -> RunResult<Vec<u8>> {
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

/// Extracts bytes from a Value, raising CPython's `a bytes-like object is required, not '{type}'` otherwise.
///
/// `str` is rejected too, as CPython's `bytes` methods do.
fn extract_bytes_only<'a>(value: &Value, vm: &'a VM<'_>) -> RunResult<&'a [u8]> {
    <[u8]>::extract_arg(value, vm).ok_or_else(|| ExcType::type_error_bytes_like_required(&value.py_type_name(vm)))
}

// =============================================================================
// Simple transformations (no arguments)
// =============================================================================

/// Implements Python's `bytes.lower()` method.
///
/// Returns a copy of the bytes with all ASCII uppercase characters converted to lowercase.
fn bytes_lower<'h>(bytes: &impl HeapView<'h, [u8]>, vm: &mut VM<'h>) -> Value {
    let result: Vec<u8> = bytes.get(vm.heap).iter().map(|&b| b.to_ascii_lowercase()).collect();
    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.upper()` method.
///
/// Returns a copy of the bytes with all ASCII lowercase characters converted to uppercase.
fn bytes_upper<'h>(bytes: &impl HeapView<'h, [u8]>, vm: &mut VM<'h>) -> Value {
    let result: Vec<u8> = bytes.get(vm.heap).iter().map(|&b| b.to_ascii_uppercase()).collect();
    allocate_bytes(result, vm.heap)
}

/// Implements Python's `bytes.capitalize()` method.
///
/// Returns a copy of the bytes with the first byte capitalized (if ASCII) and
/// the rest lowercased.
fn bytes_capitalize<'h>(bytes: &impl HeapView<'h, [u8]>, vm: &mut VM<'h>) -> Value {
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
fn bytes_title<'h>(bytes: &impl HeapView<'h, [u8]>, vm: &mut VM<'h>) -> Value {
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
fn bytes_swapcase<'h>(bytes: &impl HeapView<'h, [u8]>, vm: &mut VM<'h>) -> Value {
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
// Split methods
// =============================================================================

/// Implements Python's `bytes.splitlines([keepends])` method.
///
/// Returns a list of the lines in the bytes, breaking at line boundaries.
fn bytes_splitlines<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let keepends = parse_bytes_splitlines_args(args, vm)?;

    let lines = Vec::new();
    let mut lines_guard = DropGuard::new(lines, vm);
    let (lines, vm) = lines_guard.as_parts_mut();
    let mut start = 0;
    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    while start < len {
        vm.heap.tracker.check_memory_time_every(lines.len())?;

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
        lines.push(allocate_bytes(line.to_vec(), vm.heap));
        start = end;
    }

    let (lines, vm) = lines_guard.into_parts();
    let list = List::new(lines);
    let heap_id = vm.heap.allocate(HeapData::List(list));
    Ok(Value::Ref(heap_id))
}

/// Parses arguments for bytes.splitlines method.
fn parse_bytes_splitlines_args(args: ArgValues, vm: &mut VM<'_>) -> RunResult<bool> {
    let BytesSplitlinesArgs { keepends } = BytesSplitlinesArgs::from_args(args, vm)?;
    let result = match keepends {
        None => false,
        Some(v) => {
            let result = v.py_bool(vm);
            v.drop_with(vm.heap);
            result?
        }
    };
    Ok(result)
}

/// Argument shape for `bytes.splitlines(keepends=False)`. CPython evaluates
/// `keepends` for truthiness rather than strict-typing, so the field stays as
/// a raw `Value` for `py_bool` to inspect.
#[derive(FromArgs)]
#[from_args(name = "splitlines", at_most_total)]
struct BytesSplitlinesArgs {
    #[from_args(default)]
    keepends: Option<Value>,
}

// =============================================================================
// Replace/padding methods
// =============================================================================

/// Implements Python's `bytes.replace(old, new[, count])` method.
///
/// Returns a copy with all occurrences of old replaced by new.
fn bytes_replace<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let (old, new, count) = parse_bytes_replace_args("bytes.replace", args, vm)?;

    let bytes = bytes.get(vm.heap);

    check_replace_size(bytes.len(), old.len(), new.len(), count, &vm.heap.tracker)?;

    let result = if count < 0 {
        bytes_replace_all(bytes, &old, &new, vm.heap)?
    } else {
        let n = usize::try_from(count).unwrap_or(usize::MAX);
        bytes_replace_n(bytes, &old, &new, n, vm.heap)?
    };

    Ok(allocate_bytes(result, vm.heap))
}

/// Parses arguments for bytes.replace method.
fn parse_bytes_replace_args(_method: &str, args: ArgValues, vm: &mut VM<'_>) -> RunResult<(Vec<u8>, Vec<u8>, i64)> {
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
/// Checks the time limits periodically during potentially long replacement
/// operations on large byte sequences.
fn bytes_replace_all(bytes: &[u8], old: &[u8], new: &[u8], heap: &Heap) -> Result<Vec<u8>, ResourceError> {
    if old.is_empty() {
        // Empty pattern: insert new before each byte and at the end
        let mut result = Vec::with_capacity(bytes.len() + new.len() * (bytes.len() + 1));
        for (i, &b) in bytes.iter().enumerate() {
            heap.tracker.check_memory_time_every(i)?;
            result.extend_from_slice(new);
            result.push(b);
        }
        result.extend_from_slice(new);
        Ok(result)
    } else if let Some(finder) = finder_for(old, bytes) {
        let mut result = Vec::new();
        let mut start = 0;
        let mut matches = 0usize;
        while let Some(pos) = find_with(&finder, &bytes[start..], heap)? {
            heap.tracker.check_memory_time_every(matches)?;
            matches += 1;
            result.extend_from_slice(&bytes[start..start + pos]);
            result.extend_from_slice(new);
            start = start + pos + old.len();
        }
        result.extend_from_slice(&bytes[start..]);
        Ok(result)
    } else {
        replace_nothing(bytes, heap)
    }
}

/// Copies `bytes` unchanged, for a `replace` that cannot match anything.
///
/// Polls before copying: the callers reaching here skip the scan loop, and
/// with it the `check_time()` it would have run first, so without this a
/// no-match `replace` over a large input would never touch the clock.
fn replace_nothing(bytes: &[u8], heap: &Heap) -> Result<Vec<u8>, ResourceError> {
    heap.tracker.check_time()?;
    Ok(bytes.to_vec())
}

/// Replaces at most n occurrences of `old` with `new` in bytes.
///
/// Checks the time limits periodically during potentially long replacement
/// operations on large byte sequences.
fn bytes_replace_n(bytes: &[u8], old: &[u8], new: &[u8], n: usize, heap: &Heap) -> Result<Vec<u8>, ResourceError> {
    // `count=0` permits no replacements, so return before `finder_for`
    // preprocesses `old` — that runs ahead of any `check_time()`.
    if n == 0 {
        return replace_nothing(bytes, heap);
    }
    if old.is_empty() {
        // Empty pattern: insert new before each byte (up to n times)
        let mut result = Vec::new();
        let mut count = 0;
        for (i, &b) in bytes.iter().enumerate() {
            heap.tracker.check_memory_time_every(i)?;
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
    } else if let Some(finder) = finder_for(old, bytes) {
        let mut result = Vec::new();
        let mut start = 0;
        let mut count = 0;
        while count < n {
            heap.tracker.check_memory_time_every(count)?;
            if let Some(pos) = find_with(&finder, &bytes[start..], heap)? {
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
    } else {
        replace_nothing(bytes, heap)
    }
}

/// Implements Python's `bytes.center(width[, fillbyte])` method.
///
/// Returns centered in a bytes of length width.
fn bytes_center<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let (width, fillbyte) = parse_bytes_justify_args("bytes.center", args, vm)?;

    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    let result = if width <= len {
        bytes.to_vec()
    } else {
        check_repeat_size(width, 1, &vm.heap.tracker)?;
        let total_pad = width - len;
        // CPython puts the odd fill unit on the left only when `width` is also odd.
        let left_pad = total_pad / 2 + (total_pad & width & 1);
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

    Ok(allocate_bytes(result, vm.heap))
}

/// Implements Python's `bytes.ljust(width[, fillbyte])` method.
///
/// Returns left-justified in a bytes of length width.
fn bytes_ljust<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let (width, fillbyte) = parse_bytes_justify_args("bytes.ljust", args, vm)?;

    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    let result = if width <= len {
        bytes.to_vec()
    } else {
        check_repeat_size(width, 1, &vm.heap.tracker)?;
        let pad = width - len;
        let mut result = Vec::with_capacity(width);
        result.extend_from_slice(bytes);
        for _ in 0..pad {
            result.push(fillbyte);
        }
        result
    };

    Ok(allocate_bytes(result, vm.heap))
}

/// Implements Python's `bytes.rjust(width[, fillbyte])` method.
///
/// Returns right-justified in a bytes of length width.
fn bytes_rjust<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let (width, fillbyte) = parse_bytes_justify_args("bytes.rjust", args, vm)?;

    let bytes = bytes.get(vm.heap);
    let len = bytes.len();

    let result = if width <= len {
        bytes.to_vec()
    } else {
        check_repeat_size(width, 1, &vm.heap.tracker)?;
        let pad = width - len;
        let mut result = Vec::with_capacity(width);
        for _ in 0..pad {
            result.push(fillbyte);
        }
        result.extend_from_slice(bytes);
        result
    };

    Ok(allocate_bytes(result, vm.heap))
}

/// Parses arguments for bytes justify methods (center, ljust, rjust).
fn parse_bytes_justify_args(method: &str, args: ArgValues, vm: &mut VM<'_>) -> RunResult<(usize, u8)> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);

    // Free functions rather than closures: `extract_width` needs `&mut VM` for a
    // user `__index__`, which cannot coexist with a second closure capturing the
    // same `vm` immutably.
    match pos.as_slice() {
        [width_value] => Ok((extract_justify_width(width_value, vm)?, b' ')),
        [width_value, fillbyte_value] => {
            // Width first, so its error wins for `center('x', 'y')` as in CPython.
            let width = extract_justify_width(width_value, vm)?;
            Ok((width, extract_justify_fill(method, fillbyte_value, vm)?))
        }
        [] => Err(ExcType::type_error_at_least(method, 1, 0)),
        _ => Err(ExcType::type_error_at_most(method, 2, pos.len())),
    }
}

/// Reads a justify method's `width`, clamping negatives to zero as CPython does.
fn extract_justify_width(value: &Value, vm: &mut VM<'_>) -> RunResult<usize> {
    let width = value.as_int(vm)?;
    Ok(if width < 0 {
        0
    } else {
        usize::try_from(width).unwrap_or(usize::MAX)
    })
}

/// Reads a justify method's `fillbyte`, which must be exactly one byte.
fn extract_justify_fill(method: &str, value: &Value, vm: &VM<'_>) -> RunResult<u8> {
    let name = str_like::bare_name(method);
    match <[u8]>::extract_arg(value, vm) {
        Some(&[byte]) => Ok(byte),
        Some(fill) => Err(ExcType::type_error(format!(
            "{name}(): argument 2 must be a byte string of length 1, not a bytes object of length {}",
            fill.len()
        ))),
        None => Err(ExcType::type_error(format!(
            "{name}() argument 2 must be a byte string of length 1, not {}",
            value.py_type(vm).cpython_arg_name(vm.heap, vm.interns)
        ))),
    }
}

/// Implements Python's `bytes.zfill(width)` method.
///
/// Returns a copy of the bytes left filled with ASCII '0' digits.
fn bytes_zfill<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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
        check_repeat_size(width, 1, &vm.heap.tracker)?;
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

    Ok(allocate_bytes(result, vm.heap))
}

// =============================================================================
// Join method
// =============================================================================

/// Implements Python's `bytes.join(iterable)` method.
///
/// Joins elements of the iterable with the separator bytes.
fn bytes_join<'h>(separator: &impl HeapView<'h, [u8]>, iterable: Value, vm: &mut VM<'h>) -> RunResult<Value> {
    // Checked up front: a user `__iter__` can raise anything, and rewriting that
    // as "can only join an iterable" would hide it, resource errors included.
    if !iterable.py_is_iterable(vm) {
        iterable.drop_with(vm);
        return Err(ExcType::type_error_join_not_iterable());
    }
    let iter = iterable.into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    let mut result = Vec::new();
    let mut index = 0usize;

    while let Some(item) = iter.py_next(vm)? {
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
                    let t = item.py_type_name(vm);
                    return Err(ExcType::type_error(format!(
                        "sequence item {index}: expected a bytes-like object, {t} found"
                    )));
                }
            }
            _ => {
                let t = item.py_type_name(vm);
                return Err(ExcType::type_error(format!(
                    "sequence item {index}: expected a bytes-like object, {t} found"
                )));
            }
        }
        index += 1;
    }

    Ok(allocate_bytes(result, vm.heap))
}

// =============================================================================
// Hex method
// =============================================================================

/// Implements Python's `bytes.hex([sep[, bytes_per_sep]])` method.
///
/// Returns a string containing the hexadecimal representation of the bytes.
fn bytes_hex<'h>(bytes: &impl HeapView<'h, [u8]>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
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

    Ok(super::str::allocate_string(result, vm.heap))
}

/// Parses arguments for bytes.hex method.
fn parse_bytes_hex_args(args: ArgValues, vm: &mut VM<'_>) -> RunResult<(Option<char>, i64)> {
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
pub fn bytes_fromhex(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
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
            let t = hex_value.py_type_name(vm);
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

    Ok(allocate_bytes(result, vm.heap))
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
pub(crate) fn allocate_bytes(bytes: Vec<u8>, heap: &Heap) -> Value {
    let heap_id = heap.allocate(HeapData::Bytes(Bytes::new(bytes)));
    Value::Ref(heap_id)
}

/// Source representation retained by a bytes iterator.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum BytesIteratorSource {
    Intern(BytesId),
    Heap(HeapId),
}

/// Iterator yielding integer elements from bytes storage.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct BytesIterator {
    source: BytesIteratorSource,
    index: usize,
}

impl BytesIterator {
    /// Allocates an iterator over interned bytes.
    pub(crate) fn from_intern(id: BytesId, vm: &mut VM<'_>) -> Value {
        Self::allocate(BytesIteratorSource::Intern(id), vm)
    }

    /// Allocates an iterator retaining heap bytes.
    fn from_heap(id: HeapId, vm: &mut VM<'_>) -> Value {
        Self::allocate(BytesIteratorSource::Heap(id), vm)
    }

    /// Returns a retained heap source, if the bytes are not interned.
    pub(crate) fn source_id(&self) -> Option<HeapId> {
        match self.source {
            BytesIteratorSource::Intern(_) => None,
            BytesIteratorSource::Heap(id) => Some(id),
        }
    }

    /// Returns the number of bytes not yet yielded.
    pub(crate) fn size_hint(&self, vm: &VM<'_>) -> usize {
        self.as_slice(vm).len().saturating_sub(self.index)
    }

    /// Borrows the source bytes.
    fn as_slice<'a>(&self, vm: &'a VM<'_>) -> &'a [u8] {
        match self.source {
            BytesIteratorSource::Intern(id) => vm.interns.get_bytes(id),
            BytesIteratorSource::Heap(id) => match vm.heap.get(id) {
                HeapData::Bytes(bytes) => bytes.as_slice(),
                _ => unreachable!("bytes iterator must retain bytes"),
            },
        }
    }

    /// Allocates an iterator and retains a heap source when present.
    fn allocate(source: BytesIteratorSource, vm: &mut VM<'_>) -> Value {
        let source_id = match source {
            BytesIteratorSource::Heap(id) => Some(id),
            BytesIteratorSource::Intern(_) => None,
        };
        let id = vm.heap.allocate(HeapData::BytesIterator(Self { source, index: 0 }));
        if let Some(source_id) = source_id {
            vm.heap.inc_ref(source_id);
        }
        Value::Ref(id)
    }
}

impl HeapItem for BytesIterator {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(id) = self.source_id() {
            stack.push(id);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, BytesIterator> {
    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::BytesIterator
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let byte = {
            let iter = self.get(vm.heap);
            iter.as_slice(vm).get(iter.index).copied()
        };
        if let Some(byte) = byte {
            self.get_mut(vm.heap).index += 1;
            Ok(Some(Value::Int(i64::from(byte))))
        } else {
            Ok(None)
        }
    }
}

/// Membership for `item in <bytes>`, shared by heap and interned containers.
///
/// CPython accepts a bytes-like probe (substring) or an integer (byte value);
/// an integer outside `range(0, 256)` is a `ValueError`, anything else a `TypeError`.
pub(crate) fn bytes_contains(container: &[u8], item: &Value, vm: &VM<'_>) -> RunResult<bool> {
    // `None` marks an integer probe that cannot be a byte, keeping the range
    // error in one place. A `LongInt` never fits in a `u8` by construction — it
    // exists precisely because the value overflowed `i64`.
    let byte = match item {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::InternLongInt(_) => None,
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)) => None,
        // A bytes-like probe is a substring test.
        Value::InternBytes(_) | Value::Ref(_) => {
            let needle = extract_bytes_only(item, vm)?;
            // An empty needle is in everything, and `find_subsequence` rejects it.
            return Ok(needle.is_empty() || find_subsequence(container, needle, vm.heap)?.is_some());
        }
        other => return Err(ExcType::type_error_bytes_like_required(&other.py_type_name(vm))),
    };
    match byte.and_then(|b| u8::try_from(b).ok()) {
        Some(b) => Ok(container.contains(&b)),
        None => Err(ExcType::value_error_byte_range()),
    }
}
