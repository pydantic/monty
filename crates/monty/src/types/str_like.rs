//! Implementations of the methods `str` and `bytes` share, written once over [`StrLike`].
//!
//! Searching runs on the UTF-8 / raw bytes through `memchr`, polling the time limit as it goes, so
//! `str` and `bytes` get the same search cost and the same resource checks.

use std::ops::{Index, Range};

use memchr::memmem::{Finder, FinderRev};
use monty_types::ResourceError;
use smallvec::smallvec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{Heap, HeapData, HeapView},
    types::{List, allocate_tuple, slice::search_range},
    value::Value,
};

/// The text operations `str` and `bytes` methods are built from.
///
/// Implemented for `str` (whose units are `char`s) and `[u8]` (whose units are bytes).
/// Ranges given to `Index` are byte offsets; a UTF-8 needle found in UTF-8 text starts and ends
/// on char boundaries, so byte-level search results always slice a `str` safely.
/// Python-visible indices (`start`/`end` arguments, `find` results) count units, not bytes.
pub(crate) trait StrLike: Index<Range<usize>, Output = Self> {
    /// One element of the text: `char` for `str`, `u8` for `bytes`.
    type Unit: Copy;

    /// Whether search methods type-check `sub` before converting `start`/`end`.
    ///
    /// CPython's `str` methods do; its `bytes` methods convert the bounds first.
    const CHECKS_SUB_FIRST: bool;

    /// The underlying bytes, which every search runs over.
    fn as_bytes(&self) -> &[u8];

    /// Length in units, as Python's `len()` reports it.
    fn py_len(&self) -> usize;

    /// Converts a unit range, already clamped by [`search_range`], into byte offsets.
    fn byte_range(&self, start: usize, end: usize) -> Range<usize>;

    /// Converts a byte offset on a unit boundary into a unit index.
    fn unit_index(&self, byte_offset: usize) -> usize;

    /// Iterates `(byte offset, unit)` pairs.
    fn units(&self) -> impl DoubleEndedIterator<Item = (usize, Self::Unit)>;

    /// Byte length of one unit.
    fn unit_len(unit: Self::Unit) -> usize;

    /// Whether `unit` is whitespace for `split()`/`strip()` with no argument.
    fn is_space(unit: Self::Unit) -> bool;

    /// Whether `unit` occurs in this text, for `strip(chars)`.
    fn contains_unit(&self, unit: Self::Unit) -> bool;

    /// Borrows an argument of this type, or `None` for any other type.
    fn extract_arg<'a>(value: &Value, vm: &'a VM<'_>) -> Option<&'a Self>;

    /// The `TypeError` for a wrong-type argument; CPython words it per method family.
    ///
    /// `method` is the qualified name, e.g. `str.find`.
    fn arg_type_error(role: ArgRole, method: &str, value: &Value, vm: &VM<'_>) -> RunError;

    /// Converts a search method's integer `sub` to the one byte it names.
    ///
    /// `bytes` accepts an int there (`b'abc'.find(97) == 0`); `str` never does, returning `Ok(None)`.
    /// Also `Ok(None)` for a value of this type, which [`StrLike::extract_arg`] then borrows.
    fn byte_needle(value: &Value, method: &str, vm: &mut VM<'_>) -> RunResult<Option<u8>>;

    /// Allocates a copy of this text as a new Python value.
    fn allocate(&self, heap: &Heap) -> Value;

    /// The `ValueError` `index`/`rindex` raise when nothing matches.
    fn not_found_error() -> RunError;
}

/// Which argument a type check is for, choosing CPython's wording in [`StrLike::arg_type_error`].
#[derive(Clone, Copy)]
pub(crate) enum ArgRole {
    /// `sub` of `find`, `index`, `count` and their variants.
    Sub,
    /// `chars` of `strip` and its variants.
    Chars,
    /// The argument of `removeprefix` and `removesuffix`.
    Affix,
    /// `sep` of `partition` and `rpartition`.
    Separator,
    /// `sep` of `split` and `rsplit`.
    SplitSeparator,
}

/// Strips the type from a qualified method name (`str.find` → `find`), as some CPython errors name it.
pub(crate) fn bare_name(method: &str) -> &str {
    method.rsplit_once('.').map_or(method, |(_, name)| name)
}

/// Borrows an argument of type `T`, raising `T`'s `TypeError` for `role` otherwise.
fn extract<'a, T: StrLike + ?Sized>(value: &Value, role: ArgRole, method: &str, vm: &'a VM<'_>) -> RunResult<&'a T> {
    T::extract_arg(value, vm).ok_or_else(|| T::arg_type_error(role, method, value, vm))
}

/// Which of the `find` family [`search`] implements; they share parsing and searching.
#[derive(Clone, Copy)]
pub(crate) enum SearchOp {
    Find,
    Rfind,
    Index,
    Rindex,
    Count,
}

/// Which end(s) [`strip`] removes units from.
#[derive(Clone, Copy)]
pub(crate) enum StripSide {
    Both,
    Start,
    End,
}

/// Whether a split or partition matches from the left or from the right.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Forward,
    Reverse,
}

/// Implements `find`, `rfind`, `index`, `rindex` and `count`, all `(sub[, start[, end]])`.
pub(crate) fn search<'h, T: StrLike + ?Sized>(
    s: &impl HeapView<'h, T>,
    op: SearchOp,
    method: &str,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let len = s.get(vm.heap).py_len();
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);
    let (sub_value, bounds) = match pos.as_slice() {
        [sub_value, bounds @ ..] if bounds.len() <= 2 => (sub_value, bounds),
        [] => return Err(ExcType::type_error_at_least(bare_name(method), 1, 0)),
        _ => return Err(ExcType::type_error_at_most(bare_name(method), 3, pos.len())),
    };
    if T::CHECKS_SUB_FIRST {
        extract::<T>(sub_value, ArgRole::Sub, method, vm)?;
    }
    let range = search_range(bounds.first(), bounds.get(1), len, vm)?;
    let byte = T::byte_needle(sub_value, method, vm)?;
    let byte_needle;
    let needle = match byte {
        Some(byte) => {
            byte_needle = [byte];
            &byte_needle[..]
        }
        None => extract::<T>(sub_value, ArgRole::Sub, method, vm)?.as_bytes(),
    };

    // `start` past `end` finds nothing, even an empty `sub`: `'abc'.find('', 4) == -1`
    let Some((start, end)) = range else {
        return match op {
            SearchOp::Count => Ok(Value::Int(0)),
            SearchOp::Find | SearchOp::Rfind => Ok(Value::Int(-1)),
            SearchOp::Index | SearchOp::Rindex => Err(T::not_found_error()),
        };
    };
    let s = s.get(vm.heap);
    let slice = &s[s.byte_range(start, end)];
    let haystack = slice.as_bytes();

    let found = match op {
        SearchOp::Count => {
            // The empty string matches between every unit, plus at both ends.
            let count = if needle.is_empty() {
                slice.py_len() + 1
            } else {
                count_non_overlapping(haystack, needle, vm.heap)?
            };
            return Ok(Value::Int(to_i64(count)));
        }
        SearchOp::Find | SearchOp::Index if needle.is_empty() => Some(0),
        SearchOp::Find | SearchOp::Index => find_subsequence(haystack, needle, vm.heap)?,
        SearchOp::Rfind | SearchOp::Rindex if needle.is_empty() => Some(haystack.len()),
        SearchOp::Rfind | SearchOp::Rindex => rfind_subsequence(haystack, needle, vm.heap)?,
    };
    match (found, op) {
        (Some(pos), _) => Ok(Value::Int(to_i64(start + slice.unit_index(pos)))),
        (None, SearchOp::Index | SearchOp::Rindex) => Err(T::not_found_error()),
        (None, _) => Ok(Value::Int(-1)),
    }
}

/// Converts a length or index to a Python int; no real text reaches `i64::MAX`.
fn to_i64(n: usize) -> i64 {
    i64::try_from(n).expect("index exceeds i64::MAX")
}

/// Implements `strip`, `lstrip` and `rstrip`, all `([chars])`.
///
/// With no argument or `None`, strips whitespace as [`StrLike::is_space`] defines it.
pub(crate) fn strip<'h, T: StrLike + ?Sized>(
    s: &impl HeapView<'h, T>,
    side: StripSide,
    method: &str,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let pos = args.into_pos_only(method, vm.heap)?;
    defer_drop!(pos, vm);
    let chars = match pos.as_slice() {
        [] => None,
        [chars] => Some(chars),
        _ => return Err(ExcType::type_error_at_most(bare_name(method), 1, pos.len())),
    };
    let s = s.get(vm.heap);
    let stripped = match chars {
        None | Some(Value::None) => trim(s, side, T::is_space),
        Some(chars) => {
            let chars = extract::<T>(chars, ArgRole::Chars, method, vm)?;
            trim(s, side, |unit| chars.contains_unit(unit))
        }
    };
    Ok(stripped.allocate(vm.heap))
}

/// Removes leading and/or trailing units matching `strip_unit`.
fn trim<T: StrLike + ?Sized>(s: &T, side: StripSide, strip_unit: impl Fn(T::Unit) -> bool) -> &T {
    let len = s.as_bytes().len();
    let start = match side {
        StripSide::Both | StripSide::Start => s
            .units()
            .find(|&(_, unit)| !strip_unit(unit))
            .map_or(len, |(offset, _)| offset),
        StripSide::End => 0,
    };
    let end = match side {
        // Stripping everything from the start leaves `start == len`, so the fallback stays in range.
        StripSide::Both | StripSide::End => s
            .units()
            .rev()
            .find(|&(_, unit)| !strip_unit(unit))
            .map_or(start, |(offset, unit)| offset + T::unit_len(unit)),
        StripSide::Start => len,
    };
    &s[start..end]
}

/// Implements `removeprefix(prefix)`: `s[len(prefix):]` if `s` starts with `prefix`, else a copy.
pub(crate) fn remove_prefix<'h, T: StrLike + ?Sized>(
    s: &impl HeapView<'h, T>,
    method: &str,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let prefix = args.get_one_arg(method, vm.heap)?;
    defer_drop!(prefix, vm);
    let prefix = extract::<T>(prefix, ArgRole::Affix, method, vm)?.as_bytes();
    let s = s.get(vm.heap);
    let rest = if s.as_bytes().starts_with(prefix) {
        &s[prefix.len()..s.as_bytes().len()]
    } else {
        s
    };
    Ok(rest.allocate(vm.heap))
}

/// Implements `removesuffix(suffix)`: `s[:-len(suffix)]` if `s` ends with `suffix`, else a copy.
pub(crate) fn remove_suffix<'h, T: StrLike + ?Sized>(
    s: &impl HeapView<'h, T>,
    method: &str,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let suffix = args.get_one_arg(method, vm.heap)?;
    defer_drop!(suffix, vm);
    let suffix = extract::<T>(suffix, ArgRole::Affix, method, vm)?.as_bytes();
    let s = s.get(vm.heap);
    let len = s.as_bytes().len();
    let rest = if s.as_bytes().ends_with(suffix) {
        &s[0..len - suffix.len()]
    } else {
        s
    };
    Ok(rest.allocate(vm.heap))
}

/// Implements `partition(sep)` and `rpartition(sep)`, returning `(before, sep, after)`.
///
/// When `sep` is absent, `partition` returns `(s, '', '')` and `rpartition` `('', '', s)`.
pub(crate) fn partition<'h, T: StrLike + ?Sized>(
    s: &impl HeapView<'h, T>,
    direction: Direction,
    method: &str,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    let sep = args.get_one_arg(method, vm.heap)?;
    defer_drop!(sep, vm);
    let sep = extract::<T>(sep, ArgRole::Separator, method, vm)?;
    let needle = sep.as_bytes();
    if needle.is_empty() {
        return Err(ExcType::value_error_empty_separator());
    }

    let s = s.get(vm.heap);
    let haystack = s.as_bytes();
    let found = match direction {
        Direction::Forward => find_subsequence(haystack, needle, vm.heap)?,
        Direction::Reverse => rfind_subsequence(haystack, needle, vm.heap)?,
    };
    let empty = &s[0..0];
    let (before, sep, after) = match (found, direction) {
        (Some(pos), _) => (&s[0..pos], sep, &s[pos + needle.len()..haystack.len()]),
        (None, Direction::Forward) => (s, empty, empty),
        (None, Direction::Reverse) => (empty, empty, s),
    };
    let parts = smallvec![before.allocate(vm.heap), sep.allocate(vm.heap), after.allocate(vm.heap)];
    Ok(allocate_tuple(parts, vm.heap))
}

/// Implements `split(sep=None, maxsplit=-1)` and `rsplit(sep=None, maxsplit=-1)`.
///
/// `maxsplit` converts before `sep` is type-checked, as in CPython. A negative `maxsplit`
/// means no limit; `sep=None` splits on runs of whitespace and drops empty parts.
pub(crate) fn split<'h, T: StrLike + ?Sized>(
    s: &impl HeapView<'h, T>,
    direction: Direction,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Value> {
    // Separate shapes only so each error names its own method.
    let (sep, maxsplit) = match direction {
        Direction::Forward => {
            let SplitArgs { sep, maxsplit } = SplitArgs::from_args(args, vm)?;
            (sep, maxsplit)
        }
        Direction::Reverse => {
            let RsplitArgs { sep, maxsplit } = RsplitArgs::from_args(args, vm)?;
            (sep, maxsplit)
        }
    };
    defer_drop!(sep, vm);
    defer_drop!(maxsplit, vm);
    let maxsplit = usize::try_from(maxsplit.as_int(vm)?).ok();
    let sep = match sep {
        Value::None => None,
        sep => Some(extract::<T>(sep, ArgRole::SplitSeparator, "split", vm)?),
    };

    let s = s.get(vm.heap);
    let parts = match sep {
        Some(sep) if sep.as_bytes().is_empty() => return Err(ExcType::value_error_empty_separator()),
        Some(sep) => split_by(s, sep, maxsplit, direction, vm.heap)?,
        None => split_whitespace(s, maxsplit, direction),
    };

    let mut items = Vec::with_capacity(parts.len());
    for (i, part) in parts.into_iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(i)?;
        items.push(part.allocate(vm.heap));
    }
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))))
}

/// Argument shape for `split(sep=None, maxsplit=-1)`.
#[derive(FromArgs)]
#[from_args(name = "split", at_most_total)]
struct SplitArgs {
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::Int(-1))]
    maxsplit: Value,
}

/// Argument shape for `rsplit(sep=None, maxsplit=-1)`.
#[derive(FromArgs)]
#[from_args(name = "rsplit", at_most_total)]
struct RsplitArgs {
    #[from_args(default = Value::None)]
    sep: Value,
    #[from_args(default = Value::Int(-1))]
    maxsplit: Value,
}

/// Splits `s` on a non-empty `sep`, making at most `maxsplit` splits from the given end.
///
/// Matches are found from the splitting end, so overlapping separators split as CPython does:
/// `b'aaa'.rsplit(b'aa') == [b'a', b'']`.
fn split_by<'a, T: StrLike + ?Sized>(
    s: &'a T,
    sep: &T,
    maxsplit: Option<usize>,
    direction: Direction,
    heap: &Heap,
) -> Result<Vec<&'a T>, ResourceError> {
    let (haystack, needle) = (s.as_bytes(), sep.as_bytes());
    let mut parts = Vec::new();
    let can_split = |parts: &Vec<&T>| maxsplit.is_none_or(|max| parts.len() < max);
    // `maxsplit=0` permits no splits, so build no finder: its preprocessing runs ahead of any `check_time()`.
    if !can_split(&parts) {
        return Ok(vec![s]);
    }
    match direction {
        Direction::Forward => {
            let mut start = 0;
            if let Some(finder) = finder_for(needle, haystack) {
                while can_split(&parts)
                    && let Some(pos) = find_with(&finder, &haystack[start..], heap)?
                {
                    parts.push(&s[start..start + pos]);
                    start += pos + needle.len();
                }
            }
            parts.push(&s[start..haystack.len()]);
        }
        Direction::Reverse => {
            let mut end = haystack.len();
            if let Some(finder) = rfinder_for(needle, haystack) {
                while can_split(&parts)
                    && let Some(pos) = rfind_with(&finder, &haystack[..end], heap)?
                {
                    parts.push(&s[pos + needle.len()..end]);
                    end = pos;
                }
            }
            parts.push(&s[0..end]);
            parts.reverse();
        }
    }
    Ok(parts)
}

/// Splits `s` on runs of whitespace, making at most `maxsplit` splits from the given end.
///
/// Leading and trailing whitespace never produce empty parts, but once `maxsplit` is reached the
/// remainder is kept whole, including whitespace at the far end: `' a b '.split(None, 1) == ['a', 'b ']`.
fn split_whitespace<T: StrLike + ?Sized>(s: &T, maxsplit: Option<usize>, direction: Direction) -> Vec<&T> {
    let mut parts = Vec::new();
    let can_split = |parts: &Vec<&T>| maxsplit.is_none_or(|max| parts.len() < max);
    match direction {
        Direction::Forward => {
            let mut rest = trim(s, StripSide::Start, T::is_space);
            while !rest.as_bytes().is_empty()
                && can_split(&parts)
                && let Some((end, _)) = rest.units().find(|&(_, unit)| T::is_space(unit))
            {
                parts.push(&rest[0..end]);
                rest = trim(&rest[end..rest.as_bytes().len()], StripSide::Start, T::is_space);
            }
            if !rest.as_bytes().is_empty() {
                parts.push(rest);
            }
        }
        Direction::Reverse => {
            let mut rest = trim(s, StripSide::End, T::is_space);
            while !rest.as_bytes().is_empty()
                && can_split(&parts)
                && let Some((start, unit)) = rest.units().rev().find(|&(_, unit)| T::is_space(unit))
            {
                parts.push(&rest[start + T::unit_len(unit)..rest.as_bytes().len()]);
                rest = trim(&rest[0..start], StripSide::End, T::is_space);
            }
            if !rest.as_bytes().is_empty() {
                parts.push(rest);
            }
            parts.reverse();
        }
    }
    parts
}

// =============================================================================
// Time-polled byte search
// =============================================================================

/// Bytes scanned between `check_time()` polls.
const SCAN_CHUNK: usize = 64 * 1024;

/// Finds the first occurrence of `needle` in `haystack`.
///
/// Callers must handle the empty needle: it would spin [`count_non_overlapping`].
/// Repeated scans for the same needle should build one [`Finder`] via
/// [`finder_for`] and call [`find_with`] instead — see their docstrings.
pub(crate) fn find_subsequence(haystack: &[u8], needle: &[u8], heap: &Heap) -> Result<Option<usize>, ResourceError> {
    match finder_for(needle, haystack) {
        Some(finder) => find_with(&finder, haystack, heap),
        None => Ok(None),
    }
}

/// Builds a forward finder, or `None` when `needle` cannot fit in `haystack`.
///
/// [`Finder::new`] runs Two-Way preprocessing over the whole needle *before*
/// [`find_with`] reaches its first `check_time()`, so a host-supplied needle
/// longer than the haystack would burn unpolled time on a search that provably
/// cannot match. Every finder must be built through this or [`rfinder_for`].
pub(crate) fn finder_for<'n>(needle: &'n [u8], haystack: &[u8]) -> Option<Finder<'n>> {
    (needle.len() <= haystack.len()).then(|| Finder::new(needle))
}

/// Builds a reverse finder, or `None` when `needle` cannot fit in `haystack`.
///
/// The mirror of [`finder_for`], with the same preprocessing rationale.
fn rfinder_for<'n>(needle: &'n [u8], haystack: &[u8]) -> Option<FinderRev<'n>> {
    (needle.len() <= haystack.len()).then(|| FinderRev::new(needle))
}

/// Finds the first occurrence of `finder`'s needle in `haystack`.
///
/// Chunks overlap by `needle.len() - 1` so boundary-straddling matches are
/// found; the stride never drops below the needle length so that overlap cannot
/// dominate. Above that floor a chunk spans up to `2 * needle.len()`, so a long
/// needle widens the time-limit overshoot — unavoidable, since a window
/// shorter than the needle cannot hold a match. See
/// `limitations/resource_limits.md`.
///
/// Takes the [`Finder`] by reference because constructing one runs Two-Way
/// preprocessing over the needle; callers scanning in a loop (counting,
/// splitting, replacing) build it once rather than per match.
pub(crate) fn find_with(finder: &Finder<'_>, haystack: &[u8], heap: &Heap) -> Result<Option<usize>, ResourceError> {
    let needle_len = finder.needle().len();
    debug_assert!(needle_len > 0, "callers must handle the empty needle");
    let stride = SCAN_CHUNK.max(needle_len);
    let mut start = 0;
    while start < haystack.len() {
        heap.tracker.check_time()?;
        let end = start
            .saturating_add(stride + needle_len.saturating_sub(1))
            .min(haystack.len());
        if let Some(pos) = finder.find(&haystack[start..end]) {
            return Ok(Some(start + pos));
        }
        start += stride;
    }
    Ok(None)
}

/// Finds the last occurrence of `needle` in `haystack`.
///
/// The mirror of [`find_subsequence`]; [`rfind_with`] is the reusable form.
fn rfind_subsequence(haystack: &[u8], needle: &[u8], heap: &Heap) -> Result<Option<usize>, ResourceError> {
    match rfinder_for(needle, haystack) {
        Some(finder) => rfind_with(&finder, haystack, heap),
        None => Ok(None),
    }
}

/// Finds the last occurrence of `finder`'s needle in `haystack`.
///
/// The mirror of [`find_with`], walking chunks from the end; the same
/// preprocessing-reuse rationale applies.
fn rfind_with(finder: &FinderRev<'_>, haystack: &[u8], heap: &Heap) -> Result<Option<usize>, ResourceError> {
    let needle_len = finder.needle().len();
    debug_assert!(needle_len > 0, "callers must handle the empty needle");
    let stride = SCAN_CHUNK.max(needle_len);
    let mut end = haystack.len();
    while end > 0 {
        heap.tracker.check_time()?;
        let start = end.saturating_sub(stride + needle_len.saturating_sub(1));
        if let Some(pos) = finder.rfind(&haystack[start..end]) {
            return Ok(Some(start + pos));
        }
        end = end.saturating_sub(stride);
    }
    Ok(None)
}

/// Counts non-overlapping occurrences of `needle` in `haystack`.
fn count_non_overlapping(haystack: &[u8], needle: &[u8], heap: &Heap) -> Result<usize, ResourceError> {
    let Some(finder) = finder_for(needle, haystack) else {
        return Ok(0);
    };
    let mut count = 0;
    let mut pos = 0;
    while let Some(found) = find_with(&finder, &haystack[pos..], heap)? {
        count += 1;
        pos += found + needle.len();
    }
    Ok(count)
}
