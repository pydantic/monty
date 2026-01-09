/// Python string type, wrapping a Rust `String`.
///
/// This type provides Python string semantics. Currently supports basic
/// operations like length and equality comparison.
use std::borrow::Cow;
use std::fmt::Write;

use ahash::AHashSet;

use super::PyTrait;
use crate::{
    args::ArgValues,
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::Type,
    value::Value,
};

/// Python string value stored on the heap.
///
/// Wraps a Rust `String` and provides Python-compatible operations.
/// `len()` returns the number of Unicode codepoints (characters), matching Python semantics.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Str(String);

impl Str {
    /// Creates a new Str from a Rust String.
    #[must_use]
    pub fn new(s: String) -> Self {
        Self(s)
    }

    /// Returns a reference to the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns a mutable reference to the inner string.
    pub fn as_string_mut(&mut self) -> &mut String {
        &mut self.0
    }

    /// Creates a string from the `str()` constructor call.
    ///
    /// - `str()` with no args returns an empty string
    /// - `str(x)` converts x to its string representation using `py_str`
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let value = args.get_zero_one_arg("str")?;
        match value {
            None => {
                let heap_id = heap.allocate(HeapData::Str(Str::new(String::new())))?;
                Ok(Value::Ref(heap_id))
            }
            Some(v) => {
                let s = v.py_str(heap, interns).into_owned();
                let heap_id = heap.allocate(HeapData::Str(Str::new(s)))?;
                v.drop_with_heap(heap);
                Ok(Value::Ref(heap_id))
            }
        }
    }
}

impl From<String> for Str {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Str {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<Str> for String {
    fn from(value: Str) -> Self {
        value.0
    }
}

impl std::ops::Deref for Str {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PyTrait for Str {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::Str
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.0.len()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        // Count Unicode characters, not bytes, to match Python semantics
        Some(self.0.chars().count())
    }

    fn py_eq(&self, other: &Self, _heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        self.0 == other.0
    }

    /// Interns don't contain nested heap references.
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No-op: strings don't hold Value references
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
        string_repr_fmt(&self.0, f)
    }

    fn py_str(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Cow<'static, str> {
        self.0.clone().into()
    }

    fn py_add(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        let result = format!("{}{}", self.0, other.0);
        let id = heap.allocate(HeapData::Str(result.into()))?;
        Ok(Some(Value::Ref(id)))
    }

    fn py_iadd(
        &mut self,
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        self_id: Option<HeapId>,
        interns: &Interns,
    ) -> Result<bool, crate::resource::ResourceError> {
        match &other {
            Value::Ref(other_id) => {
                if Some(*other_id) == self_id {
                    let rhs = self.0.clone();
                    self.0.push_str(&rhs);
                } else if let HeapData::Str(rhs) = heap.get(*other_id) {
                    self.0.push_str(rhs.as_str());
                } else {
                    return Ok(false);
                }
                // Drop the other value - we've consumed it
                other.drop_with_heap(heap);
                Ok(true)
            }
            Value::InternString(string_id) => {
                self.0.push_str(interns.get_str(*string_id));
                Ok(true)
            }
            _ => Ok(false),
        }
    }
    // py_call_attr uses default implementation which returns AttributeError
}

/// Writes a Python repr() string for a given string slice to a formatter.
///
/// Chooses between single and double quotes based on the string content:
/// - Uses double quotes if the string contains single quotes but not double quotes
/// - Uses single quotes by default, escaping any contained single quotes
///
/// Common escape sequences (backslash, newline, tab, carriage return) are always escaped.
pub fn string_repr_fmt(s: &str, f: &mut impl Write) -> std::fmt::Result {
    // Check if the string contains single quotes but not double quotes
    if s.contains('\'') && !s.contains('"') {
        // Use double quotes if string contains only single quotes
        f.write_char('"')?;
        write_escaped_string(s, f)?;
        f.write_char('"')
    } else {
        // Use single quotes by default, escape any single quotes in the string
        f.write_char('\'')?;
        write_escaped_string_with_single_quote(s, f)?;
        f.write_char('\'')
    }
}

/// Writes the string with common escape sequences replaced.
fn write_escaped_string(s: &str, f: &mut impl Write) -> std::fmt::Result {
    for c in s.chars() {
        match c {
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            '\r' => f.write_str("\\r")?,
            _ => f.write_char(c)?,
        }
    }
    Ok(())
}

/// Writes the string with common escape sequences replaced, plus single quotes escaped.
fn write_escaped_string_with_single_quote(s: &str, f: &mut impl Write) -> std::fmt::Result {
    for c in s.chars() {
        match c {
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            '\r' => f.write_str("\\r")?,
            '\'' => f.write_str("\\'")?,
            _ => f.write_char(c)?,
        }
    }
    Ok(())
}

/// Returns a Python repr() string for a given string slice.
///
/// Convenience wrapper around `string_repr_fmt` that returns an owned String.
#[must_use]
pub fn string_repr(s: &str) -> String {
    let mut result = String::new();
    // Writing to String never fails
    string_repr_fmt(s, &mut result).unwrap();
    result
}
