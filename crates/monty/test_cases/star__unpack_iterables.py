# Any value Monty can iterate may follow a `*`, matching CPython.
#
# Monty previously accepted only list/tuple/set/dict/str after a `*`, so
# `[*range(3)]` raised TypeError even though `list(range(3))` worked. These
# assertions pin the wider set, across every syntactic form that unpacks.

import sys

d = {'a': 1, 'b': 2}

# === List and tuple literals ===
assert [*range(3)] == [0, 1, 2]
assert [*frozenset([1])] == [1]
assert [*b'ab'] == [97, 98]
assert [*d.keys()] == ['a', 'b']
assert [*d.values()] == [1, 2]
assert [*d.items()] == [('a', 1), ('b', 2)]
assert (*range(3),) == (0, 1, 2)

# a namedtuple unpacks as the tuple it subclasses (only major/minor are
# compared — Monty and CPython differ on the micro version)
assert [*sys.version_info][:2] == [3, 14]
assert len([*sys.version_info]) == 5

# === Set literals ===
assert {*range(3)} == {0, 1, 2}
assert {*b'ab'} == {97, 98}
assert {*d.keys()} == {'a', 'b'}

# === Iterators, including the two-argument `iter()` form ===
assert [*iter([1, 2])] == [1, 2]

calls: list[int] = []


def step() -> int:
    calls.append(1)
    return len(calls)


assert [*iter(step, 3)] == [1, 2]

# === Mixed with other elements, and repeated ===
assert [0, *range(1, 3), 3] == [0, 1, 2, 3]
assert [*range(2), *range(2)] == [0, 1, 0, 1]
assert [*'ab', *range(2)] == ['a', 'b', 0, 1]


# === Call-site unpacking ===
def _add3(a, b, c):
    return a + b + c


assert _add3(*range(3)) == 3
assert _add3(*b'\x01\x02\x03') == 6

# === Sequence unpacking (assignment targets) ===
x, y, z = range(3)
assert (x, y, z) == (0, 1, 2)

first, *rest = range(4)
assert first == 0
assert rest == [1, 2, 3]

*init, last = b'ab'
assert init == [97]
assert last == 98

# === Empty iterables ===
assert [*range(0)] == []
assert [*frozenset()] == []

# === Non-iterables still raise, with each site's own message ===
_big = 2**70

try:
    [*_big]
    assert False, 'expected list unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'Value after * must be an iterable, not int'

try:
    {*_big}
    assert False, 'expected set unpack of an int to raise'
except TypeError as e:
    assert str(e) == "'int' object is not iterable"

try:
    _p, _q = _big
    assert False, 'expected sequence unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'cannot unpack non-iterable int object'

try:
    (*_big,)
    assert False, 'expected tuple unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'Value after * must be an iterable, not int'

try:
    _r, *_s = _big
    assert False, 'expected starred-target unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'cannot unpack non-iterable int object'

# `f(*non_iterable)` is deliberately not asserted here: Monty reports the
# list-literal message where CPython names the function, a divergence that
# predates this change (see limitations/language.md).

# === "too many values" reports a total only for an exact list/tuple/dict ===
# CPython unpacks those three without the iterator protocol, so it knows the
# length. Every other type stops at the first surplus item and never learns the
# total. CPython excludes subclasses of those three as well, which is not
# asserted here because Monty has no class inheritance yet.
d3 = {1: 'a', 2: 'b', 3: 'c'}

for src in ([1, 2, 3], (1, 2, 3), d3):
    try:
        _a, _b = src
        raise AssertionError('expected too many values')
    except ValueError as e:
        assert str(e) == 'too many values to unpack (expected 2, got 3)'

for src in ('abc', b'abc', {1, 2, 3}, frozenset([1, 2, 3]), d3.keys(), range(3), iter([1, 2, 3])):
    try:
        _a, _b = src
        raise AssertionError('expected too many values')
    except ValueError as e:
        assert str(e) == 'too many values to unpack (expected 2)'

# Too *few* always carries the total: the iterable was drained, so the real
# length is known whatever the source type.
for src in ([1], (1,), 'a', b'a', {1}, range(1), iter([1])):
    try:
        _a, _b = src
        raise AssertionError('expected not enough values')
    except ValueError as e:
        assert str(e) == 'not enough values to unpack (expected 2, got 1)'

# A starred target drains in full, so it always knows the total too.
for src in ([1], (1,), 'a', range(1), iter([1])):
    try:
        _a, _b, *_rest = src
        raise AssertionError('expected not enough values')
    except ValueError as e:
        assert str(e) == 'not enough values to unpack (expected at least 2, got 1)'


# === Heap-allocated values take a different path from interned literals ===
# A `bytes`/`str` literal is interned and never reaches the heap; a computed one
# is a heap value resolved through a different arm at every unpacking site.
_heap_bytes = b'a' + b'b'
_heap_str = 'a' + 'b'


def _add2(a, b):
    return a + b


assert [*_heap_bytes] == [97, 98]
assert [*_heap_str] == ['a', 'b']
assert {*_heap_bytes} == {97, 98}
assert (*_heap_bytes,) == (97, 98)
assert _add2(*_heap_bytes) == 195
_hb1, _hb2 = _heap_bytes
assert (_hb1, _hb2) == (97, 98)
_hb3, *_hbrest = _heap_bytes
assert (_hb3, _hbrest) == (97, [98])

# === Every unpacking form agrees with `list()` on what is iterable ===
# The property that matters: iterability is one answer, not six. Each form is a
# separate site in the VM, so a type that reports iterable in one place and not
# another is exactly the drift this guards against.


def _accepts_list(v: object) -> bool:
    try:
        list(v)
        return True
    except TypeError:
        return False


def _accepts_list_star(v: object) -> bool:
    try:
        [*v]
        return True
    except TypeError:
        return False


def _accepts_tuple_star(v: object) -> bool:
    try:
        (*v,)
        return True
    except TypeError:
        return False


def _accepts_set_star(v: object) -> bool:
    try:
        {*v}
        return True
    except TypeError:
        return False


def _accepts_call_star(v: object) -> bool:
    try:
        _varargs(*v)
        return True
    except TypeError:
        return False


def _accepts_seq_unpack(v: object) -> bool:
    try:
        (_only,) = v
        return True
    except TypeError:
        return False
    except ValueError:
        # Iterated fine, just not one item - still iterable.
        return True


def _accepts_ex_unpack(v: object) -> bool:
    try:
        (_head, *_tail) = v
        return True
    except TypeError:
        return False
    except ValueError:
        return True


def _varargs(*args: object) -> object:
    return args


# Built fresh per probe so a one-shot iterator is not exhausted by an earlier form.
def _probe_values() -> list[object]:
    _d = {'a': 1, 'b': 2}
    seen: list[int] = []

    def probe_step() -> int:
        seen.append(1)
        return len(seen)

    return [
        [1, 2],
        (1, 2),
        {1, 2},
        frozenset([1, 2]),
        {1: 'x', 2: 'y'},
        _d.keys(),
        _d.values(),
        _d.items(),
        'ab',
        b'ab',
        b'a' + b'b',
        'a' + 'b',
        range(2),
        iter([1, 2]),
        iter(probe_step, 3),
        sys.version_info,
        1,
        2**70,
        1.5,
        None,
        True,
        len,
        slice(1),
        ...,
    ]


_forms = [
    _accepts_list_star,
    _accepts_tuple_star,
    _accepts_set_star,
    _accepts_call_star,
    _accepts_seq_unpack,
    _accepts_ex_unpack,
]

for _i in range(len(_probe_values())):
    _expected = _accepts_list(_probe_values()[_i])
    for _form in _forms:
        _got = _form(_probe_values()[_i])
        assert _got == _expected, 'every unpacking form must agree with list() on iterability'
