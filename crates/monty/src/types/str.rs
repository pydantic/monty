use std::fmt::Write;
/// Python string type, wrapping a Rust `String`.
///
/// This type provides Python string semantics. Currently supports basic
/// operations like length and equality comparison.
use std::{borrow::Cow, fmt};

use ahash::AHashSet;
use smallvec::smallvec;

use super::{Bytes, MontyIter, PyTrait};
use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::{ResourceError, ResourceTracker},
    types::Type,
    value::{EitherStr, Value},
};

/// Python string value stored on the heap.
///
/// Wraps a Rust `String` and provides Python-compatible operations.
/// `len()` returns the number of Unicode codepoints (characters), matching Python semantics.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Str(String);

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
        let value = args.get_zero_one_arg("str", heap)?;
        match value {
            None => Ok(Value::InternString(StaticStrings::EmptyString.into())),
            Some(v) => {
                let s = v.py_str(heap, interns).into_owned();
                v.drop_with_heap(heap);
                allocate_string(s, heap)
            }
        }
    }

    /// Handles slice-based indexing for strings.
    ///
    /// Returns a new string containing the selected characters (Unicode-aware).
    fn getitem_slice(&self, slice: &crate::types::Slice, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        let char_count = self.0.chars().count();
        let (start, stop, step) = slice
            .indices(char_count)
            .map_err(|()| ExcType::value_error_slice_step_zero())?;

        let result_str = get_str_slice(&self.0, start, stop, step);
        let heap_id = heap.allocate(HeapData::Str(Self::from(result_str)))?;
        Ok(Value::Ref(heap_id))
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

/// Allocates a string, using interned versions when possible.
///
/// Optimizations:
/// - Empty strings return the pre-interned `StaticStrings::EmptyString`
/// - Single ASCII characters return pre-interned ASCII strings
/// - Other strings are allocated on the heap
///
/// This avoids heap allocation for common cases like results from `strip()`,
/// `split()`, string iteration, etc.
pub fn allocate_string(s: String, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    match s.len() {
        0 => Ok(Value::InternString(StaticStrings::EmptyString.into())),
        1 => {
            // Single byte means single ASCII character
            let byte = s.as_bytes()[0];
            Ok(Value::InternString(StringId::from_ascii(byte)))
        }
        _ => {
            let heap_id = heap.allocate(HeapData::Str(Str::new(s)))?;
            Ok(Value::Ref(heap_id))
        }
    }
}

/// Allocates a single character as a string value.
///
/// ASCII characters use pre-interned strings for efficiency.
/// Non-ASCII characters are allocated on the heap.
///
/// This is used by string iteration and `chr()` builtin.
pub fn allocate_char(c: char, heap: &mut Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    if c.is_ascii() {
        Ok(Value::InternString(StringId::from_ascii(c as u8)))
    } else {
        let heap_id = heap.allocate(HeapData::Str(Str::new(c.to_string())))?;
        Ok(Value::Ref(heap_id))
    }
}

/// Gets the character at a given index in a string, handling negative indices.
///
/// Returns `None` if the index is out of bounds. This uses a single-pass scan
/// to avoid allocating a `Vec<char>`.
///
/// Negative indices count from the end: -1 is the last character.
pub fn get_char_at_index(s: &str, index: i64) -> Option<char> {
    let char_count = s.chars().count();
    let len = i64::try_from(char_count).ok()?;
    let normalized = if index < 0 { index + len } else { index };

    if normalized < 0 || normalized >= len {
        return None;
    }

    let idx = usize::try_from(normalized).ok()?;
    s.chars().nth(idx)
}

/// Extracts a slice of a string (Unicode-aware).
///
/// Handles both positive and negative step values. For negative step,
/// iterates backward from start down to (but not including) stop.
/// The `stop` parameter uses a sentinel value of `len + 1` for negative
/// step to indicate "go to the beginning".
///
/// Note: step must be non-zero (callers should validate this via `slice.indices()`).
pub(crate) fn get_str_slice(s: &str, start: usize, stop: usize, step: i64) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::new();

    // try_from succeeds for non-negative step; step==0 rejected upstream by slice.indices()
    if let Ok(step_usize) = usize::try_from(step) {
        // Positive step: iterate forward
        let mut i = start;
        while i < stop && i < chars.len() {
            result.push(chars[i]);
            i += step_usize;
        }
    } else {
        // Negative step: iterate backward
        // start is the highest index, stop is the sentinel
        // stop > chars.len() means "go to the beginning"
        let step_abs = usize::try_from(-step).expect("step is negative so -step is positive");
        let step_abs_i64 = i64::try_from(step_abs).expect("step magnitude fits in i64");
        let mut i = i64::try_from(start).expect("start index fits in i64");
        // stop > chars.len() is sentinel meaning "go to beginning", use -1
        let stop_i64 = if stop > chars.len() {
            -1
        } else {
            i64::try_from(stop).expect("stop bounded by chars.len() fits in i64")
        };

        while let Ok(i_usize) = usize::try_from(i) {
            if i_usize >= chars.len() || i <= stop_i64 {
                break;
            }
            result.push(chars[i_usize]);
            i -= step_abs_i64;
        }
    }

    result
}

impl std::ops::Deref for Str {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PyTrait for Str {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Str
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.0.len()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        // Count Unicode characters, not bytes, to match Python semantics
        Some(self.0.chars().count())
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = heap.get(*id)
        {
            // Clone the slice to release the borrow on heap before calling getitem_slice
            let slice = slice.clone();
            return self.getitem_slice(&slice, heap);
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(heap, Type::Str)?;

        // Use single-pass indexing to avoid Vec<char> allocation
        let c = get_char_at_index(&self.0, index).ok_or_else(ExcType::str_index_error)?;
        Ok(allocate_char(c, heap)?)
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
    ) -> fmt::Result {
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

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(heap);
            return Err(ExcType::attribute_error(Type::Str, attr.as_str(interns)));
        };

        call_str_method_impl(&self.0, method, args, heap, interns)
    }
}

/// Dispatches a method call on a string value by method name.
///
/// This is the entry point for string method calls from the VM on interned strings.
/// Converts the `StringId` to `StaticStrings` and delegates to `call_str_method_impl`.
pub fn call_str_method(
    s: &str,
    method_id: StringId,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let Some(method) = StaticStrings::from_string_id(method_id) else {
        args.drop_with_heap(heap);
        return Err(ExcType::attribute_error(Type::Str, interns.get_str(method_id)));
    };
    call_str_method_impl(s, method, args, heap, interns)
}

/// Dispatches a method call on a string value.
///
/// This is the unified implementation for string method calls, used by both:
/// - `Str::py_call_attr()` for heap-allocated strings
/// - `call_str_method()` for interned string literals from the VM
///
/// # Not Yet Implemented
///
/// The following Python string methods are not yet implemented:
///
/// - `format()` - Requires implementing the format spec mini-language (PEP 3101),
///   which is complex and involves parsing format specifications like `{:>10.2f}`.
/// - `format_map(mapping)` - Similar to `format()` but takes a mapping; depends on
///   `format()` implementation.
/// - `maketrans()` / `translate()` - Character translation tables; moderate complexity,
///   requires building and applying Unicode translation maps.
/// - `expandtabs(tabsize=8)` - Tab expansion; simple but rarely used in practice.
/// - `isprintable()` - Checks if all characters are printable; requires accurate Unicode
///   category data for the "printable" property.
fn call_str_method_impl(
    s: &str,
    method: StaticStrings,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    match method {
        // Simple transformations (no arguments)
        StaticStrings::Lower => {
            args.check_zero_args("str.lower", heap)?;
            str_lower(s, heap)
        }
        StaticStrings::Upper => {
            args.check_zero_args("str.upper", heap)?;
            str_upper(s, heap)
        }
        StaticStrings::Capitalize => {
            args.check_zero_args("str.capitalize", heap)?;
            str_capitalize(s, heap)
        }
        StaticStrings::Title => {
            args.check_zero_args("str.title", heap)?;
            str_title(s, heap)
        }
        StaticStrings::Swapcase => {
            args.check_zero_args("str.swapcase", heap)?;
            str_swapcase(s, heap)
        }
        StaticStrings::Casefold => {
            args.check_zero_args("str.casefold", heap)?;
            str_casefold(s, heap)
        }
        // Predicate methods (no arguments, return bool)
        StaticStrings::Isalpha => {
            args.check_zero_args("str.isalpha", heap)?;
            Ok(Value::Bool(str_isalpha(s)))
        }
        StaticStrings::Isdigit => {
            args.check_zero_args("str.isdigit", heap)?;
            Ok(Value::Bool(str_isdigit(s)))
        }
        StaticStrings::Isalnum => {
            args.check_zero_args("str.isalnum", heap)?;
            Ok(Value::Bool(str_isalnum(s)))
        }
        StaticStrings::Isnumeric => {
            args.check_zero_args("str.isnumeric", heap)?;
            Ok(Value::Bool(str_isnumeric(s)))
        }
        StaticStrings::Isspace => {
            args.check_zero_args("str.isspace", heap)?;
            Ok(Value::Bool(str_isspace(s)))
        }
        StaticStrings::Islower => {
            args.check_zero_args("str.islower", heap)?;
            Ok(Value::Bool(str_islower(s)))
        }
        StaticStrings::Isupper => {
            args.check_zero_args("str.isupper", heap)?;
            Ok(Value::Bool(str_isupper(s)))
        }
        StaticStrings::Isascii => {
            args.check_zero_args("str.isascii", heap)?;
            Ok(Value::Bool(s.is_ascii()))
        }
        StaticStrings::Isdecimal => {
            args.check_zero_args("str.isdecimal", heap)?;
            Ok(Value::Bool(str_isdecimal(s)))
        }
        // Search methods
        StaticStrings::Find => str_find(s, args, heap, interns),
        StaticStrings::Rfind => str_rfind(s, args, heap, interns),
        StaticStrings::Index => str_index(s, args, heap, interns),
        StaticStrings::Rindex => str_rindex(s, args, heap, interns),
        StaticStrings::Count => str_count(s, args, heap, interns),
        StaticStrings::Startswith => str_startswith(s, args, heap, interns),
        StaticStrings::Endswith => str_endswith(s, args, heap, interns),
        // Strip/trim methods
        StaticStrings::Strip => str_strip(s, args, heap, interns),
        StaticStrings::Lstrip => str_lstrip(s, args, heap, interns),
        StaticStrings::Rstrip => str_rstrip(s, args, heap, interns),
        StaticStrings::Removeprefix => str_removeprefix(s, args, heap, interns),
        StaticStrings::Removesuffix => str_removesuffix(s, args, heap, interns),
        // Split methods
        StaticStrings::Split => str_split(s, args, heap, interns),
        StaticStrings::Rsplit => str_rsplit(s, args, heap, interns),
        StaticStrings::Splitlines => str_splitlines(s, args, heap, interns),
        StaticStrings::Partition => str_partition(s, args, heap, interns),
        StaticStrings::Rpartition => str_rpartition(s, args, heap, interns),
        // Replace/modify methods
        StaticStrings::Replace => str_replace(s, args, heap, interns),
        StaticStrings::Center => str_center(s, args, heap, interns),
        StaticStrings::Ljust => str_ljust(s, args, heap, interns),
        StaticStrings::Rjust => str_rjust(s, args, heap, interns),
        StaticStrings::Zfill => str_zfill(s, args, heap),
        // Additional methods
        StaticStrings::Encode => str_encode(s, args, heap, interns),
        StaticStrings::Isidentifier => {
            args.check_zero_args("str.isidentifier", heap)?;
            Ok(Value::Bool(str_isidentifier(s)))
        }
        StaticStrings::Istitle => {
            args.check_zero_args("str.istitle", heap)?;
            Ok(Value::Bool(str_istitle(s)))
        }
        // Existing method
        StaticStrings::Join => {
            let iterable = args.get_one_arg("str.join", heap)?;
            str_join(s, iterable, heap, interns)
        }
        _ => {
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error(Type::Str, method.into()))
        }
    }
}

/// Implements Python's `str.join(iterable)` method.
///
/// Joins elements of the iterable with the separator string, returning
/// a new heap-allocated string. Each element must be a string.
///
/// # Arguments
/// * `separator` - The separator string (e.g., "," for comma-separated)
/// * `iterable` - The iterable containing string elements to join
/// * `heap` - The heap for allocation and reference counting
/// * `interns` - The interns table for resolving interned strings
///
/// # Errors
/// Returns `TypeError` if the argument is not iterable or if any element is not a string.
fn str_join(
    separator: &str,
    iterable: Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    // Create MontyIter from the iterable, with join-specific error message
    let Ok(mut iter) = MontyIter::new(iterable, heap, interns) else {
        return Err(ExcType::type_error_join_not_iterable());
    };

    // Build result string, tracking index for error messages
    let mut result = String::new();
    let mut index = 0usize;

    // Use explicit match to properly drop iter on for_next errors (e.g., dict/set size change
    // during iteration, or allocation failure). The `?` operator would leak the iterator.
    loop {
        let item = match iter.for_next(heap, interns) {
            Ok(Some(item)) => item,
            Ok(None) => break,
            Err(e) => {
                iter.drop_with_heap(heap);
                return Err(e);
            }
        };

        if index > 0 {
            result.push_str(separator);
        }

        // Check item is a string and extract its content
        match &item {
            Value::InternString(id) => {
                result.push_str(interns.get_str(*id));
                item.drop_with_heap(heap); // No-op for InternString but consistent
            }
            Value::Ref(heap_id) => {
                if let HeapData::Str(s) = heap.get(*heap_id) {
                    result.push_str(s.as_str());
                    item.drop_with_heap(heap);
                } else {
                    let t = item.py_type(heap);
                    item.drop_with_heap(heap);
                    iter.drop_with_heap(heap);
                    return Err(ExcType::type_error_join_item(index, t));
                }
            }
            _ => {
                let t = item.py_type(heap);
                item.drop_with_heap(heap);
                iter.drop_with_heap(heap);
                return Err(ExcType::type_error_join_item(index, t));
            }
        }

        index += 1;
    }

    iter.drop_with_heap(heap);

    // Allocate result (uses interned empty string if result is empty)
    allocate_string(result, heap)
}

/// Writes a Python repr() string for a given string slice to a formatter.
///
/// Chooses between single and double quotes based on the string content:
/// - Uses double quotes if the string contains single quotes but not double quotes
/// - Uses single quotes by default, escaping any contained single quotes
///
/// Common escape sequences (backslash, newline, tab, carriage return) are always escaped.
pub fn string_repr_fmt(s: &str, f: &mut impl Write) -> fmt::Result {
    // Check if the string contains single quotes but not double quotes
    if s.contains('\'') && !s.contains('"') {
        // Use double quotes if string contains only single quotes
        f.write_char('"')?;
        for c in s.chars() {
            match c {
                '\\' => f.write_str("\\\\")?,
                '\n' => f.write_str("\\n")?,
                '\t' => f.write_str("\\t")?,
                '\r' => f.write_str("\\r")?,
                _ => f.write_char(c)?,
            }
        }
        f.write_char('"')
    } else {
        // Use single quotes by default, escape any single quotes in the string
        f.write_char('\'')?;
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
        f.write_char('\'')
    }
}

/// Formatter for a Python repr() string.
#[derive(Debug)]
pub struct StringRepr<'a>(pub &'a str);

impl fmt::Display for StringRepr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        string_repr_fmt(self.0, f)
    }
}

// =============================================================================
// Simple transformations (no arguments)
// =============================================================================

/// Implements Python's `str.lower()` method.
fn str_lower(s: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    allocate_string(s.to_lowercase(), heap)
}

/// Implements Python's `str.upper()` method.
fn str_upper(s: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    allocate_string(s.to_uppercase(), heap)
}

/// Implements Python's `str.capitalize()` method.
///
/// Returns a copy of the string with its first character capitalized and the rest lowercased.
fn str_capitalize(s: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    let mut chars = s.chars();
    let result = match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut result = first.to_uppercase().to_string();
            for c in chars {
                result.extend(c.to_lowercase());
            }
            result
        }
    };
    allocate_string(result, heap)
}

/// Implements Python's `str.title()` method.
///
/// Returns a titlecased version of the string where words start with an uppercase
/// character and the remaining characters are lowercase.
fn str_title(s: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    let mut result = String::with_capacity(s.len());
    let mut prev_is_cased = false;

    for c in s.chars() {
        if prev_is_cased {
            result.extend(c.to_lowercase());
        } else {
            result.extend(c.to_uppercase());
        }
        prev_is_cased = c.is_alphabetic();
    }

    allocate_string(result, heap)
}

/// Implements Python's `str.swapcase()` method.
///
/// Returns a copy of the string with uppercase characters converted to lowercase and vice versa.
fn str_swapcase(s: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    let mut result = String::with_capacity(s.len());

    for c in s.chars() {
        if c.is_uppercase() {
            result.extend(c.to_lowercase());
        } else if c.is_lowercase() {
            result.extend(c.to_uppercase());
        } else {
            result.push(c);
        }
    }

    allocate_string(result, heap)
}

/// Implements Python's `str.casefold()` method.
///
/// Returns a casefolded copy of the string. Casefolding is similar to lowercasing
/// but more aggressive because it is intended for caseless string matching.
fn str_casefold(s: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    // Rust's to_lowercase() is equivalent to Unicode casefolding for most purposes
    allocate_string(s.to_lowercase(), heap)
}

// =============================================================================
// Predicate methods (no arguments, return bool)
// =============================================================================

/// Implements Python's `str.isalpha()` method.
///
/// Returns True if all characters in the string are alphabetic and there is at least one character.
fn str_isalpha(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_alphabetic)
}

/// Implements Python's `str.isdigit()` method.
///
/// Returns True if all characters in the string are digits and there is at least one character.
/// In Python, digits include decimal digits (Nd) plus characters with Numeric_Type=Digit
/// (superscripts, subscripts, circled digits, etc.).
fn str_isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_unicode_digit)
}

/// Implements Python's `str.isalnum()` method.
///
/// Returns True if all characters in the string are alphanumeric and there is at least one character.
fn str_isalnum(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_alphanumeric)
}

/// Implements Python's `str.isnumeric()` method.
///
/// Returns True if all characters in the string are numeric and there is at least one character.
/// In Python, numeric includes decimal digits (Nd), letter numerals (Nl), and other numerals (No).
/// Rust's `char::is_numeric()` checks for all of these categories.
fn str_isnumeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_numeric)
}

/// Implements Python's `str.isspace()` method.
///
/// Returns True if all characters in the string are whitespace and there is at least one character.
fn str_isspace(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_whitespace)
}

/// Implements Python's `str.islower()` method.
///
/// Returns True if all cased characters in the string are lowercase and there is at least one cased character.
fn str_islower(s: &str) -> bool {
    let mut has_cased = false;
    for c in s.chars() {
        if c.is_uppercase() {
            return false;
        }
        if c.is_lowercase() {
            has_cased = true;
        }
    }
    has_cased
}

/// Implements Python's `str.isupper()` method.
///
/// Returns True if all cased characters in the string are uppercase and there is at least one cased character.
fn str_isupper(s: &str) -> bool {
    let mut has_cased = false;
    for c in s.chars() {
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            has_cased = true;
        }
    }
    has_cased
}

/// Implements Python's `str.isdecimal()` method.
///
/// Returns True if all characters in the string are decimal characters and there is at least one character.
/// Decimal characters are those in Unicode category Nd (Decimal_Number) - digits that can be used
/// to form numbers in base 10.
fn str_isdecimal(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_unicode_decimal)
}

/// Checks if a character is a Unicode decimal digit (Nd category).
///
/// This covers decimal digit ranges from various scripts including ASCII, Arabic-Indic,
/// Devanagari, Bengali, Thai, Fullwidth, and many others.
fn is_unicode_decimal(c: char) -> bool {
    let cp = c as u32;
    matches!(
        cp,
        // Basic Latin (ASCII digits)
        0x0030..=0x0039
        // Arabic-Indic digits
        | 0x0660..=0x0669
        // Extended Arabic-Indic digits
        | 0x06F0..=0x06F9
        // NKo digits
        | 0x07C0..=0x07C9
        // Devanagari digits
        | 0x0966..=0x096F
        // Bengali digits
        | 0x09E6..=0x09EF
        // Gurmukhi digits
        | 0x0A66..=0x0A6F
        // Gujarati digits
        | 0x0AE6..=0x0AEF
        // Oriya digits
        | 0x0B66..=0x0B6F
        // Tamil digits
        | 0x0BE6..=0x0BEF
        // Telugu digits
        | 0x0C66..=0x0C6F
        // Kannada digits
        | 0x0CE6..=0x0CEF
        // Malayalam digits
        | 0x0D66..=0x0D6F
        // Sinhala Lith digits
        | 0x0DE6..=0x0DEF
        // Thai digits
        | 0x0E50..=0x0E59
        // Lao digits
        | 0x0ED0..=0x0ED9
        // Tibetan digits
        | 0x0F20..=0x0F29
        // Myanmar digits
        | 0x1040..=0x1049
        // Myanmar Shan digits
        | 0x1090..=0x1099
        // Khmer digits
        | 0x17E0..=0x17E9
        // Mongolian digits
        | 0x1810..=0x1819
        // Limbu digits
        | 0x1946..=0x194F
        // New Tai Lue digits
        | 0x19D0..=0x19D9
        // Tai Tham Hora digits
        | 0x1A80..=0x1A89
        // Tai Tham Tham digits
        | 0x1A90..=0x1A99
        // Balinese digits
        | 0x1B50..=0x1B59
        // Sundanese digits
        | 0x1BB0..=0x1BB9
        // Lepcha digits
        | 0x1C40..=0x1C49
        // Ol Chiki digits
        | 0x1C50..=0x1C59
        // Vai digits
        | 0xA620..=0xA629
        // Saurashtra digits
        | 0xA8D0..=0xA8D9
        // Kayah Li digits
        | 0xA900..=0xA909
        // Javanese digits
        | 0xA9D0..=0xA9D9
        // Myanmar Tai Laing digits
        | 0xA9F0..=0xA9F9
        // Cham digits
        | 0xAA50..=0xAA59
        // Meetei Mayek digits
        | 0xABF0..=0xABF9
        // Fullwidth digits
        | 0xFF10..=0xFF19
        // Osmanya digits
        | 0x104A0..=0x104A9
        // Hanifi Rohingya digits
        | 0x10D30..=0x10D39
        // Brahmi digits
        | 0x11066..=0x1106F
        // Sora Sompeng digits
        | 0x110F0..=0x110F9
        // Chakma digits
        | 0x11136..=0x1113F
        // Sharada digits
        | 0x111D0..=0x111D9
        // Khudawadi digits
        | 0x112F0..=0x112F9
        // Newa digits
        | 0x11450..=0x11459
        // Tirhuta digits
        | 0x114D0..=0x114D9
        // Modi digits
        | 0x11650..=0x11659
        // Takri digits
        | 0x116C0..=0x116C9
        // Ahom digits
        | 0x11730..=0x11739
        // Warang Citi digits
        | 0x118E0..=0x118E9
        // Dives Akuru digits
        | 0x11950..=0x11959
        // Bhaiksuki digits
        | 0x11C50..=0x11C59
        // Masaram Gondi digits
        | 0x11D50..=0x11D59
        // Gunjala Gondi digits
        | 0x11DA0..=0x11DA9
        // Adlam digits
        | 0x1E950..=0x1E959
        // Segmented digits
        | 0x1FBF0..=0x1FBF9
    )
}

/// Checks if a character is a Unicode digit (isdigit).
///
/// This includes decimal digits (Nd) plus characters with Numeric_Type=Digit
/// such as superscripts, subscripts, and circled digits.
fn is_unicode_digit(c: char) -> bool {
    // First check if it's a decimal digit
    if is_unicode_decimal(c) {
        return true;
    }

    let cp = c as u32;
    matches!(
        cp,
        // Superscripts (², ³)
        0x00B2..=0x00B3
        // Superscript 1
        | 0x00B9
        // Superscript digits 0, 4-9
        | 0x2070
        | 0x2074..=0x2079
        // Subscript digits 0-9
        | 0x2080..=0x2089
        // Circled digits 1-9
        | 0x2460..=0x2468
        // Circled digit 0
        | 0x24EA
        // Circled digits 10-20
        | 0x2469..=0x2473
        // Parenthesized digits 1-9
        | 0x2474..=0x247C
        // Period digits 1-9
        | 0x2488..=0x2490
        // Double circled digits 1-10
        | 0x24F5..=0x24FE
        // Dingbat circled sans-serif digits 1-10
        | 0x2780..=0x2789
        // Dingbat negative circled digits 1-10
        | 0x278A..=0x2793
        // Dingbat circled sans-serif digits 1-10
        | 0x24FF
        // Fullwidth digit zero (already in decimal, but include for completeness)
        // | 0xFF10..=0xFF19  // Already covered by is_unicode_decimal
    )
}

// =============================================================================
// Search methods
// =============================================================================

/// Implements Python's `str.find(sub, start?, end?)` method.
///
/// Returns the lowest index in the string where substring sub is found within
/// the slice s[start:end]. Returns -1 if sub is not found.
fn str_find(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (sub, start, end) = parse_search_args("str.find", s, args, heap, interns)?;
    let slice = slice_string(s, start, end);
    let result = match slice.find(&sub) {
        Some(pos) => {
            // Convert byte offset to char offset, then add start offset
            let char_pos = slice[..pos].chars().count();
            i64::try_from(start + char_pos).unwrap_or(i64::MAX)
        }
        None => -1,
    };
    Ok(Value::Int(result))
}

/// Implements Python's `str.rfind(sub, start?, end?)` method.
///
/// Returns the highest index in the string where substring sub is found within
/// the slice s[start:end]. Returns -1 if sub is not found.
fn str_rfind(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (sub, start, end) = parse_search_args("str.rfind", s, args, heap, interns)?;
    let slice = slice_string(s, start, end);
    let result = match slice.rfind(&sub) {
        Some(pos) => {
            // Convert byte offset to char offset, then add start offset
            let char_pos = slice[..pos].chars().count();
            i64::try_from(start + char_pos).unwrap_or(i64::MAX)
        }
        None => -1,
    };
    Ok(Value::Int(result))
}

/// Implements Python's `str.index(sub, start?, end?)` method.
///
/// Like find(), but raises ValueError when the substring is not found.
fn str_index(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (sub, start, end) = parse_search_args("str.index", s, args, heap, interns)?;
    let slice = slice_string(s, start, end);
    match slice.find(&sub) {
        Some(pos) => {
            let char_pos = slice[..pos].chars().count();
            let result = i64::try_from(start + char_pos).unwrap_or(i64::MAX);
            Ok(Value::Int(result))
        }
        None => Err(ExcType::value_error_substring_not_found()),
    }
}

/// Implements Python's `str.rindex(sub, start?, end?)` method.
///
/// Like rfind(), but raises ValueError when the substring is not found.
fn str_rindex(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (sub, start, end) = parse_search_args("str.rindex", s, args, heap, interns)?;
    let slice = slice_string(s, start, end);
    match slice.rfind(&sub) {
        Some(pos) => {
            let char_pos = slice[..pos].chars().count();
            let result = i64::try_from(start + char_pos).unwrap_or(i64::MAX);
            Ok(Value::Int(result))
        }
        None => Err(ExcType::value_error_substring_not_found()),
    }
}

/// Implements Python's `str.count(sub, start?, end?)` method.
///
/// Returns the number of non-overlapping occurrences of substring sub in
/// the string s[start:end].
fn str_count(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (sub, start, end) = parse_search_args("str.count", s, args, heap, interns)?;
    let slice = slice_string(s, start, end);
    let count = if sub.is_empty() {
        // Empty string matches between every character, plus start and end
        slice.chars().count() + 1
    } else {
        slice.matches(&sub).count()
    };
    let result = i64::try_from(count).unwrap_or(i64::MAX);
    Ok(Value::Int(result))
}

/// Implements Python's `str.startswith(prefix, start?, end?)` method.
///
/// Returns True if the string starts with the prefix, otherwise returns False.
/// The prefix argument can be a string or a tuple of strings.
fn str_startswith(
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (prefixes, start, end) = parse_prefix_suffix_args("str.startswith", s, args, heap, interns)?;
    let slice = slice_string(s, start, end);
    let result = prefixes.iter().any(|prefix| slice.starts_with(prefix));
    Ok(Value::Bool(result))
}

/// Implements Python's `str.endswith(suffix, start?, end?)` method.
///
/// Returns True if the string ends with the suffix, otherwise returns False.
/// The suffix argument can be a string or a tuple of strings.
fn str_endswith(
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let (suffixes, start, end) = parse_prefix_suffix_args("str.endswith", s, args, heap, interns)?;
    let slice = slice_string(s, start, end);
    let result = suffixes.iter().any(|suffix| slice.ends_with(suffix));
    Ok(Value::Bool(result))
}

/// Parses arguments for search methods (find, rfind, index, rindex, count, startswith, endswith).
///
/// Returns (substring, start, end) where start and end are character indices.
fn parse_search_args(
    method: &str,
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(String, usize, usize)> {
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
        // Drop remaining values
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

    // Extract substring
    let sub = extract_string_arg(&sub_value, heap, interns)?;
    sub_value.drop_with_heap(heap);

    // Extract start (default 0, None means default)
    let str_len = s.chars().count();
    let start = if let Some(v) = start_value {
        if matches!(v, Value::None) {
            v.drop_with_heap(heap);
            0
        } else {
            let result = extract_int_arg(&v, heap)?;
            v.drop_with_heap(heap);
            normalize_index(result, str_len)
        }
    } else {
        0
    };

    // Extract end (default len, None means default)
    let end = if let Some(v) = end_value {
        if matches!(v, Value::None) {
            v.drop_with_heap(heap);
            str_len
        } else {
            let result = extract_int_arg(&v, heap)?;
            v.drop_with_heap(heap);
            normalize_index(result, str_len)
        }
    } else {
        str_len
    };

    Ok((sub, start, end))
}

/// Parses arguments for startswith/endswith methods.
///
/// Returns (prefixes/suffixes as Vec, start, end) where start and end are character indices.
/// The first argument can be either a string or a tuple of strings.
fn parse_prefix_suffix_args(
    method: &str,
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(Vec<String>, usize, usize)> {
    let (pos, kwargs) = args.into_parts();
    if !kwargs.is_empty() {
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_no_kwargs(method));
    }

    let mut pos_iter = pos;
    let prefix_value = pos_iter
        .next()
        .ok_or_else(|| ExcType::type_error_at_least(method, 1, 0))?;
    let start_value = pos_iter.next();
    let end_value = pos_iter.next();

    // Check no extra arguments
    if pos_iter.next().is_some() {
        // Drop remaining values
        for v in pos_iter {
            v.drop_with_heap(heap);
        }
        prefix_value.drop_with_heap(heap);
        if let Some(v) = start_value {
            v.drop_with_heap(heap);
        }
        if let Some(v) = end_value {
            v.drop_with_heap(heap);
        }
        return Err(ExcType::type_error_at_most(method, 3, 4));
    }

    // Extract prefix/suffix - can be a string or tuple of strings
    let prefixes = extract_str_or_tuple_of_str(&prefix_value, heap, interns)?;
    prefix_value.drop_with_heap(heap);

    // Extract start (default 0, None means default)
    let str_len = s.chars().count();
    let start = if let Some(v) = start_value {
        if matches!(v, Value::None) {
            v.drop_with_heap(heap);
            0
        } else {
            let result = extract_int_arg(&v, heap)?;
            v.drop_with_heap(heap);
            normalize_index(result, str_len)
        }
    } else {
        0
    };

    // Extract end (default len, None means default)
    let end = if let Some(v) = end_value {
        if matches!(v, Value::None) {
            v.drop_with_heap(heap);
            str_len
        } else {
            let result = extract_int_arg(&v, heap)?;
            v.drop_with_heap(heap);
            normalize_index(result, str_len)
        }
    } else {
        str_len
    };

    Ok((prefixes, start, end))
}

/// Extracts a string or tuple of strings from a Value.
///
/// Returns a Vec of strings - a single-element Vec if given a string,
/// or multiple elements if given a tuple of strings.
fn extract_str_or_tuple_of_str(
    value: &Value,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Vec<String>> {
    match value {
        Value::InternString(id) => Ok(vec![interns.get_str(*id).to_owned()]),
        Value::Ref(heap_id) => match heap.get(*heap_id) {
            HeapData::Str(s) => Ok(vec![s.as_str().to_owned()]),
            HeapData::Tuple(tuple) => {
                let items = tuple.as_vec();
                let mut strings = Vec::with_capacity(items.len());
                for item in items {
                    let s = extract_string_arg(item, heap, interns)?;
                    strings.push(s);
                }
                Ok(strings)
            }
            _ => Err(ExcType::type_error("expected str or tuple of str")),
        },
        _ => Err(ExcType::type_error("expected str or tuple of str")),
    }
}

/// Extracts a string from a Value, returning an error if not a string.
fn extract_string_arg(value: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<String> {
    match value {
        Value::InternString(id) => Ok(interns.get_str(*id).to_owned()),
        Value::Ref(heap_id) => {
            if let HeapData::Str(s) = heap.get(*heap_id) {
                Ok(s.as_str().to_owned())
            } else {
                Err(ExcType::type_error("expected str"))
            }
        }
        _ => Err(ExcType::type_error("expected str")),
    }
}

/// Extracts an integer from a Value, returning an error if not an integer.
fn extract_int_arg(value: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<i64> {
    match value {
        Value::Int(i) => Ok(*i),
        Value::Ref(heap_id) => {
            if let HeapData::LongInt(li) = heap.get(*heap_id) {
                // Try to convert to i64
                li.to_i64().ok_or_else(|| ExcType::type_error("integer too large"))
            } else {
                Err(ExcType::type_error("expected int"))
            }
        }
        _ => Err(ExcType::type_error("expected int")),
    }
}

/// Normalizes a Python-style index to a valid index in range [0, len].
fn normalize_index(index: i64, len: usize) -> usize {
    if index < 0 {
        // Safe cast: we've checked index is negative, so -index is positive
        // For very large negative numbers that don't fit in usize, saturate to usize::MAX
        let abs_index = usize::try_from(-index).unwrap_or(usize::MAX);
        len.saturating_sub(abs_index)
    } else {
        // Safe cast: we've checked index is non-negative
        // For values > usize::MAX, saturate to len
        usize::try_from(index).unwrap_or(len).min(len)
    }
}

/// Returns a substring of s from character index start to end.
fn slice_string(s: &str, start: usize, end: usize) -> &str {
    if start >= end {
        return "";
    }

    let mut start_byte = s.len();
    let mut end_byte = s.len();

    for (char_idx, (byte_idx, _)) in s.char_indices().enumerate() {
        if char_idx == start {
            start_byte = byte_idx;
        }
        if char_idx == end {
            end_byte = byte_idx;
            break;
        }
    }

    &s[start_byte..end_byte]
}

// =============================================================================
// Strip/trim methods
// =============================================================================

/// Implements Python's `str.strip(chars?)` method.
///
/// Returns a copy of the string with leading and trailing characters removed.
/// If chars is not specified, whitespace characters are removed.
fn str_strip(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let chars = parse_strip_arg("str.strip", args, heap, interns)?;
    let result = match &chars {
        Some(c) => s.trim_matches(|ch| c.contains(ch)).to_owned(),
        None => s.trim().to_owned(),
    };
    allocate_string(result, heap)
}

/// Implements Python's `str.lstrip(chars?)` method.
///
/// Returns a copy of the string with leading characters removed.
fn str_lstrip(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let chars = parse_strip_arg("str.lstrip", args, heap, interns)?;
    let result = match &chars {
        Some(c) => s.trim_start_matches(|ch| c.contains(ch)).to_owned(),
        None => s.trim_start().to_owned(),
    };
    allocate_string(result, heap)
}

/// Implements Python's `str.rstrip(chars?)` method.
///
/// Returns a copy of the string with trailing characters removed.
fn str_rstrip(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let chars = parse_strip_arg("str.rstrip", args, heap, interns)?;
    let result = match &chars {
        Some(c) => s.trim_end_matches(|ch| c.contains(ch)).to_owned(),
        None => s.trim_end().to_owned(),
    };
    allocate_string(result, heap)
}

/// Parses the optional chars argument for strip methods.
///
/// Accepts None as a value meaning "use default whitespace stripping".
fn parse_strip_arg(
    method: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Option<String>> {
    let value = args.get_zero_one_arg(method, heap)?;
    match value {
        None => Ok(None),
        Some(Value::None) => Ok(None), // Explicit None means default whitespace
        Some(v) => {
            let result = extract_string_arg(&v, heap, interns)?;
            v.drop_with_heap(heap);
            Ok(Some(result))
        }
    }
}

/// Implements Python's `str.removeprefix(prefix)` method.
///
/// If the string starts with the prefix string, return string[len(prefix):].
/// Otherwise, return a copy of the original string.
fn str_removeprefix(
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let prefix_value = args.get_one_arg("str.removeprefix", heap)?;
    let prefix = extract_string_arg(&prefix_value, heap, interns)?;
    prefix_value.drop_with_heap(heap);

    let result = s.strip_prefix(&prefix).unwrap_or(s).to_owned();
    allocate_string(result, heap)
}

/// Implements Python's `str.removesuffix(suffix)` method.
///
/// If the string ends with the suffix string, return string[:-len(suffix)].
/// Otherwise, return a copy of the original string.
fn str_removesuffix(
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let suffix_value = args.get_one_arg("str.removesuffix", heap)?;
    let suffix = extract_string_arg(&suffix_value, heap, interns)?;
    suffix_value.drop_with_heap(heap);

    let result = s.strip_suffix(&suffix).unwrap_or(s).to_owned();
    allocate_string(result, heap)
}

// =============================================================================
// Split methods
// =============================================================================

/// Implements Python's `str.split(sep?, maxsplit?)` method.
///
/// Returns a list of the words in the string, using sep as the delimiter string.
fn str_split(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (sep, maxsplit) = parse_split_args("str.split", args, heap, interns)?;

    let parts: Vec<&str> = match &sep {
        Some(sep) => {
            // Empty separator raises ValueError
            if sep.is_empty() {
                return Err(ExcType::value_error_empty_separator());
            }
            if maxsplit < 0 {
                s.split(sep.as_str()).collect()
            } else {
                // Safe cast: we've checked maxsplit >= 0
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                s.splitn(max.saturating_add(1), sep.as_str()).collect()
            }
        }
        None => {
            // Split on whitespace, filtering empty strings
            if maxsplit < 0 {
                s.split_whitespace().collect()
            } else {
                // Safe cast: we've checked maxsplit >= 0
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                split_whitespace_n(s, max)
            }
        }
    };

    // Convert to list of strings (using interned empty string when applicable)
    let mut list_items = Vec::with_capacity(parts.len());
    for part in parts {
        list_items.push(allocate_string(part.to_owned(), heap)?);
    }

    let list = crate::types::List::new(list_items);
    let heap_id = heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `str.rsplit(sep?, maxsplit?)` method.
///
/// Returns a list of the words in the string, using sep as the delimiter string,
/// splitting from the right.
fn str_rsplit(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (sep, maxsplit) = parse_split_args("str.rsplit", args, heap, interns)?;

    let parts: Vec<&str> = match &sep {
        Some(sep) => {
            // Empty separator raises ValueError
            if sep.is_empty() {
                return Err(ExcType::value_error_empty_separator());
            }
            if maxsplit < 0 {
                s.rsplit(sep.as_str()).collect::<Vec<_>>().into_iter().rev().collect()
            } else {
                // Safe cast: we've checked maxsplit >= 0
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                let mut parts: Vec<_> = s.rsplitn(max.saturating_add(1), sep.as_str()).collect();
                parts.reverse();
                parts
            }
        }
        None => {
            // Split on whitespace from right
            if maxsplit < 0 {
                s.split_whitespace().collect()
            } else {
                // Safe cast: we've checked maxsplit >= 0
                let max = usize::try_from(maxsplit).unwrap_or(usize::MAX);
                rsplit_whitespace_n(s, max)
            }
        }
    };

    // Convert to list of strings (using interned empty string when applicable)
    let mut list_items = Vec::with_capacity(parts.len());
    for part in parts {
        list_items.push(allocate_string(part.to_owned(), heap)?);
    }

    let list = crate::types::List::new(list_items);
    let heap_id = heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Parses arguments for split methods.
///
/// Supports both positional and keyword arguments for sep and maxsplit.
fn parse_split_args(
    method: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(Option<String>, i64)> {
    let (pos, kwargs) = args.into_parts();

    let mut pos_iter = pos;
    let sep_value = pos_iter.next();
    let maxsplit_value = pos_iter.next();

    // Check no extra positional arguments
    if pos_iter.next().is_some() {
        for v in pos_iter {
            v.drop_with_heap(heap);
        }
        if let Some(v) = sep_value {
            v.drop_with_heap(heap);
        }
        if let Some(v) = maxsplit_value {
            v.drop_with_heap(heap);
        }
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_at_most(method, 2, 3));
    }

    // Extract positional sep (default None)
    let mut has_pos_sep = sep_value.is_some();
    let mut sep = if let Some(v) = sep_value {
        if matches!(v, Value::None) {
            v.drop_with_heap(heap);
            None
        } else {
            let result = extract_string_arg(&v, heap, interns)?;
            v.drop_with_heap(heap);
            Some(result)
        }
    } else {
        None
    };

    // Extract positional maxsplit (default -1)
    let mut has_pos_maxsplit = maxsplit_value.is_some();
    let mut maxsplit = if let Some(v) = maxsplit_value {
        let result = extract_int_arg(&v, heap)?;
        v.drop_with_heap(heap);
        result
    } else {
        -1
    };

    // Process kwargs
    for (key, value) in kwargs {
        let Some(keyword_name) = key.as_either_str(heap) else {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(ExcType::type_error("keywords must be strings"));
        };

        let key_str = keyword_name.as_str(interns);
        match key_str {
            "sep" => {
                if has_pos_sep {
                    key.drop_with_heap(heap);
                    value.drop_with_heap(heap);
                    return Err(ExcType::type_error(format!(
                        "{method}() got multiple values for argument 'sep'"
                    )));
                }
                if matches!(value, Value::None) {
                    sep = None;
                } else {
                    sep = Some(extract_string_arg(&value, heap, interns)?);
                }
                has_pos_sep = true;
            }
            "maxsplit" => {
                if has_pos_maxsplit {
                    key.drop_with_heap(heap);
                    value.drop_with_heap(heap);
                    return Err(ExcType::type_error(format!(
                        "{method}() got multiple values for argument 'maxsplit'"
                    )));
                }
                maxsplit = extract_int_arg(&value, heap)?;
                has_pos_maxsplit = true;
            }
            _ => {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                return Err(ExcType::type_error(format!(
                    "'{key_str}' is an invalid keyword argument for {method}()"
                )));
            }
        }
        key.drop_with_heap(heap);
        value.drop_with_heap(heap);
    }

    Ok((sep, maxsplit))
}

/// Split string on whitespace, returning at most `maxsplit + 1` parts.
fn split_whitespace_n(s: &str, maxsplit: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut remaining = s.trim_start();
    let mut count = 0;

    while !remaining.is_empty() && count < maxsplit {
        if let Some(end) = remaining.find(|c: char| c.is_whitespace()) {
            parts.push(&remaining[..end]);
            remaining = remaining[end..].trim_start();
            count += 1;
        } else {
            break;
        }
    }

    if !remaining.is_empty() {
        parts.push(remaining);
    }

    parts
}

/// Split string on whitespace from the right, returning at most `maxsplit + 1` parts.
fn rsplit_whitespace_n(s: &str, maxsplit: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut remaining = s.trim_end();
    let mut count = 0;

    while !remaining.is_empty() && count < maxsplit {
        if let Some(start) = remaining.rfind(|c: char| c.is_whitespace()) {
            parts.push(&remaining[start + 1..]);
            remaining = remaining[..start].trim_end();
            count += 1;
        } else {
            break;
        }
    }

    if !remaining.is_empty() {
        parts.push(remaining);
    }

    parts.reverse();
    parts
}

/// Implements Python's `str.splitlines(keepends?)` method.
///
/// Returns a list of the lines in the string, breaking at line boundaries.
/// Accepts keepends as either positional or keyword argument.
fn str_splitlines(
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let keepends = parse_splitlines_args(args, heap, interns)?;

    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let len = bytes.len();

    while start < len {
        // Find the next line ending
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
                    // Check for \r\n
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

        let line = if keepends { &s[start..end] } else { &s[start..line_end] };

        lines.push(allocate_string(line.to_owned(), heap)?);

        start = end;
    }

    let list = crate::types::List::new(lines);
    let heap_id = heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Parses arguments for splitlines method.
///
/// Supports both positional and keyword arguments for keepends.
fn parse_splitlines_args(args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<bool> {
    let (pos, kwargs) = args.into_parts();

    let mut pos_iter = pos;
    let keepends_value = pos_iter.next();

    // Check no extra positional arguments
    if pos_iter.next().is_some() {
        for v in pos_iter {
            v.drop_with_heap(heap);
        }
        if let Some(v) = keepends_value {
            v.drop_with_heap(heap);
        }
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_at_most("str.splitlines", 1, 2));
    }

    // Extract positional keepends (default false)
    let mut has_pos_keepends = keepends_value.is_some();
    let mut keepends = if let Some(v) = keepends_value {
        let result = value_is_truthy(&v);
        v.drop_with_heap(heap);
        result
    } else {
        false
    };

    // Process kwargs
    for (key, value) in kwargs {
        let Some(keyword_name) = key.as_either_str(heap) else {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(ExcType::type_error("keywords must be strings"));
        };

        let key_str = keyword_name.as_str(interns);
        if key_str == "keepends" {
            if has_pos_keepends {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                return Err(ExcType::type_error(
                    "str.splitlines() got multiple values for argument 'keepends'",
                ));
            }
            keepends = value_is_truthy(&value);
            has_pos_keepends = true;
        } else {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(ExcType::type_error(format!(
                "'{key_str}' is an invalid keyword argument for str.splitlines()"
            )));
        }
        key.drop_with_heap(heap);
        value.drop_with_heap(heap);
    }

    Ok(keepends)
}

/// Checks if a value is truthy for bool conversion.
fn value_is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::None => false,
        _ => true, // Most other values are truthy
    }
}

/// Implements Python's `str.partition(sep)` method.
///
/// Splits the string at the first occurrence of sep, and returns a 3-tuple
/// containing the part before the separator, the separator itself, and the part after.
fn str_partition(
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let sep_value = args.get_one_arg("str.partition", heap)?;
    let sep = extract_string_arg(&sep_value, heap, interns)?;
    sep_value.drop_with_heap(heap);

    if sep.is_empty() {
        return Err(ExcType::value_error_empty_separator());
    }

    let (before, sep_found, after) = match s.find(&sep) {
        Some(pos) => (&s[..pos], &sep[..], &s[pos + sep.len()..]),
        None => (s, "", ""),
    };

    let before_val = allocate_string(before.to_owned(), heap)?;
    let sep_val = allocate_string(sep_found.to_owned(), heap)?;
    let after_val = allocate_string(after.to_owned(), heap)?;

    Ok(crate::types::allocate_tuple(
        smallvec![before_val, sep_val, after_val],
        heap,
    )?)
}

/// Implements Python's `str.rpartition(sep)` method.
///
/// Splits the string at the last occurrence of sep, and returns a 3-tuple
/// containing the part before the separator, the separator itself, and the part after.
fn str_rpartition(
    s: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let sep_value = args.get_one_arg("str.rpartition", heap)?;
    let sep = extract_string_arg(&sep_value, heap, interns)?;
    sep_value.drop_with_heap(heap);

    if sep.is_empty() {
        return Err(ExcType::value_error_empty_separator());
    }

    let (before, sep_found, after) = match s.rfind(&sep) {
        Some(pos) => (&s[..pos], &sep[..], &s[pos + sep.len()..]),
        None => ("", "", s),
    };

    let before_val = allocate_string(before.to_owned(), heap)?;
    let sep_val = allocate_string(sep_found.to_owned(), heap)?;
    let after_val = allocate_string(after.to_owned(), heap)?;

    Ok(crate::types::allocate_tuple(
        smallvec![before_val, sep_val, after_val],
        heap,
    )?)
}

// =============================================================================
// Replace/modify methods
// =============================================================================

/// Implements Python's `str.replace(old, new, count?)` method.
///
/// Returns a copy with all occurrences of substring old replaced by new.
/// If count is given, only the first count occurrences are replaced.
fn str_replace(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (old, new, count) = parse_replace_args("str.replace", args, heap, interns)?;

    let result = if count < 0 {
        s.replace(&old, &new)
    } else {
        // Safe cast: we've checked count >= 0
        let n = usize::try_from(count).unwrap_or(usize::MAX);
        s.replacen(&old, &new, n)
    };

    allocate_string(result, heap)
}

/// Parses arguments for the replace method.
///
/// Supports both positional and keyword arguments for count (Python 3.13+).
fn parse_replace_args(
    method: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(String, String, i64)> {
    let (pos, kwargs) = args.into_parts();

    let mut pos_iter = pos;
    let Some(old_value) = pos_iter.next() else {
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_at_least(method, 2, 0));
    };
    let Some(new_value) = pos_iter.next() else {
        old_value.drop_with_heap(heap);
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_at_least(method, 2, 1));
    };
    let count_value = pos_iter.next();

    // Check no extra positional arguments
    if pos_iter.next().is_some() {
        for v in pos_iter {
            v.drop_with_heap(heap);
        }
        old_value.drop_with_heap(heap);
        new_value.drop_with_heap(heap);
        if let Some(v) = count_value {
            v.drop_with_heap(heap);
        }
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_at_most(method, 3, 4));
    }

    let old = extract_string_arg(&old_value, heap, interns)?;
    old_value.drop_with_heap(heap);

    let new = extract_string_arg(&new_value, heap, interns)?;
    new_value.drop_with_heap(heap);

    let mut has_pos_count = count_value.is_some();
    let mut count = if let Some(v) = count_value {
        let result = extract_int_arg(&v, heap)?;
        v.drop_with_heap(heap);
        result
    } else {
        -1
    };

    // Process kwargs (Python 3.13+ allows count as keyword)
    for (key, value) in kwargs {
        let Some(keyword_name) = key.as_either_str(heap) else {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(ExcType::type_error("keywords must be strings"));
        };

        let key_str = keyword_name.as_str(interns);
        if key_str == "count" {
            if has_pos_count {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                return Err(ExcType::type_error(format!(
                    "{method}() got multiple values for argument 'count'"
                )));
            }
            count = extract_int_arg(&value, heap)?;
            has_pos_count = true;
        } else {
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(ExcType::type_error(format!(
                "'{key_str}' is an invalid keyword argument for {method}()"
            )));
        }
        key.drop_with_heap(heap);
        value.drop_with_heap(heap);
    }

    Ok((old, new, count))
}

/// Implements Python's `str.center(width, fillchar?)` method.
///
/// Returns centered in a string of length width. Padding is done using the
/// specified fill character (default is a space).
fn str_center(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (width, fillchar) = parse_justify_args("str.center", args, heap, interns)?;
    let len = s.chars().count();

    let result = if width <= len {
        s.to_owned()
    } else {
        let total_pad = width - len;
        let left_pad = total_pad / 2;
        let right_pad = total_pad - left_pad;
        let mut result = String::with_capacity(width);
        for _ in 0..left_pad {
            result.push(fillchar);
        }
        result.push_str(s);
        for _ in 0..right_pad {
            result.push(fillchar);
        }
        result
    };

    allocate_string(result, heap)
}

/// Implements Python's `str.ljust(width, fillchar?)` method.
///
/// Returns left-justified in a string of length width.
fn str_ljust(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (width, fillchar) = parse_justify_args("str.ljust", args, heap, interns)?;
    let len = s.chars().count();

    let result = if width <= len {
        s.to_owned()
    } else {
        let pad = width - len;
        let mut result = String::with_capacity(width);
        result.push_str(s);
        for _ in 0..pad {
            result.push(fillchar);
        }
        result
    };

    allocate_string(result, heap)
}

/// Implements Python's `str.rjust(width, fillchar?)` method.
///
/// Returns right-justified in a string of length width.
fn str_rjust(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (width, fillchar) = parse_justify_args("str.rjust", args, heap, interns)?;
    let len = s.chars().count();

    let result = if width <= len {
        s.to_owned()
    } else {
        let pad = width - len;
        let mut result = String::with_capacity(width);
        for _ in 0..pad {
            result.push(fillchar);
        }
        result.push_str(s);
        result
    };

    allocate_string(result, heap)
}

/// Parses arguments for justify methods (center, ljust, rjust).
fn parse_justify_args(
    method: &str,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(usize, char)> {
    let (pos, kwargs) = args.into_parts();
    if !kwargs.is_empty() {
        kwargs.drop_with_heap(heap);
        return Err(ExcType::type_error_no_kwargs(method));
    }

    let mut pos_iter = pos;
    let width_value = pos_iter
        .next()
        .ok_or_else(|| ExcType::type_error_at_least(method, 1, 0))?;
    let fillchar_value = pos_iter.next();

    // Check no extra arguments
    if pos_iter.next().is_some() {
        for v in pos_iter {
            v.drop_with_heap(heap);
        }
        width_value.drop_with_heap(heap);
        if let Some(v) = fillchar_value {
            v.drop_with_heap(heap);
        }
        return Err(ExcType::type_error_at_most(method, 2, 3));
    }

    let width_i64 = extract_int_arg(&width_value, heap)?;
    width_value.drop_with_heap(heap);

    // Safe cast: treat negative as 0, saturate large positive values
    let width = if width_i64 < 0 {
        0
    } else {
        usize::try_from(width_i64).unwrap_or(usize::MAX)
    };

    let fillchar = if let Some(v) = fillchar_value {
        let fill_str = extract_string_arg(&v, heap, interns)?;
        v.drop_with_heap(heap);
        if fill_str.chars().count() != 1 {
            return Err(ExcType::type_error_fillchar_must_be_single_char());
        }
        fill_str.chars().next().unwrap()
    } else {
        ' '
    };

    Ok((width, fillchar))
}

/// Implements Python's `str.zfill(width)` method.
///
/// Returns a copy of the string left filled with ASCII '0' digits to make a
/// string of length width. A sign prefix is handled correctly.
fn str_zfill(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
    let width_value = args.get_one_arg("str.zfill", heap)?;
    let width_i64 = extract_int_arg(&width_value, heap)?;
    width_value.drop_with_heap(heap);

    // Safe cast: treat negative as 0, saturate large positive values
    let width = if width_i64 < 0 {
        0
    } else {
        usize::try_from(width_i64).unwrap_or(usize::MAX)
    };
    let len = s.chars().count();

    let result = if width <= len {
        s.to_owned()
    } else {
        let pad = width - len;
        let mut chars = s.chars();
        let first = chars.next();

        let mut result = String::with_capacity(width);

        // Handle sign prefix
        if matches!(first, Some('+' | '-')) {
            result.push(first.unwrap());
            for _ in 0..pad {
                result.push('0');
            }
            result.extend(chars);
        } else {
            for _ in 0..pad {
                result.push('0');
            }
            result.push_str(s);
        }
        result
    };

    allocate_string(result, heap)
}

/// Implements Python's `str.encode(encoding='utf-8', errors='strict')` method.
///
/// Returns an encoded version of the string as a bytes object. Only supports
/// UTF-8 encoding (the native encoding for Rust strings).
fn str_encode(s: &str, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
    let (encoding, errors) = parse_encode_args(args, heap, interns)?;

    // Only UTF-8 is supported - Rust strings are always valid UTF-8
    let encoding_lower = encoding.to_ascii_lowercase();
    if encoding_lower != "utf-8" && encoding_lower != "utf8" {
        return Err(ExcType::lookup_error_unknown_encoding(&encoding));
    }

    // For UTF-8 encoding of a valid UTF-8 string, errors mode doesn't matter
    // since there's nothing to handle - the string is already valid UTF-8
    if errors != "strict" && errors != "ignore" && errors != "replace" && errors != "backslashreplace" {
        return Err(ExcType::lookup_error_unknown_error_handler(&errors));
    }

    let bytes = s.as_bytes().to_vec();
    let heap_id = heap.allocate(HeapData::Bytes(Bytes::new(bytes)))?;
    Ok(Value::Ref(heap_id))
}

/// Parses arguments for `str.encode()`.
///
/// Returns (encoding, errors) with defaults "utf-8" and "strict".
fn parse_encode_args(
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(String, String)> {
    let (first, second) = args.get_zero_one_two_args("str.encode", heap)?;

    let encoding = if let Some(v) = first {
        let s = extract_string_arg(&v, heap, interns)?;
        v.drop_with_heap(heap);
        s
    } else {
        "utf-8".to_owned()
    };

    let errors = if let Some(v) = second {
        let s = extract_string_arg(&v, heap, interns)?;
        v.drop_with_heap(heap);
        s
    } else {
        "strict".to_owned()
    };

    Ok((encoding, errors))
}

/// Implements Python's `str.isidentifier()` predicate.
///
/// Returns True if the string is a valid Python identifier according to
/// the language definition (starts with letter or underscore, followed by
/// letters, digits, or underscores). Empty strings return False.
fn str_isidentifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let mut chars = s.chars();

    // First character must be a letter (Unicode) or underscore
    let first = chars.next().unwrap();
    if !is_xid_start(first) && first != '_' {
        return false;
    }

    // Remaining characters must be letters, digits (Unicode), or underscores
    chars.all(is_xid_continue)
}

/// Checks if a character is valid at the start of an identifier (XID_Start).
///
/// This is a simplified implementation that covers ASCII and common Unicode letters.
/// Python uses the full Unicode XID_Start property.
fn is_xid_start(c: char) -> bool {
    c.is_alphabetic()
}

/// Checks if a character is valid in the continuation of an identifier (XID_Continue).
///
/// This is a simplified implementation that covers ASCII and common Unicode.
/// Python uses the full Unicode XID_Continue property.
fn is_xid_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Implements Python's `str.istitle()` predicate.
///
/// Returns True if the string is titlecased: uppercase characters follow
/// uncased characters and lowercase characters follow cased characters.
/// Empty strings return False.
fn str_istitle(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let mut prev_cased = false;
    let mut has_cased = false;

    for c in s.chars() {
        if c.is_uppercase() {
            // Uppercase must follow uncased
            if prev_cased {
                return false;
            }
            prev_cased = true;
            has_cased = true;
        } else if c.is_lowercase() {
            // Lowercase must follow cased
            if !prev_cased {
                return false;
            }
            prev_cased = true;
            has_cased = true;
        } else {
            // Uncased character
            prev_cased = false;
        }
    }

    has_cased
}
