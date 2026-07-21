/// Python string type, wrapping a Rust `String`.
///
/// This type provides Python string semantics. Currently supports basic
/// operations like length and equality comparison.
use std::{cell::Cell, fmt, fmt::Write, mem, ops};

use smallvec::smallvec;
use unicode_general_category::{GeneralCategory, get_general_category};

use super::{Bytes, CmpOrder, PyTrait};
use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    codecs::Codec,
    defer_drop,
    exception_private::{ExcType, RunResult},
    hash::{HashValue, hash_python_str},
    heap::{DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapRead, heap_read_ref_as_field},
    intern::{StaticStrings, StringId},
    resource::{ResourceError, ResourceTracker, check_replace_size},
    string_builder::StringBuilder,
    types::{
        LazyHeapSet, Type,
        slice::{normalize_sequence_index, slice_collect_iterator},
    },
    value::{EitherStr, Value, eq_str},
};

/// Python string value stored on the heap.
///
/// Wraps a Rust `String` and provides Python-compatible operations.
/// `len()` returns the number of Unicode codepoints (characters), matching Python semantics.
///
/// Carries an inline `cached_hash` field so a `Str` only computes its Python
/// hash once.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct Str(Box<str>, #[serde(skip)] Cell<Option<HashValue>>);

impl PartialEq for Str {
    /// Compares only the string content — the `cached_hash` field is a pure
    /// optimisation and must not affect equality.
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Str {
    /// Creates a new Str from anything convertible into a `Box<str>`.
    ///
    /// Private — use [`allocate_string`] or [`allocate_string_no_interning`] instead.
    #[must_use]
    fn new(s: impl Into<Box<str>>) -> Self {
        Self(s.into(), Cell::new(None))
    }

    /// Returns a reference to the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a string from the `str()` constructor call.
    ///
    /// - `str()` with no args returns an empty string (even with `encoding=`)
    /// - `str(x)` converts x to its string representation using `py_str`
    /// - `str(b, encoding, errors)` decodes a bytes object via the codec registry
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
                None => Ok(Value::InternString(StaticStrings::EmptyString.into())),
                Some(v) => v.py_str(vm),
            };
        }
        // Decoding mode. Type-check `encoding` then `errors` first — CPython's
        // clinic converters run before the constructor body sees the object.
        str_ctor_arg_check(encoding.as_ref(), "encoding", vm)?;
        str_ctor_arg_check(errors.as_ref(), "errors", vm)?;
        let Some(object) = object else {
            // A missing object wins over the decoding args: `str(encoding='utf-8')` is ''.
            return Ok(Value::InternString(StaticStrings::EmptyString.into()));
        };
        let bytes: &[u8] = match object {
            Value::InternBytes(bytes_id) => vm.interns.get_bytes(*bytes_id),
            Value::InternString(_) => return Err(ExcType::type_error_decoding_str_not_supported()),
            Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
                HeapData::Bytes(b) => b.as_slice(),
                HeapData::Str(_) => return Err(ExcType::type_error_decoding_str_not_supported()),
                _ => return Err(ExcType::type_error_decoding_need_bytes(object.py_type_name(vm))),
            },
            _ => return Err(ExcType::type_error_decoding_need_bytes(object.py_type_name(vm))),
        };
        let encoding = ctor_str_arg(encoding.as_ref(), "utf-8", vm)?;
        let errors = ctor_str_arg(errors.as_ref(), "strict", vm)?;
        let codec = Codec::find(encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(encoding))?;
        let s = codec.decode(bytes, errors)?;
        Ok(allocate_string(s, vm.heap)?)
    }

    /// Handles slice-based indexing for strings.
    ///
    /// Returns a new string containing the selected characters (Unicode-aware).
    fn getitem_slice(&self, vm: &VM<'_, impl ResourceTracker>, slice: &super::Slice) -> RunResult<Value> {
        let result_str: Box<str> = slice_collect_iterator(vm, slice, self.0.chars(), |c| c)?;
        Ok(allocate_string(result_str, vm.heap)?)
    }
}

/// Argument shape for `str(object='', encoding='utf-8', errors='strict')`.
///
/// `encoding`/`errors` are raw `Value`s validated in the body: CPython's
/// `unicode_new` names a wrong type by its `tp_name` (`must be str, not
/// NoneType`), not `_PyArg_BadArgument`'s `None` special case, so the macro's
/// `bad_arg_named` wording can't be used. `vectorcall` + `at_most_total`
/// model `unicode_vectorcall`'s dual arity wording.
#[derive(FromArgs)]
#[from_args(name = "str", at_most_total, vectorcall)]
struct StrInitArgs {
    #[from_args(default)]
    object: Option<Value>,
    #[from_args(default)]
    encoding: Option<Value>,
    #[from_args(default)]
    errors: Option<Value>,
}

/// Rejects a non-str `encoding`/`errors` argument to `str()` with CPython's
/// `str() argument '{name}' must be str, not {tp_name}` wording.
fn str_ctor_arg_check(arg: Option<&Value>, name: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<()> {
    match arg {
        Some(v) if !v.is_str(vm.heap) => Err(ExcType::type_error_bad_arg_named(
            "str",
            name,
            "str",
            v.py_type_name(vm),
        )),
        _ => Ok(()),
    }
}

/// Borrows a validated (str-typed) optional constructor argument's text,
/// falling back to `default` when the argument was absent.
fn ctor_str_arg<'a>(
    arg: Option<&'a Value>,
    default: &'a str,
    vm: &'a VM<'_, impl ResourceTracker>,
) -> RunResult<&'a str> {
    match arg {
        None => Ok(default),
        Some(v) => v.to_str(vm),
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
/// `split()`, string iteration, etc. Prefer this over manual `Str` construction
/// so callsites consistently benefit from interning. When the caller can prove
/// the string is longer than one byte, [`allocate_string_no_interning`] avoids
/// the length branch.
///
/// The dual bound `AsRef<str> + Into<Box<str>>` lets the function peek the
/// length via the borrow before committing to a conversion. Callers with an
/// owned `String`/`Box<str>` move the value in (consumed only on the heap
/// path), and borrowed `&str` callers avoid an upfront `to_owned()` —
/// allocation happens only when the string actually needs heap storage.
///
/// Returns `Result<_, ResourceError>` so the function composes with
/// `RunResult` and other error types that implement `From<ResourceError>`.
pub fn allocate_string(
    s: impl AsRef<str> + Into<Box<str>>,
    heap: &Heap<impl ResourceTracker>,
) -> Result<Value, ResourceError> {
    let bytes = s.as_ref().as_bytes();
    match bytes.len() {
        0 => Ok(Value::InternString(StaticStrings::EmptyString.into())),
        1 => Ok(Value::InternString(StringId::from_ascii(bytes[0]))),
        _ => allocate_string_no_interning(s, heap),
    }
}

/// Allocates a string directly on the heap, skipping the intern check.
///
/// Use this only when the caller can guarantee the string is longer than one
/// byte (e.g. always contains a fixed prefix like `"0x"`, `"0o"`, or a
/// formatted date). For inputs of unknown length, use [`allocate_string`].
///
/// Accepts `impl Into<Box<str>>` for the same reasons as [`allocate_string`].
pub fn allocate_string_no_interning(
    s: impl Into<Box<str>>,
    heap: &Heap<impl ResourceTracker>,
) -> Result<Value, ResourceError> {
    let heap_id = heap.allocate(HeapData::Str(Str::new(s)))?;
    Ok(Value::Ref(heap_id))
}

/// Allocates a single character as a string value.
///
/// ASCII characters use pre-interned strings for efficiency.
/// Non-ASCII characters are allocated on the heap.
///
/// This is used by string iteration and `chr()` builtin.
pub fn allocate_char(c: char, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    if c.is_ascii() {
        Ok(Value::InternString(StringId::from_ascii(c as u8)))
    } else {
        let heap_id = heap.allocate(HeapData::Str(Str::new(c.to_string())))?;
        Ok(Value::Ref(heap_id))
    }
}

/// Concatenates two string slices and allocates the result as a Python string.
///
/// Backs the `str + str` arms of `py_add`: one exactly-sized allocation filled
/// via `push_str`, instead of `format!`'s runtime formatting machinery. The
/// result size is bounded by two already-tracked inputs, so a plain `String`
/// is fine per the `StringBuilder` rule.
pub(crate) fn concat_allocate_str(a: &str, b: &str, heap: &Heap<impl ResourceTracker>) -> Result<Value, ResourceError> {
    let mut concat = String::with_capacity(a.len() + b.len());
    concat.push_str(a);
    concat.push_str(b);
    allocate_string(concat, heap)
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

impl ops::Deref for Str {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Str> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Str
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        // Count Unicode characters, not bytes, to match Python semantics
        Some(self.get(vm.heap).0.chars().count())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        // Check for slice first (Value::Ref pointing to HeapData::Slice)
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            return self.get(vm.heap).getitem_slice(vm, slice);
        }

        // Extract integer index, accepting Int, Bool (True=1, False=0), and LongInt
        let index = key.as_index(vm, Type::Str)?;

        // Use single-pass indexing to avoid Vec<char> allocation
        let s = self.get(vm.heap);
        let c = get_char_at_index(&s.0, index).ok_or_else(ExcType::str_index_error)?;
        Ok(allocate_char(c, vm.heap)?)
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        // A heap string equals an interned or heap string with the same content.
        Ok(eq_str(self.get(vm.heap).as_str(), other, vm))
    }

    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        let s = self.get(vm.heap);
        if let Some(cached) = s.1.get() {
            return Ok(Some(cached));
        }
        // Delegates to the canonical helper used by both heap and intern paths;
        // an interned `"foo"` and a heap `"foo"` must hash identically for dict
        // lookup to work.
        let hash = hash_python_str(s.as_str());
        s.1.set(Some(hash));
        Ok(Some(hash))
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        !self.get(vm.heap).0.is_empty()
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<CmpOrder> {
        Ok(CmpOrder::Ordered(self.get(vm.heap).0.cmp(&other.get(vm.heap).0)))
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        Ok(string_repr_fmt(&self.get(vm.heap).0, f)?)
    }

    fn py_str(&self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        Ok(allocate_string(self.get(vm.heap).as_str(), vm.heap)?)
    }

    fn py_add(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        Ok(Some(concat_allocate_str(
            self.get(vm.heap).as_str(),
            other.get(vm.heap).as_str(),
            vm.heap,
        )?))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let Some(method) = attr.static_string() else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::Str, attr.as_str(vm.interns)));
        };

        let s = heap_read_ref_as_field!(self, Str, 0);
        let s = s.as_box_value(vm.heap);
        call_str_method_impl(&s, method, args, vm).map(CallResult::Value)
    }
}

impl HeapItem for Str {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.0.len()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // No-op: strings don't hold Value references
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
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Value> {
    let args_guard = DropGuard::new(args, vm.heap);
    let Some(method) = StaticStrings::from_string_id(method_id) else {
        return Err(ExcType::attribute_error(Type::Str, vm.interns.get_str(method_id)));
    };
    let args = args_guard.into_inner();
    call_str_method_impl(&vm.heap.protect(s), method, args, vm)
}

/// Dispatches a method call on a string value.
///
/// This is the unified implementation for string method calls, used by both:
/// - `HeapRead<Str>::py_call_attr()` for heap-allocated strings
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
fn call_str_method_impl<'h>(
    s: &HeapRead<'h, str>,
    method: StaticStrings,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    match method {
        // Simple transformations (no arguments)
        StaticStrings::Lower => {
            args.check_zero_args("str.lower", vm.heap)?;
            str_lower(s.get(vm.heap), vm)
        }
        StaticStrings::Upper => {
            args.check_zero_args("str.upper", vm.heap)?;
            str_upper(s.get(vm.heap), vm)
        }
        StaticStrings::Capitalize => {
            args.check_zero_args("str.capitalize", vm.heap)?;
            str_capitalize(s.get(vm.heap), vm)
        }
        StaticStrings::Title => {
            args.check_zero_args("str.title", vm.heap)?;
            str_title(s.get(vm.heap), vm)
        }
        StaticStrings::Swapcase => {
            args.check_zero_args("str.swapcase", vm.heap)?;
            str_swapcase(s.get(vm.heap), vm)
        }
        StaticStrings::Casefold => {
            args.check_zero_args("str.casefold", vm.heap)?;
            str_casefold(s.get(vm.heap), vm)
        }
        // Predicate methods (no arguments, return bool)
        StaticStrings::Isalpha => {
            args.check_zero_args("str.isalpha", vm.heap)?;
            Ok(Value::Bool(str_isalpha(s.get(vm.heap))))
        }
        StaticStrings::Isdigit => {
            args.check_zero_args("str.isdigit", vm.heap)?;
            Ok(Value::Bool(str_isdigit(s.get(vm.heap))))
        }
        StaticStrings::Isalnum => {
            args.check_zero_args("str.isalnum", vm.heap)?;
            Ok(Value::Bool(str_isalnum(s.get(vm.heap))))
        }
        StaticStrings::Isnumeric => {
            args.check_zero_args("str.isnumeric", vm.heap)?;
            Ok(Value::Bool(str_isnumeric(s.get(vm.heap))))
        }
        StaticStrings::Isspace => {
            args.check_zero_args("str.isspace", vm.heap)?;
            Ok(Value::Bool(str_isspace(s.get(vm.heap))))
        }
        StaticStrings::Islower => {
            args.check_zero_args("str.islower", vm.heap)?;
            Ok(Value::Bool(str_islower(s.get(vm.heap))))
        }
        StaticStrings::Isupper => {
            args.check_zero_args("str.isupper", vm.heap)?;
            Ok(Value::Bool(str_isupper(s.get(vm.heap))))
        }
        StaticStrings::Isascii => {
            args.check_zero_args("str.isascii", vm.heap)?;
            Ok(Value::Bool(s.get(vm.heap).is_ascii()))
        }
        StaticStrings::Isdecimal => {
            args.check_zero_args("str.isdecimal", vm.heap)?;
            Ok(Value::Bool(str_isdecimal(s.get(vm.heap))))
        }
        // Search methods
        StaticStrings::Find => str_find(s, args, vm),
        StaticStrings::Rfind => str_rfind(s, args, vm),
        StaticStrings::Index => str_index(s, args, vm),
        StaticStrings::Rindex => str_rindex(s, args, vm),
        StaticStrings::Count => str_count(s, args, vm),
        StaticStrings::Startswith => str_startswith(s, args, vm),
        StaticStrings::Endswith => str_endswith(s, args, vm),
        // Strip/trim methods
        StaticStrings::Strip => str_strip(s, args, vm),
        StaticStrings::Lstrip => str_lstrip(s, args, vm),
        StaticStrings::Rstrip => str_rstrip(s, args, vm),
        StaticStrings::Removeprefix => str_removeprefix(s, args, vm),
        StaticStrings::Removesuffix => str_removesuffix(s, args, vm),
        // Split methods
        StaticStrings::Split => str_split(s, args, vm),
        StaticStrings::Rsplit => str_rsplit(s, args, vm),
        StaticStrings::Splitlines => str_splitlines(s, args, vm),
        StaticStrings::Partition => str_partition(s, args, vm),
        StaticStrings::Rpartition => str_rpartition(s, args, vm),
        // Replace/modify methods
        StaticStrings::Replace => str_replace(s, args, vm),
        StaticStrings::Center => str_center(s, args, vm),
        StaticStrings::Ljust => str_ljust(s, args, vm),
        StaticStrings::Rjust => str_rjust(s, args, vm),
        StaticStrings::Zfill => str_zfill(s, args, vm),
        StaticStrings::Expandtabs => str_expandtabs(s, args, vm),
        // Additional methods
        StaticStrings::Encode => str_encode(s, args, vm),
        StaticStrings::Isidentifier => {
            args.check_zero_args("str.isidentifier", vm.heap)?;
            Ok(Value::Bool(str_isidentifier(s.get(vm.heap))))
        }
        StaticStrings::Istitle => {
            args.check_zero_args("str.istitle", vm.heap)?;
            Ok(Value::Bool(str_istitle(s.get(vm.heap))))
        }
        // Existing method
        StaticStrings::Join => {
            let iterable = args.get_one_arg("str.join", vm.heap)?;
            str_join(s, iterable, vm)
        }
        _ => {
            args.drop_with(vm);
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
fn str_join<'h>(
    separator: &HeapRead<'h, str>,
    iterable: Value,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let Ok(iter) = iterable.into_py_iter(vm) else {
        return Err(ExcType::type_error_join_not_iterable());
    };
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    // Build result string, tracking index for error messages
    let mut result = String::new();
    let mut index = 0usize;

    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        if index > 0 {
            result.push_str(separator.get(vm.heap));
        }

        // Check item is a string and extract its content
        match item {
            Value::InternString(id) => {
                result.push_str(vm.interns.get_str(*id));
            }
            Value::Ref(heap_id) => {
                if let HeapData::Str(s) = vm.heap.get(*heap_id) {
                    result.push_str(s.as_str());
                } else {
                    let t = item.py_type_name(vm);
                    return Err(ExcType::type_error_join_item(index, &t));
                }
            }
            _ => {
                let t = item.py_type_name(vm);
                return Err(ExcType::type_error_join_item(index, &t));
            }
        }
        index += 1;
    }

    // Allocate result (uses interned empty string if result is empty)
    Ok(allocate_string(result, vm.heap)?)
}

/// Writes a Python `repr()` string for a given string slice to a formatter.
///
/// Quote choice matches CPython: single quotes by default, switching to double
/// quotes only when the string contains a `'` but no `"` (so the quote needn't
/// be escaped). Backslash, the active quote, and `\n`/`\t`/`\r` use the short
/// escapes; any other **non-printable** character is escaped numerically
/// (`\xNN`/`\uNNNN`/`\UNNNNNNNN`), e.g. `repr('\x00') == "'\\x00'"` and
/// `repr('\xa0') == "'\\xa0'"`.
///
/// "Non-printable" matches CPython's `str.isprintable` (see
/// [`repr_needs_escape`]): Unicode categories `C*` and `Z*`, except the ASCII
/// space. Category data comes from `unicode-general-category`, whose Unicode
/// version may differ slightly from CPython's, affecting only recently
/// (re)assigned code points.
pub fn string_repr_fmt(s: &str, f: &mut impl Write) -> fmt::Result {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    f.write_char(quote)?;
    for c in s.chars() {
        match c {
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            '\r' => f.write_str("\\r")?,
            _ if c == quote => {
                f.write_char('\\')?;
                f.write_char(quote)?;
            }
            _ if repr_needs_escape(c) => write_char_escape(c, f)?,
            _ => f.write_char(c)?,
        }
    }
    f.write_char(quote)
}

/// Whether `c` is escaped numerically in a Python `repr` — i.e. it is not
/// "printable" in CPython's sense.
///
/// Non-printable = Unicode general categories `Other` (`Cc`, `Cf`, `Cs`, `Co`,
/// `Cn`) and `Separator` (`Zl`, `Zp`, `Zs`), with the sole exception of the
/// ASCII space `U+0020`. The `\t`/`\n`/`\r` short escapes are handled by the
/// caller before this is consulted.
fn repr_needs_escape(c: char) -> bool {
    c != ' '
        && matches!(
            get_general_category(c),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
                | GeneralCategory::SpaceSeparator
        )
}

/// Writes the numeric repr escape for a single character, matching CPython's
/// width selection: `\xNN` for code points `<= 0xFF`, `\uNNNN` for `<= 0xFFFF`,
/// otherwise `\UNNNNNNNN`.
fn write_char_escape(c: char, f: &mut impl Write) -> fmt::Result {
    let cp = c as u32;
    if cp <= 0xFF {
        write!(f, "\\x{cp:02x}")
    } else if cp <= 0xFFFF {
        write!(f, "\\u{cp:04x}")
    } else {
        write!(f, "\\U{cp:08x}")
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
fn str_lower(s: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    Ok(allocate_string(s.to_lowercase(), vm.heap)?)
}

/// Implements Python's `str.upper()` method.
fn str_upper(s: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    Ok(allocate_string(s.to_uppercase(), vm.heap)?)
}

/// Implements Python's `str.capitalize()` method.
///
/// Returns a copy of the string with its first character capitalized and the rest lowercased.
fn str_capitalize(s: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
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
    Ok(allocate_string(result, vm.heap)?)
}

/// Implements Python's `str.title()` method.
///
/// Returns a titlecased version of the string where words start with an uppercase
/// character and the remaining characters are lowercase.
fn str_title(s: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
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

    Ok(allocate_string(result, vm.heap)?)
}

/// Implements Python's `str.swapcase()` method.
///
/// Returns a copy of the string with uppercase characters converted to lowercase and vice versa.
fn str_swapcase(s: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
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

    Ok(allocate_string(result, vm.heap)?)
}

/// Implements Python's `str.casefold()` method.
///
/// Returns a casefolded copy of the string. Casefolding is similar to lowercasing
/// but more aggressive because it is intended for caseless string matching.
fn str_casefold(s: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    // Rust's to_lowercase() is equivalent to Unicode casefolding for most purposes
    Ok(allocate_string(s.to_lowercase(), vm.heap)?)
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
fn str_find<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let str_len = s.get(vm.heap).chars().count();
    let (sub, start, end) = parse_search_args("str.find", str_len, args, vm)?;
    let s = s.get(vm.heap);
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
fn str_rfind<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let str_len = s.get(vm.heap).chars().count();
    let (sub, start, end) = parse_search_args("str.rfind", str_len, args, vm)?;
    let s = s.get(vm.heap);
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
fn str_index<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let str_len = s.get(vm.heap).chars().count();
    let (sub, start, end) = parse_search_args("str.index", str_len, args, vm)?;
    let s = s.get(vm.heap);
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
fn str_rindex<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let str_len = s.get(vm.heap).chars().count();
    let (sub, start, end) = parse_search_args("str.rindex", str_len, args, vm)?;
    let s = s.get(vm.heap);
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
fn str_count<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let str_len = s.get(vm.heap).chars().count();
    let (sub, start, end) = parse_search_args("str.count", str_len, args, vm)?;
    let s = s.get(vm.heap);
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
fn str_startswith<'h>(
    s: &HeapRead<'h, str>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    str_starts_ends_with(s, "str.startswith", |hay, prefix| hay.starts_with(prefix), args, vm)
}

/// Implements Python's `str.endswith(suffix, start?, end?)` method.
///
/// Returns True if the string ends with the suffix, otherwise returns False.
/// The suffix argument can be a string or a tuple of strings.
fn str_endswith<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    str_starts_ends_with(s, "str.endswith", |hay, suffix| hay.ends_with(suffix), args, vm)
}

/// Shared implementation of `str.startswith`/`str.endswith`.
///
/// Argument handling mirrors CPython's order: arg count, then start/end index
/// conversion (bad indices raise before the affix is even looked at), then the
/// affix itself — where tuple elements are validated lazily and in order, so a
/// match short-circuits before later elements are type-checked. Affix strings
/// are borrowed straight from interns/heap, never copied.
fn str_starts_ends_with<'h>(
    s: &HeapRead<'h, str>,
    method: &'static str,
    matcher: impl Fn(&str, &str) -> bool,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let str_len = s.get(vm.heap).chars().count();
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);
    let [affix, rest @ ..] = pos.as_slice() else {
        return Err(ExcType::type_error_at_least(method, 1, 0));
    };
    if rest.len() > 2 {
        return Err(ExcType::type_error_at_most(method, 3, pos.len()));
    }
    let start = match rest.first() {
        Some(value) => optional_index(value, 0, str_len, vm)?,
        None => 0,
    };
    let end = match rest.get(1) {
        Some(value) => optional_index(value, str_len, str_len, vm)?,
        None => str_len,
    };
    let slice = slice_string(s.get(vm.heap), start, end);
    Ok(Value::Bool(affix_matches(affix, slice, method, matcher, vm)?))
}

/// Tests `matcher(slice, affix)` (`str::starts_with` or `str::ends_with`) against
/// the affix argument, validating it in the process — see [`str_starts_ends_with`].
///
/// Tuple elements are checked in CPython order: a matching element returns
/// `Ok(true)` before later elements are type-checked; the first non-str element
/// reached raises `tuple for {method} must only contain str, not {type}`.
fn affix_matches(
    affix: &Value,
    slice: &str,
    method: &'static str,
    matcher: impl Fn(&str, &str) -> bool,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<bool> {
    // CPython error messages use the bare method name, not "str.startswith".
    let short_method = method.strip_prefix("str.").unwrap_or(method);
    let check = |a: &str| matcher(slice, a);
    match affix {
        Value::InternString(id) => Ok(check(vm.interns.get_str(*id))),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(a) => Ok(check(a.as_str())),
            HeapData::Tuple(tuple) => {
                for item in tuple.as_slice() {
                    let matched = match item {
                        Value::InternString(id) => check(vm.interns.get_str(*id)),
                        Value::Ref(hid) if let HeapData::Str(a) = vm.heap.get(*hid) => check(a.as_str()),
                        _ => {
                            return Err(ExcType::type_error_affix_tuple_item(
                                short_method,
                                "str",
                                &item.py_type_name(vm),
                            ));
                        }
                    };
                    if matched {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Err(ExcType::type_error_affix_arg(
                short_method,
                "str",
                &affix.py_type_name(vm),
            )),
        },
        _ => Err(ExcType::type_error_affix_arg(
            short_method,
            "str",
            &affix.py_type_name(vm),
        )),
    }
}

/// Parses arguments for search methods (find, rfind, index, rindex, count, startswith, endswith).
///
/// Returns (substring, start, end) where start and end are character indices.
fn parse_search_args(
    method: &str,
    str_len: usize,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(String, usize, usize)> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);
    match pos.as_slice() {
        [sub_value] => {
            let sub = extract_string_arg(sub_value, vm)?;
            Ok((sub, 0, str_len))
        }
        [sub_value, start_value] => {
            let sub = extract_string_arg(sub_value, vm)?;
            let start = optional_index(start_value, 0, str_len, vm)?;
            Ok((sub, start, str_len))
        }
        [sub_value, start_value, end_value] => {
            let sub = extract_string_arg(sub_value, vm)?;
            let start = optional_index(start_value, 0, str_len, vm)?;
            let end = optional_index(end_value, str_len, str_len, vm)?;
            Ok((sub, start, end))
        }
        [] => Err(ExcType::type_error_at_least(method, 1, 0)),
        _ => Err(ExcType::type_error_at_most(method, 3, pos.len())),
    }
}

/// Extracts a string from a Value, returning an error if not a string.
fn extract_string_arg(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<String> {
    match value {
        Value::InternString(id) => Ok(vm.interns.get_str(*id).to_owned()),
        Value::Ref(heap_id) => {
            if let HeapData::Str(s) = vm.heap.get(*heap_id) {
                Ok(s.as_str().to_owned())
            } else {
                Err(ExcType::type_error("expected str"))
            }
        }
        _ => Err(ExcType::type_error("expected str")),
    }
}

/// Extracts an integer from a Value, returning an error if not an integer.
fn extract_int_arg(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<i64> {
    match value {
        Value::Int(i) => Ok(*i),
        Value::Ref(heap_id) => {
            if let HeapData::LongInt(li) = vm.heap.get(*heap_id) {
                // Try to convert to i64
                li.to_i64().ok_or_else(|| ExcType::type_error("integer too large"))
            } else {
                Err(ExcType::type_error("expected int"))
            }
        }
        _ => Err(ExcType::type_error("expected int")),
    }
}

/// Extracts an optional slice index from a `Value`, treating `None` as `default`.
///
/// Used by argument parsers where `None` means "use the default index" and
/// any other value is interpreted as an integer and normalized against `str_len`.
/// Accepts `bool` like CPython (an `int` subtype); non-integers raise CPython's
/// `slice indices must be integers or None ...` rather than the generic
/// `expected int` used for non-slice integer args.
fn optional_index(
    value: &Value,
    default: usize,
    str_len: usize,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<usize> {
    match value {
        Value::None => Ok(default),
        Value::Int(i) => Ok(normalize_sequence_index(*i, str_len)),
        Value::Bool(b) => Ok(normalize_sequence_index(i64::from(*b), str_len)),
        Value::Ref(heap_id) if let HeapData::LongInt(li) = vm.heap.get(*heap_id) => {
            let i = li.to_i64().ok_or_else(|| ExcType::type_error("integer too large"))?;
            Ok(normalize_sequence_index(i, str_len))
        }
        _ => Err(ExcType::type_error_slice_indices()),
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
fn str_strip<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let chars = parse_strip_arg("str.strip", args, vm)?;
    let s = s.get(vm.heap);
    let result = match &chars {
        Some(c) => s.trim_matches(|ch| c.contains(ch)).to_owned(),
        None => s.trim().to_owned(),
    };
    Ok(allocate_string(result, vm.heap)?)
}

/// Implements Python's `str.lstrip(chars?)` method.
///
/// Returns a copy of the string with leading characters removed.
fn str_lstrip<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let chars = parse_strip_arg("str.lstrip", args, vm)?;
    let s = s.get(vm.heap);
    let result = match &chars {
        Some(c) => s.trim_start_matches(|ch| c.contains(ch)).to_owned(),
        None => s.trim_start().to_owned(),
    };
    Ok(allocate_string(result, vm.heap)?)
}

/// Implements Python's `str.rstrip(chars?)` method.
///
/// Returns a copy of the string with trailing characters removed.
fn str_rstrip<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let chars = parse_strip_arg("str.rstrip", args, vm)?;
    let s = s.get(vm.heap);
    let result = match &chars {
        Some(c) => s.trim_end_matches(|ch| c.contains(ch)).to_owned(),
        None => s.trim_end().to_owned(),
    };
    Ok(allocate_string(result, vm.heap)?)
}

/// Parses the optional chars argument for strip methods.
///
/// Accepts None as a value meaning "use default whitespace stripping".
fn parse_strip_arg(method: &str, args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<String>> {
    let value = args.get_zero_one_arg(method, vm.heap)?;
    match value {
        None => Ok(None),
        Some(Value::None) => Ok(None), // Explicit None means default whitespace
        Some(v) => {
            defer_drop!(v, vm);
            let result = extract_string_arg(v, vm)?;
            Ok(Some(result))
        }
    }
}

/// Implements Python's `str.removeprefix(prefix)` method.
///
/// If the string starts with the prefix string, return string[len(prefix):].
/// Otherwise, return a copy of the original string.
fn str_removeprefix<'h>(
    s: &HeapRead<'h, str>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let prefix_value = args.get_one_arg("str.removeprefix", vm.heap)?;
    defer_drop!(prefix_value, vm);
    let prefix = extract_string_arg(prefix_value, vm)?;

    let s = s.get(vm.heap);
    let result = s.strip_prefix(&prefix).unwrap_or(s).to_owned();
    Ok(allocate_string(result, vm.heap)?)
}

/// Implements Python's `str.removesuffix(suffix)` method.
///
/// If the string ends with the suffix string, return string[:-len(suffix)].
/// Otherwise, return a copy of the original string.
fn str_removesuffix<'h>(
    s: &HeapRead<'h, str>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let suffix_value = args.get_one_arg("str.removesuffix", vm.heap)?;
    defer_drop!(suffix_value, vm);
    let suffix = extract_string_arg(suffix_value, vm)?;

    let s = s.get(vm.heap);
    let result = s.strip_suffix(&suffix).unwrap_or(s).to_owned();
    Ok(allocate_string(result, vm.heap)?)
}

// =============================================================================
// Split methods
// =============================================================================

/// Implements Python's `str.split(sep?, maxsplit?)` method.
///
/// Returns a list of the words in the string, using sep as the delimiter string.
fn str_split<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let SplitArgs { sep, maxsplit } = SplitArgs::from_args(args, vm)?;
    let (sep, maxsplit) = coerce_split_args(sep, maxsplit, vm)?;
    let s = s.get(vm.heap);

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
        vm.heap.check_time()?;
        list_items.push(allocate_string(part, vm.heap)?);
    }

    let list = super::List::new(list_items);
    let heap_id = vm.heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Implements Python's `str.rsplit(sep?, maxsplit?)` method.
///
/// Returns a list of the words in the string, using sep as the delimiter string,
/// splitting from the right.
fn str_rsplit<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let RsplitArgs { sep, maxsplit } = RsplitArgs::from_args(args, vm)?;
    let (sep, maxsplit) = coerce_split_args(sep, maxsplit, vm)?;
    let s = s.get(vm.heap);

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
        vm.heap.check_time()?;
        list_items.push(allocate_string(part, vm.heap)?);
    }

    let list = super::List::new(list_items);
    let heap_id = vm.heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Coerces extracted `sep` / `maxsplit` `Value`s into the runtime shape used
/// by the actual `split`/`rsplit` implementations.
///
/// `sep = None` is documented as "split on whitespace", so it is mapped to
/// `Option::None`; any other value is run through `extract_string_arg`.
/// `maxsplit` is always coerced to `i64` via `extract_int_arg`. Each argument
/// is dropped on every path so refcounts stay balanced.
fn coerce_split_args(
    sep: Value,
    maxsplit: Value,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(Option<String>, i64)> {
    defer_drop!(sep, vm);
    defer_drop!(maxsplit, vm);
    let sep = match sep {
        Value::None => None,
        _ => Some(extract_string_arg(sep, vm)?),
    };
    let maxsplit = extract_int_arg(maxsplit, vm)?;
    Ok((sep, maxsplit))
}

/// Argument shape for `str.split(sep=None, maxsplit=-1)`.
#[derive(FromArgs)]
#[from_args(name = "split")]
struct SplitArgs {
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::Int(-1))]
    maxsplit: Value,
}

/// Argument shape for `str.rsplit(sep=None, maxsplit=-1)`.
#[derive(FromArgs)]
#[from_args(name = "rsplit")]
struct RsplitArgs {
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::Int(-1))]
    maxsplit: Value,
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
            let ws_len = remaining[start..].chars().next().unwrap().len_utf8();
            parts.push(&remaining[start + ws_len..]);
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
fn str_splitlines<'h>(
    s: &HeapRead<'h, str>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let keepends = parse_splitlines_args(args, vm)?;
    let s = s.get(vm.heap);

    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let len = bytes.len();

    while start < len {
        vm.heap.check_time()?;

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

        lines.push(allocate_string(line, vm.heap)?);

        start = end;
    }

    let list = super::List::new(lines);
    let heap_id = vm.heap.allocate(HeapData::List(list))?;
    Ok(Value::Ref(heap_id))
}

/// Parses arguments for splitlines method.
///
/// Supports both positional and keyword arguments for keepends.
fn parse_splitlines_args(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<bool> {
    let SplitlinesArgs { keepends } = SplitlinesArgs::from_args(args, vm)?;
    let result = keepends.as_ref().is_some_and(value_is_truthy);
    keepends.drop_with(vm.heap);
    Ok(result)
}

/// Argument shape for `str.splitlines(keepends=False)`. CPython evaluates
/// `keepends` for truthiness rather than strict-typing, so the field stays as
/// a raw `Value` for `value_is_truthy` to inspect.
#[derive(FromArgs)]
#[from_args(name = "splitlines", at_most_total)]
struct SplitlinesArgs {
    #[from_args(default)]
    keepends: Option<Value>,
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
fn str_partition<'h>(
    s: &HeapRead<'h, str>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let sep_value = args.get_one_arg("str.partition", vm.heap)?;
    defer_drop!(sep_value, vm);
    let sep = extract_string_arg(sep_value, vm)?;

    if sep.is_empty() {
        return Err(ExcType::value_error_empty_separator());
    }

    let s = s.get(vm.heap);
    let (before, sep_found, after) = match s.find(&sep) {
        Some(pos) => (&s[..pos], &sep[..], &s[pos + sep.len()..]),
        None => (s, "", ""),
    };

    let before_val = allocate_string(before, vm.heap)?;
    let sep_val = allocate_string(sep_found, vm.heap)?;
    let after_val = allocate_string(after, vm.heap)?;

    Ok(super::allocate_tuple(
        smallvec![before_val, sep_val, after_val],
        vm.heap,
    )?)
}

/// Implements Python's `str.rpartition(sep)` method.
///
/// Splits the string at the last occurrence of sep, and returns a 3-tuple
/// containing the part before the separator, the separator itself, and the part after.
fn str_rpartition<'h>(
    s: &HeapRead<'h, str>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let sep_value = args.get_one_arg("str.rpartition", vm.heap)?;
    defer_drop!(sep_value, vm);
    let sep = extract_string_arg(sep_value, vm)?;

    if sep.is_empty() {
        return Err(ExcType::value_error_empty_separator());
    }

    let s = s.get(vm.heap);
    let (before, sep_found, after) = match s.rfind(&sep) {
        Some(pos) => (&s[..pos], &sep[..], &s[pos + sep.len()..]),
        None => ("", "", s),
    };

    let before_val = allocate_string(before, vm.heap)?;
    let sep_val = allocate_string(sep_found, vm.heap)?;
    let after_val = allocate_string(after, vm.heap)?;

    Ok(super::allocate_tuple(
        smallvec![before_val, sep_val, after_val],
        vm.heap,
    )?)
}

// =============================================================================
// Replace/modify methods
// =============================================================================

/// Implements Python's `str.replace(old, new, count?)` method.
///
/// Returns a copy with all occurrences of substring old replaced by new.
/// If count is given, only the first count occurrences are replaced.
fn str_replace<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let (old, new, count) = parse_replace_args("str.replace", args, vm)?;
    let s = s.get(vm.heap);

    check_replace_size(s.len(), old.len(), new.len(), count, vm.heap.tracker())?;

    let result = if count < 0 {
        s.replace(&old, &new)
    } else {
        // Safe cast: we've checked count >= 0
        let n = usize::try_from(count).unwrap_or(usize::MAX);
        s.replacen(&old, &new, n)
    };

    Ok(allocate_string(result, vm.heap)?)
}

/// Parses arguments for the replace method.
///
/// Supports both positional and keyword arguments for count (Python 3.13+).
fn parse_replace_args(
    _method: &str,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(String, String, i64)> {
    let ReplaceArgs { old, new, count } = ReplaceArgs::from_args(args, vm)?;
    defer_drop!(old, vm);
    defer_drop!(new, vm);
    defer_drop!(count, vm);

    let old_s = extract_string_arg(old, vm)?;
    let new_s = extract_string_arg(new, vm)?;
    let count_i = extract_int_arg(count, vm)?;
    Ok((old_s, new_s, count_i))
}

/// Argument shape for `str.replace(old, new, count=-1)`.
///
/// `old` and `new` are positional-only at the C level — passing them as
/// kwargs produces CPython's "takes at least 2 positional arguments"
/// error, which the macro derives from the required pos-only count.
/// Python 3.13 promoted `count` from positional-only to
/// positional-or-keyword, hence the un-annotated default.
#[derive(FromArgs)]
#[from_args(name = "replace")]
struct ReplaceArgs {
    #[from_args(pos_only)]
    old: Value,
    #[from_args(pos_only)]
    new: Value,
    #[from_args(default = Value::Int(-1))]
    count: Value,
}

/// Implements Python's `str.center(width, fillchar?)` method.
///
/// Returns centered in a string of length width. Padding is done using the
/// specified fill character (default is a space).
fn str_center<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let (width, fillchar) = parse_justify_args("str.center", args, vm)?;
    let s = s.get(vm.heap);
    let len = s.chars().count();

    if width <= len {
        Ok(allocate_string(s, vm.heap)?)
    } else {
        // Exact byte capacity: the original string (`s.len()` bytes, possibly
        // multibyte) plus `pad` fillchars of `fillchar.len_utf8()` bytes each.
        // `width * fillchar.len_utf8()` would mis-charge the `s`-slot bytes.
        let total_pad = width - len;
        let capacity = s.len().saturating_add(total_pad.saturating_mul(fillchar.len_utf8()));
        let mut builder = StringBuilder::with_capacity(capacity, vm.heap.tracker())?;
        let left_pad = total_pad / 2;
        let right_pad = total_pad - left_pad;
        for _ in 0..left_pad {
            builder.push(fillchar)?;
        }
        builder.push_str(s)?;
        for _ in 0..right_pad {
            builder.push(fillchar)?;
        }
        builder.finish(vm.heap)
    }
}

/// Implements Python's `str.ljust(width, fillchar?)` method.
///
/// Returns left-justified in a string of length width.
fn str_ljust<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let (width, fillchar) = parse_justify_args("str.ljust", args, vm)?;
    let s = s.get(vm.heap);
    let len = s.chars().count();

    if width <= len {
        Ok(allocate_string(s, vm.heap)?)
    } else {
        let pad = width - len;
        let capacity = s.len().saturating_add(pad.saturating_mul(fillchar.len_utf8()));
        let mut builder = StringBuilder::with_capacity(capacity, vm.heap.tracker())?;
        builder.push_str(s)?;
        for _ in 0..pad {
            builder.push(fillchar)?;
        }
        builder.finish(vm.heap)
    }
}

/// Implements Python's `str.rjust(width, fillchar?)` method.
///
/// Returns right-justified in a string of length width.
fn str_rjust<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let (width, fillchar) = parse_justify_args("str.rjust", args, vm)?;
    let s = s.get(vm.heap);
    let len = s.chars().count();

    if width <= len {
        Ok(allocate_string(s, vm.heap)?)
    } else {
        let pad = width - len;
        let capacity = s.len().saturating_add(pad.saturating_mul(fillchar.len_utf8()));
        let mut builder = StringBuilder::with_capacity(capacity, vm.heap.tracker())?;
        for _ in 0..pad {
            builder.push(fillchar)?;
        }
        builder.push_str(s)?;
        builder.finish(vm.heap)
    }
}

/// Parses arguments for justify methods (center, ljust, rjust).
fn parse_justify_args(
    method: &str,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(usize, char)> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);

    match pos.as_slice() {
        [width_value] => {
            let w = extract_int_arg(width_value, vm)?;
            let width = if w < 0 {
                0
            } else {
                usize::try_from(w).unwrap_or(usize::MAX)
            };
            Ok((width, ' '))
        }
        [width_value, fillchar_value] => {
            let w = extract_int_arg(width_value, vm)?;
            let width = if w < 0 {
                0
            } else {
                usize::try_from(w).unwrap_or(usize::MAX)
            };
            let fill_str = extract_string_arg(fillchar_value, vm)?;
            if fill_str.chars().count() != 1 {
                return Err(ExcType::type_error_fillchar_must_be_single_char());
            }
            Ok((width, fill_str.chars().next().unwrap()))
        }
        [] => Err(ExcType::type_error_at_least(method, 1, 0)),
        _ => Err(ExcType::type_error_at_most(method, 2, pos.len())),
    }
}

/// Implements Python's `str.zfill(width)` method.
///
/// Returns a copy of the string left filled with ASCII '0' digits to make a
/// string of length width. A sign prefix is handled correctly.
fn str_zfill<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let width_value = args.get_one_arg("str.zfill", vm.heap)?;
    defer_drop!(width_value, vm);
    let width_i64 = extract_int_arg(width_value, vm)?;

    // Safe cast: treat negative as 0, saturate large positive values
    let width = if width_i64 < 0 {
        0
    } else {
        usize::try_from(width_i64).unwrap_or(usize::MAX)
    };
    let s = s.get(vm.heap);
    let len = s.chars().count();

    if width <= len {
        Ok(allocate_string(s, vm.heap)?)
    } else {
        // Exact byte capacity: zfill pads with ASCII '0' (1 byte each), so the
        // result is `s.len() + pad` bytes — `s.len()` (possibly multibyte)
        // rather than `width` (character count).
        let pad = width - len;
        let capacity = s.len().saturating_add(pad);
        let mut builder = StringBuilder::with_capacity(capacity, vm.heap.tracker())?;
        let mut chars = s.chars();
        let first = chars.next();

        if matches!(first, Some('+' | '-')) {
            builder.push(first.unwrap())?;
            for _ in 0..pad {
                builder.push('0')?;
            }
            for c in chars {
                builder.push(c)?;
            }
        } else {
            for _ in 0..pad {
                builder.push('0')?;
            }
            builder.push_str(s)?;
        }
        builder.finish(vm.heap)
    }
}

/// Implements Python's `str.expandtabs(tabsize=8)` method.
///
/// Returns a copy of the string where all tab characters are replaced by one or
/// more spaces, depending on the current column and the given tab size.
fn str_expandtabs<'h>(
    s: &HeapRead<'h, str>,
    args: ArgValues,
    vm: &mut VM<'h, impl ResourceTracker>,
) -> RunResult<Value> {
    let ExpandtabsArgs { tabsize } = ExpandtabsArgs::from_args(args, vm)?;

    let tabsize = match tabsize {
        None => 8,
        Some(val) => {
            let result_int = extract_int_arg(&val, vm)?;
            val.drop_with(vm.heap);
            if result_int < 0 {
                0
            } else {
                usize::try_from(result_int).unwrap_or(usize::MAX)
            }
        }
    };

    let s = s.get(vm.heap);
    // `tabsize` is attacker-controlled (saturates to `usize::MAX`) and we don't
    // know the result size up front, so use the unbounded builder — its 2×
    // growth policy rejects the build at the first push that would exceed the
    // memory limit, capping wasted intermediate allocation to `O(limit)`.
    let mut builder = StringBuilder::new(vm.heap.tracker());
    let mut column = 0;

    for c in s.chars() {
        if c == '\t' {
            if tabsize > 0 {
                let spaces = tabsize - (column % tabsize);
                for _ in 0..spaces {
                    builder.push(' ')?;
                }
                column += spaces;
            }
        } else {
            builder.push(c)?;
            if c == '\n' || c == '\r' {
                column = 0;
            } else {
                column += 1;
            }
        }
    }

    builder.finish(vm.heap)
}

/// Argument shape for `str.expandtabs(tabsize=8)`. `tabsize` is `Option<Value>`
/// so callers can distinguish "absent" (default 8) from any explicit value
/// without forcing the macro into a type-checked default.
#[derive(FromArgs)]
#[from_args(name = "expandtabs", at_most_total)]
struct ExpandtabsArgs {
    #[from_args(default)]
    tabsize: Option<Value>,
}

/// Implements Python's `str.encode(encoding='utf-8', errors='strict')` method.
///
/// Returns an encoded version of the string as a bytes object. Encoding-name
/// resolution and the codec implementations (UTF-8, ASCII, UTF-16/32
/// families) live in [`crate::codecs`]. Like CPython, the error handler name
/// is only looked up (and validated) when an actual encoding error occurs —
/// `'hello'.encode('ascii', 'bogus')` succeeds because there's nothing for
/// the handler to do.
fn str_encode<'h>(s: &HeapRead<'h, str>, args: ArgValues, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
    let EncodeArgs { encoding, errors } = EncodeArgs::from_args(args, vm)?;
    defer_drop!(encoding, vm);
    defer_drop!(errors, vm);
    let encoding = encoding.as_ref().map_or("utf-8", |e| e.as_str(vm));
    let errors = errors.as_ref().map_or("strict", |e| e.as_str(vm));

    let codec = Codec::find(encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(encoding))?;
    let bytes = codec.encode(s.get(vm.heap), errors, vm.heap.tracker())?;
    let heap_id = vm.heap.allocate(HeapData::Bytes(Bytes::new(bytes)))?;
    Ok(Value::Ref(heap_id))
}

/// Argument shape for `str.encode(encoding='utf-8', errors='strict')`.
///
/// `bad_arg_named` opts in to CPython's `_PyArg_BadArgument` named wording
/// (`encode() argument 'encoding' must be str, not <type>`) so wrong-type
/// errors match the C implementation. Both fields default to `None` (absent)
/// and the implementation supplies `"utf-8"` / `"strict"` after extraction;
/// CPython rejects explicit `None` here with the bad-arg error, which falls
/// out naturally because `Option<StrArg>::from_value` delegates to
/// `StrArg::from_value` and rejects `Value::None`.
#[derive(FromArgs)]
#[from_args(name = "encode", at_most_total, bad_arg_named)]
struct EncodeArgs {
    #[from_args(default)]
    encoding: Option<StrArg>,
    #[from_args(default)]
    errors: Option<StrArg>,
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
