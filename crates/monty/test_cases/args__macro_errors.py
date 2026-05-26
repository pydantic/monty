# Argument-extraction errors emitted by the `#[derive(FromArgs)]` macro.
#
# This file is the source of truth for every error path the macro can
# produce, exercising each across all three error styles (Python, C,
# NamedC) and the modifier flags (at_most_total, at_most_positional).
#
# Each section names the error path being tested. Where Monty's wording
# matches CPython byte-for-byte the assert is unconditional; where Monty
# qualifies method names that CPython leaves bare (e.g. `str.expandtabs()`
# vs `expandtabs()`) or otherwise diverges by an intentional design choice,
# the assert is gated on `_monty`.
import datetime
import re
import sys

_monty = 'Monty' in sys.version

# =====================================================================
# === Python style (default — no `c_error` / `c_error_named`)        ===
# =====================================================================

# === Python: unknown kwarg ===
try:
    [1, 2].sort(bogus=1)
    assert False, 'list.sort with unknown kwarg should raise'
except TypeError as e:
    if _monty:
        assert str(e) == "list.sort() got an unexpected keyword argument 'bogus'", f'py-unknown-kw: {e}'
    else:
        # CPython uses the bare method name for unbound-method error messages.
        assert str(e) == "sort() got an unexpected keyword argument 'bogus'", f'py-unknown-kw: {e}'

try:
    sorted([1], bogus=1)
    assert False, 'sorted with unknown kwarg should raise'
except TypeError as e:
    if _monty:
        assert str(e) == "sorted() got an unexpected keyword argument 'bogus'", f'py-unknown-kw-sorted: {e}'
    else:
        # CPython's sorted() delegates to list.sort and surfaces sort()'s error.
        assert str(e) == "sort() got an unexpected keyword argument 'bogus'", f'py-unknown-kw-sorted: {e}'

# === Python: pos_or_keyword conflict ('multiple values for argument') ===
# Monty's dict.update declares `source` as pos_or_keyword so the
# positional + same-named kwarg is a binding conflict. CPython's
# dict.update declares `other` as positional-only and absorbs `source`
# into **kwargs — no error there. So this assertion is monty-only.
if _monty:
    d = {}
    try:
        d.update({1: 2}, source={3: 4})
        assert False, 'dict.update with pos+kw conflict should raise'
    except TypeError as e:
        assert str(e) == "dict.update() got multiple values for argument 'source'", f'py-pos-kw: {e}'

# === Python: missing required positional ===
try:
    'abc'.replace('a')
    assert False, 'str.replace() missing arg should raise'
except TypeError as e:
    if _monty:
        assert str(e) == "str.replace() missing 1 required positional argument: 'new'", f'py-missing-pos: {e}'
    else:
        assert str(e) == 'replace() takes at least 2 positional arguments (1 given)', f'py-missing-pos: {e}'

# === Python: too-many positional (per-arg fallback — no at_most_total) ===
try:
    {}.update({1: 2}, {3: 4})
    assert False, 'dict.update too many should raise'
except TypeError as e:
    if _monty:
        assert str(e) == 'dict.update expected at most 1 arguments, got 2', f'py-toomany-pos: {e}'
    else:
        assert str(e) == 'update expected at most 1 argument, got 2', f'py-toomany-pos: {e}'

# === Python: duplicate kw_only via ** unpacking ===
# When both kwarg sources name the same key, Python's call machinery
# emits the duplicate error before the function is invoked. Monty
# surfaces the bare attribute name (`sort()`) while CPython qualifies
# it with the type (`list.sort()`).
try:
    [1, 2].sort(key=int, **{'key': str})
    assert False, 'duplicate kw via ** should raise'
except TypeError as e:
    if _monty:
        assert str(e) == "sort() got multiple values for keyword argument 'key'", f'py-dup-kw-only: {e}'
    else:
        assert str(e) == "list.sort() got multiple values for keyword argument 'key'", f'py-dup-kw-only: {e}'

# === Python: at_most_total (str.expandtabs / str.splitlines / re.Match.groupdict) ===
try:
    'hello'.expandtabs(4, tabsize=8)
    assert False, 'expandtabs pos+kw should raise via at_most_total pre-count'
except TypeError as e:
    if _monty:
        assert str(e) == 'str.expandtabs() takes at most 1 argument (2 given)', f'py-atmost-total-1: {e}'
    else:
        assert str(e) == 'expandtabs() takes at most 1 argument (2 given)', f'py-atmost-total-1: {e}'

try:
    'hello'.splitlines(True, keepends=False)
    assert False, 'splitlines pos+kw should raise via at_most_total pre-count'
except TypeError as e:
    if _monty:
        assert str(e) == 'str.splitlines() takes at most 1 argument (2 given)', f'py-atmost-total-2: {e}'
    else:
        assert str(e) == 'splitlines() takes at most 1 argument (2 given)', f'py-atmost-total-2: {e}'

m = re.match(r'(?P<x>.)', 'a')
assert m is not None
try:
    m.groupdict('N/A', default='N/A')
    assert False, 'groupdict pos+kw should raise via at_most_total pre-count'
except TypeError as e:
    if _monty:
        assert str(e) == 're.Match.groupdict() takes at most 1 argument (2 given)', f'py-atmost-total-3: {e}'
    else:
        assert str(e) == 'groupdict() takes at most 1 argument (2 given)', f'py-atmost-total-3: {e}'

# === Python: pos-only field passed as kwarg (no static_string override → falls through to unknown) ===
# `sorted`'s `iterable` field is `pos_only` without an explicit
# `static_string` override, so a kwarg of the same name is reported as
# an unknown keyword rather than CPython's "positional-only arguments
# passed as keyword arguments" wording.
try:
    sorted(iterable=[1, 2, 3])
    assert False, 'sorted iterable= kwarg should raise'
except TypeError as e:
    if _monty:
        assert str(e) == "sorted() got an unexpected keyword argument 'iterable'", f'py-posonly-as-kw: {e}'
    else:
        assert str(e) == 'sorted expected 1 argument, got 0', f'py-posonly-as-kw: {e}'

# === Python: missing required (multiple positionals) ===
try:
    map()
    assert False, 'map() should require args'
except TypeError as e:
    if _monty:
        # Monty's macro raises on the first missing required field, so
        # `map()` reports `function` (pos 1) only. CPython has a custom
        # message for map() specifically.
        assert str(e) == "map() missing 1 required positional argument: 'function'", f'py-missing-2: {e}'
    else:
        assert str(e) == 'map() must have at least two arguments.', f'py-missing-2: {e}'

try:
    map(int)
    assert False, 'map(fn) should require ≥2 args'
except TypeError as e:
    if _monty:
        assert str(e) == "map() missing 1 required positional argument: 'first_iterable'", f'py-missing-1: {e}'
    else:
        assert str(e) == 'map() must have at least two arguments.', f'py-missing-1: {e}'

# =====================================================================
# === C style (`c_error` — anonymous "function" wording)             ===
# =====================================================================
#
# Used by `date()` (with `at_most_total`) and `datetime()` (with
# `at_most_positional`). Error wording uses CPython's
# PyArg_ParseTupleAndKeywords "function" literal.

# === C: unknown kwarg (under at_most_total threshold) ===
# 2 positional + 1 unknown kwarg = total 3, max 3 → at_most_total
# does not fire; falls through to per-arg dispatch.
# CPython processes positional first and reports missing `day`; Monty
# processes positional then kwargs and reports the unknown kwarg first.
try:
    datetime.date(2024, 1, foo=1)
    assert False, 'date unknown kwarg under at_most_total should raise'
except TypeError as e:
    if _monty:
        assert str(e) == "this function got an unexpected keyword argument 'foo'", f'c-unknown-kw: {e}'
    else:
        assert str(e) == "function missing required argument 'day' (pos 3)", f'c-unknown-kw: {e}'

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
# datetime has 8 fields max; passing 9 positionals trips the
# "function takes at most 8 positional arguments (9 given)" wording
# specific to `at_most_positional`.
try:
    datetime.datetime(1, 2, 3, 4, 5, 6, 7, 8, 9)
    assert False, 'datetime 9 positional should raise'
except TypeError as e:
    assert str(e) == 'function takes at most 8 positional arguments (9 given)', f'c-atmost-positional: {e}'

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
