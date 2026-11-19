# Native argument binding, conversion errors and the positional adapters.
import asyncio
import base64
import binascii
import bisect
import copy
import datetime
import itertools
import json
import math
import re
import sys
import unicodedata

is_monty = sys.platform == 'monty'


def expected_error(*, monty, cpython):
    return monty if is_monty else cpython


# === Static keyword matching: ASCII and multi-character names ===
assert base64.b64encode(s=b'a') == b'YQ=='
assert base64.a85encode(b=b'') == b''
assert base64.b64decode(s='YQ==') == b'a'
assert base64.b64decode(**{'S'.lower(): 'YQ=='}) == b'a'
assert base64.b64decode(**{'s': '-w==', ''.join(['alt', 'chars']): b'-_'}) == b'\xfb'

for key in ['s', 'S'.lower()]:
    try:
        base64.b64decode('YQ==', **{key: 'Yg=='})
        assert False, 'expected duplicate keyword to fail'
    except TypeError as e:
        assert str(e) == "b64decode() got multiple values for argument 's'"

for key in ['Z', 'z'.upper(), ''.join(['un', 'known'])]:
    try:
        base64.b64decode('YQ==', **{key: 'Yg=='})
        assert False, 'expected unknown keyword to fail'
    except TypeError as e:
        assert str(e) == f"b64decode() got an unexpected keyword argument '{key}'"

# === Math aggregations: positional-only calls and keyword-only start ===
for function, args, kwargs, message in [
    (math.hypot, (), {'x': 1}, 'math.hypot() takes no keyword arguments'),
    (math.hypot, ('bad',), {'x': 1}, 'math.hypot() takes no keyword arguments'),
    (math.fsum, (), {}, 'math.fsum() takes exactly one argument (0 given)'),
    (math.fsum, ([], []), {}, 'math.fsum() takes exactly one argument (2 given)'),
    (math.fsum, (), {'seq': []}, 'math.fsum() takes no keyword arguments'),
    (
        math.dist,
        (),
        {},
        expected_error(
            monty="dist() missing 2 required positional arguments: 'p' and 'q'",
            cpython='dist expected 2 arguments, got 0',
        ),
    ),
    (
        math.dist,
        ([], [], []),
        {},
        expected_error(
            monty='dist() takes at most 2 arguments (3 given)',
            cpython='dist expected 2 arguments, got 3',
        ),
    ),
    (
        math.dist,
        (),
        {'p': [], 'q': []},
        expected_error(
            monty="dist() got some positional-only arguments passed as keyword arguments: 'p'",
            cpython='math.dist() takes no keyword arguments',
        ),
    ),
    (
        math.sumprod,
        ([],),
        {},
        expected_error(
            monty="sumprod() missing 1 required positional argument: 'q'",
            cpython='sumprod expected 2 arguments, got 1',
        ),
    ),
    (
        math.sumprod,
        ([], [], []),
        {},
        expected_error(
            monty='sumprod() takes at most 2 arguments (3 given)',
            cpython='sumprod expected 2 arguments, got 3',
        ),
    ),
    (
        math.sumprod,
        (),
        {'p': [], 'q': []},
        expected_error(
            monty="sumprod() got some positional-only arguments passed as keyword arguments: 'p'",
            cpython='math.sumprod() takes no keyword arguments',
        ),
    ),
    (
        math.fma,
        (),
        {},
        expected_error(
            monty="fma() missing 3 required positional arguments: 'x', 'y', and 'z'",
            cpython='fma expected 3 arguments, got 0',
        ),
    ),
    (
        math.fma,
        (1, 2, 3, 4),
        {},
        expected_error(
            monty='fma() takes at most 3 arguments (4 given)',
            cpython='fma expected 3 arguments, got 4',
        ),
    ),
    (
        math.fma,
        (),
        {'x': 1, 'y': 2, 'z': 3},
        expected_error(
            monty="fma() got some positional-only arguments passed as keyword arguments: 'x'",
            cpython='math.fma() takes no keyword arguments',
        ),
    ),
    (
        math.fma,
        ('bad', 2, 3),
        {'x': 1},
        expected_error(
            monty="fma() got some positional-only arguments passed as keyword arguments: 'x'",
            cpython='math.fma() takes no keyword arguments',
        ),
    ),
]:
    try:
        function(*args, **kwargs)
        assert False, 'invalid math arguments must fail'
    except TypeError as e:
        assert str(e) == message, (str(e), message)

# === General binder ===

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
    assert str(e) == expected_error(
        monty='dumps() takes at most 1 positional argument (2 given)',
        cpython='dumps() takes 1 positional argument but 2 positional arguments (and 1 keyword-only argument) were given',
    ), f'py-def-overflow-kwonly: {e}'

# `json.load` / `json.dump` bind like `loads` / `dumps` with the file object
# as an extra positional; unknown keywords name the CPython decoder / encoder
# constructor that would have rejected them.
try:
    json.load()
    assert False, 'json.load() should raise missing'
except TypeError as e:
    assert str(e) == "load() missing 1 required positional argument: 'fp'"

try:
    json.load(1, 2)
    assert False, 'json.load(1, 2) should raise overflow'
except TypeError as e:
    assert str(e) == expected_error(
        monty='load() takes at most 1 argument (2 given)',
        cpython='load() takes 1 positional argument but 2 were given',
    )

try:
    json.load(1, fp=2)
    assert False, 'json.load(1, fp=2) should raise multiple-values'
except TypeError as e:
    assert str(e) == "load() got multiple values for argument 'fp'"


# CPython reads `fp` before `loads` rejects the keyword, so the object needs a `read`.
class _JsonReader:
    def read(self):
        return '1'


try:
    json.load(_JsonReader(), bogus=2)
    assert False, 'json.load(bogus=) should raise'
except TypeError as e:
    assert str(e) == "JSONDecoder.__init__() got an unexpected keyword argument 'bogus'"

try:
    json.loads('1', bogus=2)
    assert False, 'json.loads(bogus=) should raise'
except TypeError as e:
    assert str(e) == "JSONDecoder.__init__() got an unexpected keyword argument 'bogus'"

try:
    json.dump()
    assert False, 'json.dump() should raise missing'
except TypeError as e:
    assert str(e) == "dump() missing 2 required positional arguments: 'obj' and 'fp'"

try:
    json.dump(1)
    assert False, 'json.dump(1) should raise missing'
except TypeError as e:
    assert str(e) == "dump() missing 1 required positional argument: 'fp'"

try:
    json.dump(1, 2, 3)
    assert False, 'json.dump(1, 2, 3) should raise overflow'
except TypeError as e:
    assert str(e) == expected_error(
        monty='dump() takes at most 2 positional arguments (3 given)',
        cpython='dump() takes 2 positional arguments but 3 were given',
    )

try:
    json.dump(1, 2, obj=3)
    assert False, 'json.dump(1, 2, obj=3) should raise multiple-values'
except TypeError as e:
    assert str(e) == "dump() got multiple values for argument 'obj'"

try:
    json.dump(1, 2, bogus=3)
    assert False, 'json.dump(bogus=) should raise'
except TypeError as e:
    assert str(e) == "JSONEncoder.__init__() got an unexpected keyword argument 'bogus'"

# === Missing required positional argument ===
try:
    'abc'.replace('a')
    assert False, 'str.replace() missing arg should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="replace() missing 1 required positional argument: 'new'",
        cpython='replace() takes at least 2 positional arguments (1 given)',
    )

# === Positional overflow ===
try:
    {}.update({1: 2}, {3: 4})
    assert False, 'dict.update too many should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='update() takes at most 2 positional arguments (3 given)',
        cpython='update expected at most 1 argument, got 2',
    )

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

# === sorted() positional arity ===
try:
    sorted()
    assert False, 'sorted() should require iterable'
except TypeError as e:
    assert str(e) == expected_error(
        monty="sorted() missing 1 required positional argument: 'iterable'",
        cpython='sorted expected 1 argument, got 0',
    ), f'py-arity-zero: {e}'

try:
    sorted([1], [2])
    assert False, 'sorted with two positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='sorted() takes at most 1 positional argument (2 given)',
        cpython='sorted expected 1 argument, got 2',
    ), f'py-arity-two: {e}'

# Same arity error when the second value was meant as a key function —
# `key` is kw_only, so it can't fill the slot positionally.
try:
    sorted([3, 1, 2], int)
    assert False, 'sorted with positional key should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='sorted() takes at most 1 positional argument (2 given)',
        cpython='sorted expected 1 argument, got 2',
    ), f'py-arity-key-pos: {e}'

# The positional-only iterable is still missing.
try:
    sorted(iterable=[1, 2, 3])
    assert False, 'sorted iterable= only should raise on arity'
except TypeError as e:
    assert str(e) == expected_error(
        monty="sorted() missing 1 required positional argument: 'iterable'",
        cpython='sorted expected 1 argument, got 0',
    ), f'py-arity-iterable-kw: {e}'

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

# === def: copy() / deepcopy() arity and kwargs ===
# Both are pure-Python `def`s in CPython, so binding names the missing
# parameter and counts the optional ones in the too-many wording.
try:
    copy.copy()
    assert False, 'copy() with no args should raise'
except TypeError as e:
    assert str(e) == "copy() missing 1 required positional argument: 'x'"

try:
    copy.deepcopy()
    assert False, 'deepcopy() with no args should raise'
except TypeError as e:
    assert str(e) == "deepcopy() missing 1 required positional argument: 'x'"

try:
    copy.copy([], [])
    assert False, 'copy() with 2 positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='copy() takes at most 1 argument (2 given)',
        cpython='copy() takes 1 positional argument but 2 were given',
    )

try:
    copy.deepcopy([], {}, None, 1)
    assert False, 'deepcopy() with 4 positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='deepcopy() takes at most 3 arguments (4 given)',
        cpython='deepcopy() takes from 1 to 3 positional arguments but 4 were given',
    )

try:
    copy.copy(bogus=1)
    assert False, 'copy() with an unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "copy() got an unexpected keyword argument 'bogus'"

try:
    copy.deepcopy([1], memo={}, bogus=2)
    assert False, 'deepcopy() with an unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "deepcopy() got an unexpected keyword argument 'bogus'"

# =====================================================================
# === Keyword unpacking ===
# =====================================================================

# === Non-string keyword key ===
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

# === Unexpected keywords and positional-only arguments ===
try:
    next(iter([]), 1, 2, bogus=1)
    assert False, 'next with kwargs should raise TypeError'
except TypeError as e:
    assert str(e) == expected_error(
        monty="next() got an unexpected keyword argument 'bogus'",
        cpython='next() takes no keyword arguments',
    )

try:
    reversed(sequence=[1])
    assert False, 'reversed with kwargs should raise TypeError'
except TypeError as e:
    assert str(e) == expected_error(
        monty="reversed() got some positional-only arguments passed as keyword arguments: 'sequence'",
        cpython='reversed() takes no keyword arguments',
    )

try:
    unicodedata.name('a', bogus=1)
    assert False, 'unicodedata.name with kwargs should raise TypeError'
except TypeError as e:
    # `kwarg_error_name` qualifies the module function like CPython does.
    assert str(e) == expected_error(
        monty="unicodedata.name() got an unexpected keyword argument 'bogus'",
        cpython='unicodedata.name() takes no keyword arguments',
    )

# === reversed: too many positional arguments ===
try:
    reversed([1], [2])
    assert False, 'reversed with 2 args should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='reversed() takes at most 1 argument (2 given)',
        cpython='reversed expected 1 argument, got 2',
    )

# =====================================================================
# === Built-in signatures ===
# =====================================================================

for call, monty_error, cpython_error in [
    (
        lambda: any(iterable=[]),
        "any() got some positional-only arguments passed as keyword arguments: 'iterable'",
        'any() takes no keyword arguments',
    ),
    (
        lambda: filter(function=None, iterable=[]),
        "filter() got some positional-only arguments passed as keyword arguments: 'function'",
        'filter() takes no keyword arguments',
    ),
    (
        lambda: b'abc'.strip(bytes=b'a'),
        "bytes.strip() got some positional-only arguments passed as keyword arguments: 'bytes'",
        'bytes.strip() takes no keyword arguments',
    ),
]:
    try:
        call()
    except TypeError as e:
        assert str(e) == (monty_error if is_monty else cpython_error)
    else:
        assert False, 'expected a positional-only argument error'


# === sum: positional-only iterable cannot be passed by keyword ===
try:
    sum(iterable=[1])
    assert False, 'sum(iterable=...) should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="sum() got some positional-only arguments passed as keyword arguments: 'iterable'",
        cpython='sum() takes at least 1 positional argument (0 given)',
    )

# === sum: positional overflow ===
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

# === round: missing required argument ===
try:
    round()
    assert False, 'round() should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="round() missing 1 required positional argument: 'number'",
        cpython="round() missing required argument 'number' (pos 1)",
    )

try:
    round(ndigits=1)
    assert False, 'round(ndigits=1) should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="round() missing 1 required positional argument: 'number'",
        cpython="round() missing required argument 'number' (pos 1)",
    )

# === round: positional overflow ===
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

# enumerate uses Monty's shared binder and CPython's custom constructor parser.
monty_missing = "enumerate() missing 1 required positional argument: 'iterable'"
cpython_missing = "enumerate() missing required argument 'iterable'"
monty_unknown = "enumerate() got an unexpected keyword argument 'bogus'"
cpython_invalid = "'bogus' is an invalid keyword argument for enumerate()"
for positional, keywords, monty_error, cpython_error in [
    ((), {}, monty_missing, cpython_missing),
    ((), {'start': 1}, monty_missing, "'start' is an invalid keyword argument for enumerate()"),
    (
        ([1],),
        {'iterable': [2]},
        "enumerate() got multiple values for argument 'iterable'",
        "'iterable' is an invalid keyword argument for enumerate()",
    ),
    (([1],), {'bogus': 1}, monty_unknown, cpython_invalid),
    ((), {'start': 1, 'bogus': 1, 'iterable': [1]}, monty_unknown, cpython_missing),
    ((), {'bogus': 1}, monty_unknown, cpython_invalid),
    ((), {'start': 1, 'bogus': 2}, monty_unknown, cpython_invalid),
    ((), {'bogus': 2, 'start': 1}, monty_unknown, cpython_invalid),
    ((), {'iterable': [1], 'bogus': 1}, monty_unknown, cpython_invalid),
]:
    try:
        enumerate(*positional, **keywords)
    except TypeError as e:
        assert str(e) == (monty_error if is_monty else cpython_error)
    else:
        assert False, (positional, keywords)

try:
    enumerate([1], 0, 0)
    assert False, 'enumerate with 3 positionals should raise'
except TypeError as e:
    assert str(e) == 'enumerate() takes at most 2 arguments (3 given)'

try:
    enumerate([1], bogus=1, worse=2)
    assert False, 'enumerate 1-pos 2-kwarg should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="enumerate() got an unexpected keyword argument 'bogus'",
        cpython='enumerate() takes at most 2 arguments (3 given)',
    )

for index in (1 << 100, -(1 << 100)):
    for call in (lambda i: [0].pop(i), lambda i: itertools.repeat('x', i)):
        try:
            call(index)
        except OverflowError as e:
            assert str(e) == 'Python int too large to convert to C ssize_t'
        else:
            assert False

# === next and reversed: positional arity ===
try:
    next()
    assert False, 'next() should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="next() missing 1 required positional argument: 'iterator'",
        cpython='next expected at least 1 argument, got 0',
    )

try:
    next(iter([]), 1, 2)
    assert False, 'next with 3 args should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='next() takes at most 2 arguments (3 given)',
        cpython='next expected at most 2 arguments, got 3',
    )

try:
    reversed()
    assert False, 'reversed() should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="reversed() missing 1 required positional argument: 'sequence'",
        cpython='reversed expected 1 argument, got 0',
    )

# === sum: body type-check reachable through the keyword path ===
try:
    sum([1], start='x')
    assert False, 'sum string start= should raise'
except TypeError as e:
    assert str(e) == "sum() can't sum strings [use ''.join(seq) instead]"

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

# =====================================================================
# === def style: the base64 module (pure-Python defs in CPython) ===
# =====================================================================

try:
    base64.b64encode()
    assert False, 'b64encode() with no args should raise'
except TypeError as e:
    assert str(e) == "b64encode() missing 1 required positional argument: 's'"

try:
    base64.b64encode(b'a', b'-_', 1)
    assert False, 'b64encode() with 3 positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='b64encode() takes at most 2 arguments (3 given)',
        cpython='b64encode() takes from 1 to 2 positional arguments but 3 were given',
    )

try:
    base64.b64encode(b'a', bogus=1)
    assert False, 'b64encode() with unknown kwarg should raise'
except TypeError as e:
    assert str(e) == "b64encode() got an unexpected keyword argument 'bogus'"

try:
    base64.b64decode(b'a', b'-_', True, 9)
    assert False, 'b64decode() with 4 positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='b64decode() takes at most 3 arguments (4 given)',
        cpython='b64decode() takes from 1 to 3 positional arguments but 4 were given',
    )

try:
    base64.b64decode(b'a', s=b'x')
    assert False, 'b64decode() with a duplicated first argument should raise'
except TypeError as e:
    assert str(e) == "b64decode() got multiple values for argument 's'"

# single-parameter functions report the fixed-arity form, not a range
try:
    base64.encodebytes(b'a', b'b')
    assert False, 'encodebytes() with 2 positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='encodebytes() takes at most 1 argument (2 given)',
        cpython='encodebytes() takes 1 positional argument but 2 were given',
    )

try:
    base64.b32decode()
    assert False, 'b32decode() with no args should raise'
except TypeError as e:
    assert str(e) == "b32decode() missing 1 required positional argument: 's'"

# keyword-only parameters do not widen the positional maximum, and supplying
# any of them changes how the overflow counts what it was given
try:
    base64.a85encode(b'a', True)
    assert False, 'a85encode() with 2 positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='a85encode() takes at most 1 positional argument (2 given)',
        cpython='a85encode() takes 1 positional argument but 2 were given',
    )

try:
    base64.a85encode(b'a', True, foldspaces=True)
    assert False, 'a85encode() with 2 positionals and a keyword should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='a85encode() takes at most 1 positional argument (2 given)',
        cpython='a85encode() takes 1 positional argument but 2 positional arguments (and 1 keyword-only argument) were given',
    )

# =====================================================================
# === binascii: the C parser families base64's pure Python delegates to ===
# =====================================================================

# === METH_O: unhexlify rejects keywords before counting positionals ===
try:
    binascii.unhexlify()
    assert False, 'unhexlify() with no args should raise'
except TypeError as e:
    assert str(e) == 'binascii.unhexlify() takes exactly one argument (0 given)'

try:
    binascii.unhexlify(b'ab', 1)
    assert False, 'unhexlify() with 2 positionals should raise'
except TypeError as e:
    assert str(e) == 'binascii.unhexlify() takes exactly one argument (2 given)'

try:
    binascii.unhexlify(data=b'ab')
    assert False, 'unhexlify() with a keyword should raise'
except TypeError as e:
    assert str(e) == 'binascii.unhexlify() takes no keyword arguments'

# === crc32: positional arity and keywords ===
try:
    binascii.crc32()
    assert False, 'crc32() with no args should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="crc32() missing 1 required positional argument: 'data'",
        cpython='crc32 expected at least 1 argument, got 0',
    )

try:
    binascii.crc32(b'a', 1, 2)
    assert False, 'crc32() with 3 positionals should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty='crc32() takes at most 2 arguments (3 given)',
        cpython='crc32 expected at most 2 arguments, got 3',
    )

try:
    binascii.crc32(data=b'a')
    assert False, 'crc32() with a keyword should raise'
except TypeError as e:
    assert str(e) == expected_error(
        monty="crc32() got some positional-only arguments passed as keyword arguments: 'data'",
        cpython='binascii.crc32() takes no keyword arguments',
    )

# === check_zero_args: keywords are reported before the positional count ===
# CPython's `PyArg_NoKeywords` runs first, so a lone keyword never reports
# `(0 given)`, and a keyword alongside positionals still wins.
try:
    datetime.date(2020, 1, 1).isoformat(timespec='minutes')
    assert False, 'isoformat(timespec=) should raise'
except TypeError as e:
    assert str(e) == 'date.isoformat() takes no keyword arguments'

try:
    datetime.date(2020, 1, 1).isoformat(1, bogus=2)
    assert False, 'isoformat(1, bogus=) should raise'
except TypeError as e:
    assert str(e) == 'date.isoformat() takes no keyword arguments'

try:
    datetime.date(2020, 1, 1).isoformat(1)
    assert False, 'isoformat(1) should raise'
except TypeError as e:
    assert str(e) == 'date.isoformat() takes no arguments (1 given)'


# === bisect: keyword-capable arguments and a keyword-only key ===
for f, name in [
    (bisect.bisect_left, 'bisect_left'),
    (bisect.bisect_right, 'bisect_right'),
    (bisect.insort_left, 'insort_left'),
    (bisect.insort_right, 'insort_right'),
]:
    cases = [
        (
            (),
            {},
            TypeError,
            expected_error(
                monty=f"{name}() missing 2 required positional arguments: 'a' and 'x'",
                cpython=f"{name}() missing required argument 'a' (pos 1)",
            ),
        ),
        (
            ([],),
            {},
            TypeError,
            expected_error(
                monty=f"{name}() missing 1 required positional argument: 'x'",
                cpython=f"{name}() missing required argument 'x' (pos 2)",
            ),
        ),
        (([], 1, 0, None, None), {}, TypeError, name + '() takes at most 4 positional arguments (5 given)'),
        (([], 1), {'unknown': 1}, TypeError, name + "() got an unexpected keyword argument 'unknown'"),
        (
            ([], 1),
            {'a': []},
            TypeError,
            expected_error(
                monty=f"{name}() got multiple values for argument 'a'",
                cpython=f"argument for {name}() given by name ('a') and position (1)",
            ),
        ),
        (([], 1), {'lo': 0.5, 'oops': 1}, TypeError, name + "() got an unexpected keyword argument 'oops'"),
        (
            ([], 1, 0.5),
            {'lo': 2},
            TypeError,
            expected_error(
                monty=f"{name}() got multiple values for argument 'lo'",
                cpython=f"argument for {name}() given by name ('lo') and position (3)",
            ),
        ),
    ]
    for args, kwargs, error, message in cases:
        try:
            f(*args, **kwargs)
            assert False, 'expected failure'
        except error as exc:
            assert str(exc) == message

# === 2-arg fast path error cleanup (refcount safety for heap-allocated objects) ===
# 1. Error on arg 1 while arg 2 is a freshly allocated heap object:
try:
    'hello'.replace(123, 'a' * 100)
    assert False, 'replace(non-str, str) should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'replace() argument 1 must be str, not int'

# 2. Error on arg 2 while arg 1 was extracted into an owning field:
try:
    'hello'.replace('a' * 100, 123)
    assert False, 'replace(str, non-str) should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'replace() argument 2 must be str, not int'

# 3. Positional argument count arity checks via fast-path / fallback:
try:
    'hello'.replace()
    assert False, 'replace() with 0 args should raise TypeError'
except TypeError as exc:
    assert str(exc) == expected_error(
        monty="replace() missing 2 required positional arguments: 'old' and 'new'",
        cpython='replace() takes at least 2 positional arguments (0 given)',
    )

try:
    'hello'.replace('a')
    assert False, 'replace() with 1 arg should raise TypeError'
except TypeError as exc:
    assert str(exc) == expected_error(
        monty="replace() missing 1 required positional argument: 'new'",
        cpython='replace() takes at least 2 positional arguments (1 given)',
    )

# 4. Overflow check for ssize_t count:
try:
    'a'.replace('a', 'b', 10**30)
    assert False, 'replace with oversized count should raise OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

# 5. Invalid count type check (must keep plain __index__ integer wording):
try:
    'a'.replace('a', 'b', 'x')
    assert False, 'replace with non-int count should raise TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

try:
    'a'.replace('a', 'b', count='x')
    assert False, 'replace with non-int count kwarg should raise TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

# 6. Type name lookup on object without __index__ (preserves exact class name):
from collections import namedtuple

Point = namedtuple('Point', ['x', 'y'])
try:
    'a'.replace('a', 'b', Point(1, 2))
    assert False, 'replace with namedtuple count should raise TypeError'
except TypeError as exc:
    assert str(exc) == "'Point' object cannot be interpreted as an integer"

# 7. 32-bit boundary: count >= 2**31 must not overflow on 32-bit targets
assert 'a'.replace('a', 'b', 2**31) == 'b'
