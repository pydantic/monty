# Argument-extraction errors emitted by the `#[derive(FromArgs)]` macro.
#
# This file is the source of truth for every error path the macro can
# produce, exercising each across all three error styles (Python, C,
# NamedC) and the modifier flags (at_most_total, at_most_positional).
#
# Each section names the error path being tested. Where Monty's wording
# matches CPython byte-for-byte the assert is unconditional; where Monty
# qualifies method names that CPython leaves bare (e.g. `str.expandtabs()`
# vs `expandtabs()`).
import asyncio
import datetime
import re
import sys

is_monty = sys.platform == 'monty'

# =====================================================================
# === Python style (default — no `c_error` / `c_error_named`)        ===
# =====================================================================

# === Python: unknown kwarg ===
try:
    [1, 2].sort(bogus=1)
    assert False, 'list.sort with unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "sort() got an unexpected keyword argument 'bogus'", f'py-unknown-kw: {e}'

try:
    sorted([1], bogus=1)
    assert False, 'sorted with unknown kwarg should raise'
except TypeError as e:
    # CPython's sorted() delegates internally to list.sort, so the
    # kwarg-name error surfaces as `sort()`. Monty matches because
    # `builtin_sorted` parses kwargs via the same `ListSortArgs` /
    # `parse_and_sort` entry point as `list.sort`.
    assert str(e) == "sort() got an unexpected keyword argument 'bogus'", f'py-unknown-kw-sorted: {e}'

# Unknown kwarg still wins after valid kwargs are accepted.
try:
    sorted([1], key=None, bogus=1)
    assert False, 'sorted valid+unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "sort() got an unexpected keyword argument 'bogus'", f'py-mixed-kw-sorted: {e}'

# Passing `iterable=` as a kwarg routes through `ListSortArgs`, which
# doesn't have an `iterable` slot — so the unknown-kwarg error fires
# (via `sort()`) instead of the sorted() arity error.
try:
    sorted([1], iterable=[2])
    assert False, 'sorted with iterable= kwarg should raise'
except TypeError as e:
    assert str(e) == "sort() got an unexpected keyword argument 'iterable'", f'py-iterable-kw-routed: {e}'

# === Python: pos_or_keyword conflict ('multiple values for argument') ===
# `re.split(pattern, string, pattern=...)` is a pos_or_keyword conflict.
# Monty's wording matches CPython's def-style here.
try:
    re.split('a', 'banana', pattern='b')
    assert False, 're.split with pos+kw conflict should raise'
except TypeError as e:
    assert str(e) == "split() got multiple values for argument 'pattern'", f'py-pos-kw: {e}'

# === Python: missing required positional (PyArg_UnpackKeywords wording derived from `pos_only`) ===
try:
    'abc'.replace('a')
    assert False, 'str.replace() missing arg should raise'
except TypeError as e:
    assert str(e) == 'replace() takes at least 2 positional arguments (1 given)', f'py-missing-pos: {e}'

# === Python: too-many positional (per-arg fallback — no at_most_total) ===
try:
    {}.update({1: 2}, {3: 4})
    assert False, 'dict.update too many should raise'
except TypeError as e:
    assert str(e) == 'update expected at most 1 argument, got 2', f'py-toomany-pos: {e}'

# === Python: duplicate kw_only via ** unpacking ===
# When both kwarg sources name the same key, Python's call machinery
# emits the duplicate error before the function is invoked — this is
# the bytecode-VM's `MethodDictMerge` opcode in Monty's case, NOT
# FromArgs. The opcode peeks the receiver from the stack at a known
# depth and qualifies the bare method name with the receiver's Python
# type, matching CPython's `list.sort()` wording byte-for-byte.
try:
    [1, 2].sort(key=int, **{'key': str})
    assert False, 'duplicate kw via ** should raise'
except TypeError as e:
    assert str(e) == "list.sort() got multiple values for keyword argument 'key'", f'py-dup-kw-only: {e}'

# Same path on a different receiver type confirms the qualifier really
# comes from the receiver, not a hard-coded name.
try:
    {}.update(a=1, **{'a': 2})
    assert False, 'dict.update duplicate kw via ** should raise'
except TypeError as e:
    assert str(e) == "dict.update() got multiple values for keyword argument 'a'", f'py-dup-kw-dict: {e}'

# === Python: at_most_total (str.expandtabs / str.splitlines / re.Match.groupdict) ===
try:
    'hello'.expandtabs(4, tabsize=8)
    assert False, 'expandtabs pos+kw should raise via at_most_total pre-count'
except TypeError as e:
    assert str(e) == 'expandtabs() takes at most 1 argument (2 given)', f'py-atmost-total-1: {e}'

try:
    'hello'.splitlines(True, keepends=False)
    assert False, 'splitlines pos+kw should raise via at_most_total pre-count'
except TypeError as e:
    assert str(e) == 'splitlines() takes at most 1 argument (2 given)', f'py-atmost-total-2: {e}'

m = re.match(r'(?P<x>.)', 'a')
assert m is not None
try:
    m.groupdict('N/A', default='N/A')
    assert False, 'groupdict pos+kw should raise via at_most_total pre-count'
except TypeError as e:
    assert str(e) == 'groupdict() takes at most 1 argument (2 given)', f'py-atmost-total-3: {e}'

# === Python: sorted() positional-arity wording (PyArg_UnpackTuple style) ===
# `builtin_sorted` manually checks the positional iterator length and
# emits CPython's `"sorted expected N argument(s), got M"` wording before
# the kwargs are even examined. Verify each direction: no args, too few
# (with kwargs that don't count), too many.
try:
    sorted()
    assert False, 'sorted() should require iterable'
except TypeError as e:
    assert str(e) == 'sorted expected 1 argument, got 0', f'py-arity-zero: {e}'

try:
    sorted([1], [2])
    assert False, 'sorted with two positionals should raise'
except TypeError as e:
    assert str(e) == 'sorted expected 1 argument, got 2', f'py-arity-two: {e}'

# Same arity error when the second value was meant as a key function —
# `key` is kw_only, so it can't fill the slot positionally.
try:
    sorted([3, 1, 2], int)
    assert False, 'sorted with positional key should raise'
except TypeError as e:
    assert str(e) == 'sorted expected 1 argument, got 2', f'py-arity-key-pos: {e}'

# Passing only kwargs (even the `iterable=` field name) hits the
# zero-positional arity branch *before* any kwarg parsing — confirms
# the positional check runs first.
try:
    sorted(iterable=[1, 2, 3])
    assert False, 'sorted iterable= only should raise on arity'
except TypeError as e:
    assert str(e) == 'sorted expected 1 argument, got 0', f'py-arity-iterable-kw: {e}'

# === Python: missing required (multiple positionals) ===
# map() has a CPython-specific bespoke message; Monty matches it via a
# pre-check in builtin_map before delegating to FromArgs.
try:
    map()
    assert False, 'map() should require args'
except TypeError as e:
    assert str(e) == 'map() must have at least two arguments.', f'py-missing-2: {e}'

try:
    map(int)
    assert False, 'map(fn) should require ≥2 args'
except TypeError as e:
    assert str(e) == 'map() must have at least two arguments.', f'py-missing-1: {e}'

# =====================================================================
# === C style (`c_error` — anonymous "function" wording)             ===
# =====================================================================
#
# Used by `date()` (with `at_most_total`) and `datetime()` (with
# `at_most_positional`). Error wording uses CPython's
# PyArg_ParseTupleAndKeywords "function" literal.

# === C: unknown kwarg under at_most_total threshold (missing-required wins) ===
# 2 positional + 1 unknown kwarg = total 3, max 3 → at_most_total
# does not fire; the macro defers the unknown-kwarg error for C / NamedC
# styles so that the missing-required check (matching CPython's
# `PyArg_ParseTupleAndKeywords` order) fires first.
try:
    datetime.date(2024, 1, foo=1)
    assert False, 'date unknown kwarg under at_most_total should raise'
except TypeError as e:
    assert str(e) == "function missing required argument 'day' (pos 3)", f'c-unknown-kw: {e}'

# === C: unknown kwarg with all required filled (unknown wins after missing check passes) ===
try:
    datetime.date(2024, day=3, month=2, foo=1)
    assert False, 'date with extra unknown should raise'
except TypeError as e:
    # 1 pos + 3 kwargs = 4 total > 3 max → at_most_total fires first.
    assert str(e) == 'function takes at most 3 arguments (4 given)', f'c-atmost-total-precheck: {e}'

try:
    datetime.date(2024, day=3, month=2)
    assert datetime.date(2024, day=3, month=2).day == 3, 'date kwarg-filled required succeeds'
except TypeError as e:
    assert False, f'unexpected error: {e}'

# === C: pos/kw conflict ===
try:
    datetime.datetime(2024, 1, 1, year=2025)
    assert False, 'datetime year pos+kw should raise'
except TypeError as e:
    assert str(e) == "argument for function given by name ('year') and position (1)", f'c-pos-kw: {e}'

# === C: missing required positional ===
try:
    datetime.date(2024)
    assert False, 'date with 1 positional should raise missing'
except TypeError as e:
    assert str(e) == "function missing required argument 'month' (pos 2)", f'c-missing: {e}'

# === C: too-many total (at_most_total — date) ===
try:
    datetime.date(2024, 1, 1, 1)
    assert False, 'date 4 positional should raise'
except TypeError as e:
    assert str(e) == 'function takes at most 3 arguments (4 given)', f'c-atmost-total-pos: {e}'

try:
    datetime.date(2024, 1, 1, year=2025)
    assert False, 'date 3pos + dup-kwarg should pre-count to too-many'
except TypeError as e:
    assert str(e) == 'function takes at most 3 arguments (4 given)', f'c-atmost-total-kwconflict: {e}'

# === C: too-many positional (at_most_positional — datetime) ===
# CPython's `datetime` constructor has 8 positional-or-keyword slots plus
# the keyword-only `fold` field (max_total = 9). The error wording pivots
# on whether the supplied count *could* still have fit in the kw-only tail:
# - 9 args: extras could fit fold → "8 positional arguments (9 given)"
# - 10+   : no slot of any kind   → "9 arguments (N given)" (drops
#                                    "positional", uses max_total = 9)
# The pivot is implemented by `type_error_c_at_most_positional_or_total`.
try:
    datetime.datetime(1, 2, 3, 4, 5, 6, 7, 8, 9)
    assert False, 'datetime 9 positional should raise'
except TypeError as e:
    assert str(e) == 'function takes at most 8 positional arguments (9 given)', f'c-atmost-positional: {e}'

# Regression: the per-arg tail used to report `__pos_count + 1`, missing
# both the remaining unconsumed positionals *and* any supplied kwargs.
# With 11 args the count must be 11, and the wording must drop "positional"
# because 11 > max_total (= 9).
try:
    datetime.datetime(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11)
    assert False, 'datetime 11 positional should raise'
except TypeError as e:
    assert str(e) == 'function takes at most 9 arguments (11 given)', f'c-atmost-positional-many: {e}'

# Same pivot reached via a kwarg counting toward the total: 9 positionals
# + 1 kwarg = 10 args > max_total (= 9), so "9 arguments (10 given)".
try:
    datetime.datetime(1, 2, 3, 4, 5, 6, 7, 8, 9, fold=0)
    assert False, 'datetime 9 positional + fold kwarg should raise'
except TypeError as e:
    assert str(e) == 'function takes at most 9 arguments (10 given)', f'c-atmost-positional-kw: {e}'

# === Python: too-many positional with >1 extra (count must include remaining iter) ===
try:
    'a'.replace('b', 'c', 1, 2, 3)
    assert False, 'str.replace 5 positional should raise'
except TypeError as e:
    assert str(e) == 'replace() takes at most 3 arguments (5 given)', f'py-atmost-pos-many: {e}'

# =====================================================================
# === NamedC style (`c_error_named` — embeds the type's name)        ===
# =====================================================================
#
# Used by `str`, `bytes`, `timezone` (the latter with `at_most_total`).

# === NamedC: unknown kwarg ===
try:
    str(wrong=42)
    assert False, 'str unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "str() got an unexpected keyword argument 'wrong'", f'named-unknown-kw-str: {e}'

try:
    bytes(wrong=3)
    assert False, 'bytes unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "bytes() got an unexpected keyword argument 'wrong'", f'named-unknown-kw-bytes: {e}'

try:
    datetime.timezone(datetime.timedelta(0), bogus=1)
    assert False, 'timezone unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "timezone() got an unexpected keyword argument 'bogus'", f'named-unknown-kw-tz: {e}'

# === NamedC: pos/kw conflict ===
try:
    str(42, object=42)
    assert False, 'str pos+kw should raise'
except TypeError as e:
    assert str(e) == "argument for str() given by name ('object') and position (1)", f'named-pos-kw-str: {e}'

try:
    bytes(3, source=3)
    assert False, 'bytes pos+kw should raise'
except TypeError as e:
    assert str(e) == "argument for bytes() given by name ('source') and position (1)", f'named-pos-kw-bytes: {e}'

try:
    datetime.timezone(datetime.timedelta(0), offset=datetime.timedelta(0))
    assert False, 'timezone pos+kw should raise'
except TypeError as e:
    assert str(e) == "argument for timezone() given by name ('offset') and position (1)", f'named-pos-kw-tz: {e}'

# === NamedC: missing required positional ===
try:
    datetime.timezone()
    assert False, 'timezone() should raise missing offset'
except TypeError as e:
    assert str(e) == "timezone() missing required argument 'offset' (pos 1)", f'named-missing: {e}'

# === NamedC: at_most_total (timezone) ===
try:
    datetime.timezone(datetime.timedelta(0), 'A', name='B')
    assert False, 'timezone 3 args should raise via at_most_total'
except TypeError as e:
    assert str(e) == 'timezone() takes at most 2 arguments (3 given)', f'named-atmost-total: {e}'

# =====================================================================
# === Cross-cutting (independent of error_style)                     ===
# =====================================================================

# === Non-string kwarg key (any style — emitted by macro's key extraction) ===
# Python rejects non-string keys before the call reaches the function, so
# the macro's defensive check rarely fires from pure Python. Both engines
# raise the same wording.
try:
    'a'.replace(**{1: 'x'})
    assert False, 'non-string kwarg key should raise'
except TypeError as e:
    assert str(e) == 'keywords must be strings', f'nonstring-key: {e}'

# === Duplicate kw_only kwarg via ** unpacking ===
# Like `list.sort` above, but on a `print()` (Python style with
# varargs + kw_only). Python's call machinery intercepts this before
# the macro sees it.
try:
    print('a', sep=',', **{'sep': '.'})
    assert False, 'print duplicate sep should raise'
except TypeError as e:
    assert str(e) == "print() got multiple values for keyword argument 'sep'", f'cross-dup-kw-only: {e}'

if is_monty:

    async def foo():
        return 1

    try:
        asyncio.gather(foo(), foo(), xxx=True)
        assert False, 'gather with kwarg should raise'
    except NotImplementedError as e:
        assert str(e) == 'gather() does not yet support keyword arguments'
