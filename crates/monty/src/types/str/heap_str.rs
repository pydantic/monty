//! Heap storage for Python strings.

use std::{cell::Cell, ops};

use super::{StrInitArgs, allocate_string, ctor_str_arg, str_ctor_arg_check};
use crate::{
    args::ArgValues,
    bytecode::VM,
    codecs::Codec,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::HashValue,
    heap::{HeapData, HeapId, HeapItem},
    intern::StringId,
    types::PyTrait,
    value::Value,
};

/// Heap-backed Python string with lazy hash and ASCII caches.
/// The text is immutable; caches are omitted from snapshots and recomputed on use.
/// ASCII text has one byte per character, so length and indexing take O(1) time.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct HeapStr(
    Box<str>,
    #[serde(skip)] pub(super) Cell<Option<HashValue>>,
    #[serde(skip)] Cell<Option<bool>>,
);

impl PartialEq for HeapStr {
    /// Compares only the string content — the cached fields are pure
    /// optimisations and must not affect equality.
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl HeapStr {
    /// Creates heap storage; use [`allocate_string`] or [`super::allocate_string_no_interning`] for values.
    #[must_use]
    pub(super) fn new(s: impl Into<Box<str>>) -> Self {
        Self(s.into(), Cell::new(None), Cell::new(None))
    }

    /// Returns a reference to the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether every character is ASCII, scanning the text only on the first call.
    pub fn is_ascii(&self) -> bool {
        if let Some(ascii) = self.2.get() {
            ascii
        } else {
            let ascii = self.0.is_ascii();
            self.2.set(Some(ascii));
            ascii
        }
    }

    /// The length in Unicode code points, as Python's `len()` reports it.
    pub fn char_len(&self) -> usize {
        if self.is_ascii() {
            self.0.len()
        } else {
            self.0.chars().count()
        }
    }

    /// Creates a string from the `str()` constructor call.
    ///
    /// - `str()` with no args returns an empty string (even with `encoding=`)
    /// - `str(x)` converts x to its string representation using `py_str`
    /// - `str(b, encoding, errors)` decodes a bytes object via the codec registry
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let StrInitArgs {
            object,
            encoding,
            errors,
        } = StrInitArgs::from_args(args, vm)?;
        defer_drop!(object, vm);
        defer_drop!(encoding, vm);
        defer_drop!(errors, vm);
        if encoding.is_none() && errors.is_none() {
            return match object {
                None => Ok(Value::InternString(StringId::EMPTY)),
                Some(v) => v.py_str(vm),
            };
        }
        // Decoding mode. Type-check `encoding` then `errors` first — CPython's
        // clinic converters run before the constructor body sees the object.
        str_ctor_arg_check(encoding.as_ref(), "encoding", vm)?;
        str_ctor_arg_check(errors.as_ref(), "errors", vm)?;
        let Some(object) = object else {
            // A missing object wins over the decoding args: `str(encoding='utf-8')` is ''.
            return Ok(Value::InternString(StringId::EMPTY));
        };
        if object.is_str(vm.heap) {
            return Err(ExcType::type_error_decoding_str_not_supported());
        }
        let bytes: &[u8] = match object {
            Value::InternBytes(bytes_id) => vm.interns.get_bytes(*bytes_id),
            Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
                HeapData::Bytes(b) => b.as_slice(),
                _ => return Err(ExcType::type_error_decoding_need_bytes(object.py_type_name(vm))),
            },
            _ => return Err(ExcType::type_error_decoding_need_bytes(object.py_type_name(vm))),
        };
        let encoding = ctor_str_arg(encoding.as_ref(), "utf-8", vm)?;
        let errors = ctor_str_arg(errors.as_ref(), "strict", vm)?;
        let codec = Codec::find(encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(encoding))?;
        let s = codec.decode(bytes, errors)?;
        Ok(allocate_string(s, vm.heap))
    }
}

impl ops::Deref for HeapStr {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl HeapItem for HeapStr {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No-op: strings don't hold Value references
    }
}
