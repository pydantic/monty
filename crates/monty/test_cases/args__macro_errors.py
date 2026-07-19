# Argument-extraction errors emitted by the `#[derive(FromArgs)]` macro.
#
# This file is the source of truth for every error path the macro (and
# the runtime binder in `crates/monty/src/args/binder.rs`) can produce,
# exercising each across the style families (`def`, `clinic`, `c`,
# `c_named`, `unpack`) and the `at_most_total` modifier.
#
# Each section names the error path being tested. Where Monty's wording
# matches CPython byte-for-byte the assert is unconditional; where Monty
# qualifies method names that CPython leaves bare (e.g. `str.expandtabs()`
# vs `expandtabs()`).
import asyncio
import datetime
import json
import re
import sys
import unicodedata

is_monty = sys.platform == 'monty'

# =====================================================================
# === Clinic style (the default — plus `def` for pure-Python targets) ===
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

# === Python: def-style kwarg errors beat too-many-positional ===
# CPython's `def` binding processes keyword arguments before its
# too-many-positional check, so unexpected-kwarg and multiple-values
# errors win even when there are also excess positionals.
try:
    re.sub('a', 'b', 'c', 'd', 'e', 'f', bogus=1)
    assert False, 're.sub with excess positionals + unknown kwarg should raise unknown-kwarg'
except TypeError as e:
    assert str(e) == "sub() got an unexpected keyword argument 'bogus'", f'py-def-kw-beats-overflow: {e}'

try:
    re.sub('a', 'b', 'c', 'd', 'e', 'f', pattern='x')
    assert False, 're.sub with excess positionals + duplicate kwarg should raise multiple-values'
except TypeError as e:
    assert str(e) == "sub() got multiple values for argument 'pattern'", f'py-def-dup-beats-overflow: {e}'

# When every kwarg binds cleanly to a keyword-only param, the overflow
# fires and counts them in the `(and N keyword-only argument(s))` suffix.
try:
    json.dumps(1, 2, indent=0)
    assert False, 'json.dumps with excess positional + kw-only kwarg should raise overflow'
except TypeError as e:
    assert str(e) == (
        'dumps() takes 1 positional argument but 2 positional arguments (and 1 keyword-only argument) were given'
    ), f'py-def-overflow-kwonly: {e}'

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
# === C style (`style = c` — anonymous "function" wording)           ===
# =====================================================================
#
# Used by `date()` (with `at_most_total`) and `datetime()` (whose
# positional-pivot wording is derived from its kw_only fields). Error
# wording uses CPython's PyArg_ParseTupleAndKeywords "function" literal.

# === C: unknown kwarg under at_most_total threshold (missing-required wins) ===
# 2 positional + 1 unknown kwarg = total 3, max 3 → at_most_total
# does not fire; the binder treats unknown kwargs as *leftovers* for the
# C families, raised only after every missing/conversion error (matching
# CPython's `PyArg_ParseTupleAndKeywords` final sweep).
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
    assert datetime.date(2024, day=3, month=2).day == 3
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

# === C: too-many positional (kw_only-derived pivot — datetime) ===
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
# === NamedC style (`style = c_named` — embeds the type's name)      ===
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

# =====================================================================
# === Binding vs conversion ordering (per style family)              ===
# =====================================================================
# The binder separates *binding* (arity, kwarg dispatch, conflicts,
# unknown kwargs) from *conversion*. The clinic family binds fully before
# any converter runs; the C families interleave missing errors with
# conversion parameter-by-parameter and report leftover kwargs
# (conflicts before unknowns) last. Every message below is CPython-exact.

# === clinic: unknown kwarg beats a bad-type positional ===
try:
    'a'.encode(42, bogus=1)
    assert False, 'encode bad type + unknown kwarg should raise on the kwarg'
except TypeError as e:
    assert str(e) == "encode() got an unexpected keyword argument 'bogus'", f'clinic-unknown-vs-type: {e}'

# === clinic: pos/kw conflict beats a bad-type positional ===
try:
    'a'.encode(42, encoding='x')
    assert False, 'encode bad type + conflict should raise the conflict'
except TypeError as e:
    assert str(e) == "argument for encode() given by name ('encoding') and position (1)", f'clinic-conflict: {e}'

# === clinic: total pre-count beats a bad-type positional (encode/decode at_most_total) ===
try:
    'a'.encode(42, 'x', 'y')
    assert False, 'encode 3 positional should raise arity'
except TypeError as e:
    assert str(e) == 'encode() takes at most 2 arguments (3 given)', f'clinic-atmost-pos: {e}'

try:
    'a'.encode('utf-8', 'strict', bogus=1)
    assert False, 'encode 2 pos + kwarg should pre-count to too-many'
except TypeError as e:
    assert str(e) == 'encode() takes at most 2 arguments (3 given)', f'clinic-atmost-total: {e}'

try:
    b'a'.decode('utf-8', 'strict', bogus=1)
    assert False, 'decode 2 pos + kwarg should pre-count to too-many'
except TypeError as e:
    assert str(e) == 'decode() takes at most 2 arguments (3 given)', f'clinic-atmost-total-decode: {e}'

# === C: conversion error beats a same-param conflict ===
try:
    datetime.date('x', year=1)
    assert False, 'date bad type + conflict should raise the conversion error'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer", f'c-convert-vs-conflict: {e}'

# === C: conversion error at an earlier param beats a later-param conflict ===
try:
    datetime.datetime('x', 1, 1, 4, hour=5)
    assert False, 'datetime bad year + hour conflict should raise the conversion error'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer", f'c-convert-vs-later-conflict: {e}'

# === C: kwarg values convert at their parameter position ===
try:
    datetime.date(2024, month='x')
    assert False, 'date bad-type month kwarg should raise at param 2'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer", f'c-kwarg-convert: {e}'

# ... but a missing earlier param wins over a later kwarg conversion.
try:
    datetime.date(month='x')
    assert False, 'date missing year should beat month conversion'
except TypeError as e:
    assert str(e) == "function missing required argument 'year' (pos 1)", f'c-missing-vs-kwarg-convert: {e}'

# === C: leftover conflict beats leftover unknown kwarg (either order) ===
try:
    datetime.datetime(2024, 1, 1, bogus=6, year=5)
    assert False, 'datetime conflict + unknown should raise the conflict'
except TypeError as e:
    assert str(e) == "argument for function given by name ('year') and position (1)", f'c-conflict-vs-unknown: {e}'

# === C: among several conflicts the earliest parameter is reported ===
try:
    datetime.datetime(2024, 1, 1, day=1, month=2)
    assert False, 'datetime double conflict should report month'
except TypeError as e:
    assert str(e) == "argument for function given by name ('month') and position (2)", f'c-earliest-conflict: {e}'

# =====================================================================
# === Unpack style (`style = unpack` — positional-only, no keywords) ===
# =====================================================================

# === unpack: any keyword is rejected wholesale, before arity ===
try:
    next(iter([]), 1, 2, bogus=1)
    assert False, 'next with kwargs should raise no-keyword error'
except TypeError as e:
    assert str(e) == 'next() takes no keyword arguments'

try:
    reversed(sequence=[1])
    assert False, 'reversed with kwargs should raise no-keyword error'
except TypeError as e:
    assert str(e) == 'reversed() takes no keyword arguments'

try:
    unicodedata.name('a', bogus=1)
    assert False, 'unicodedata.name with kwargs should raise no-keyword error'
except TypeError as e:
    # `kwarg_error_name` qualifies the module function like CPython does.
    assert str(e) == 'unicodedata.name() takes no keyword arguments'

# === unpack: exact arity (min == max collapses the wording) ===
try:
    reversed([1], [2])
    assert False, 'reversed with 2 args should raise'
except TypeError as e:
    assert str(e) == 'reversed expected 1 argument, got 2'

# =====================================================================
# === Newly bound builtins: sum (clinic), round (c_named)            ===
# =====================================================================

# === sum: positional-only iterable cannot be passed by keyword ===
try:
    sum(iterable=[1])
    assert False, 'sum(iterable=...) should raise'
except TypeError as e:
    assert str(e) == 'sum() takes at least 1 positional argument (0 given)'

# === sum: at_most_total pre-count ===
try:
    sum([1], 1, 1)
    assert False, 'sum with 3 args should raise'
except TypeError as e:
    assert str(e) == 'sum() takes at most 2 arguments (3 given)'

# === sum: unknown kwarg ===
try:
    sum([1], bogus=1)
    assert False, 'sum with unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "sum() got an unexpected keyword argument 'bogus'"

# === round: c_named missing required argument ===
try:
    round()
    assert False, 'round() should raise'
except TypeError as e:
    assert str(e) == "round() missing required argument 'number' (pos 1)"

try:
    round(ndigits=1)
    assert False, 'round(ndigits=1) should raise'
except TypeError as e:
    assert str(e) == "round() missing required argument 'number' (pos 1)"

# === round: at_most_total pre-count ===
try:
    round(1, 2, 3)
    assert False, 'round with 3 args should raise'
except TypeError as e:
    assert str(e) == 'round() takes at most 2 arguments (3 given)'

# === round: unknown kwarg ===
try:
    round(1, bogus=2)
    assert False, 'round with unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "round() got an unexpected keyword argument 'bogus'"

# =====================================================================
# === enumerate — CPython's hand-written vectorcall parser           ===
# =====================================================================
# CPython special-cases enumerate's argument parsing (enumobject.c
# `enumerate_vectorcall`), so its errors match no parser family; Monty
# mirrors that parser exactly, quirks included.

try:
    enumerate()
    assert False, 'enumerate() should raise'
except TypeError as e:
    assert str(e) == "enumerate() missing required argument 'iterable'"

# Quirk: any unrecognised zero-positional keyword shape reports the missing
# iterable — even when `iterable=` was actually passed.
try:
    enumerate(start=1, bogus=1, iterable=[1])
    assert False, 'enumerate 3-kwarg form should raise'
except TypeError as e:
    assert str(e) == "enumerate() missing required argument 'iterable'"

# Quirk: keyword names are validated positionally against the accepted
# shapes, so a lone `start=` reports 'start' as invalid.
try:
    enumerate(start=1)
    assert False, 'enumerate(start=1) should raise'
except TypeError as e:
    assert str(e) == "'start' is an invalid keyword argument for enumerate()"

try:
    enumerate([1], iterable=[2])
    assert False, 'enumerate positional + iterable= should raise'
except TypeError as e:
    assert str(e) == "'iterable' is an invalid keyword argument for enumerate()"

try:
    enumerate([1], bogus=1)
    assert False, 'enumerate with unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "'bogus' is an invalid keyword argument for enumerate()"

# Total pre-count fires whenever at least one positional is present.
try:
    enumerate([1], 0, 0)
    assert False, 'enumerate with 3 positionals should raise'
except TypeError as e:
    assert str(e) == 'enumerate() takes at most 2 arguments (3 given)'

try:
    enumerate([1], bogus=1, worse=2)
    assert False, 'enumerate 1-pos 2-kwarg should raise'
except TypeError as e:
    assert str(e) == 'enumerate() takes at most 2 arguments (3 given)'

# === unpack: arity wording (`{name} expected …`, no parentheses) ===
try:
    next()
    assert False, 'next() should raise'
except TypeError as e:
    assert str(e) == 'next expected at least 1 argument, got 0'

try:
    next(iter([]), 1, 2)
    assert False, 'next with 3 args should raise'
except TypeError as e:
    assert str(e) == 'next expected at most 2 arguments, got 3'

try:
    reversed()
    assert False, 'reversed() should raise'
except TypeError as e:
    assert str(e) == 'reversed expected 1 argument, got 0'

# === sum: body type-check reachable through the keyword path ===
try:
    sum([1], start='x')
    assert False, 'sum string start= should raise'
except TypeError as e:
    assert str(e) == "sum() can't sum strings [use ''.join(seq) instead]"

# === enumerate: lone unknown keyword ===
try:
    enumerate(bogus=1)
    assert False, 'enumerate(bogus=1) should raise'
except TypeError as e:
    assert str(e) == "'bogus' is an invalid keyword argument for enumerate()"

# === enumerate: two-kwarg form, each rejection branch ===
# swapped order (start first) with a bad second name
try:
    enumerate(start=1, bogus=2)
    assert False, 'enumerate(start=, bogus=) should raise'
except TypeError as e:
    assert str(e) == "'bogus' is an invalid keyword argument for enumerate()"

# unswapped order with a bad first name
try:
    enumerate(bogus=2, start=1)
    assert False, 'enumerate(bogus=, start=) should raise'
except TypeError as e:
    assert str(e) == "'bogus' is an invalid keyword argument for enumerate()"

# unswapped order with a good first and bad second name
try:
    enumerate(iterable=[1], bogus=1)
    assert False, 'enumerate(iterable=, bogus=) should raise'
except TypeError as e:
    assert str(e) == "'bogus' is an invalid keyword argument for enumerate()"

# === enumerate: non-string keys reach its hand-written key check ===
try:
    enumerate([1], **{1: 2})
    assert False, 'enumerate non-string key should raise'
except TypeError as e:
    assert str(e) == 'keywords must be strings'

try:
    enumerate(**{1: 2})
    assert False, 'enumerate lone non-string key should raise'
except TypeError as e:
    assert str(e) == 'keywords must be strings'
