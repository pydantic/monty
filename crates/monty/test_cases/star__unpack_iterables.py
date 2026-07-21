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
def add3(a, b, c):
    return a + b + c


assert add3(*range(3)) == 3
assert add3(*b'\x01\x02\x03') == 6

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
big = 2**70

try:
    [*big]
    assert False, 'expected list unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'Value after * must be an iterable, not int'

try:
    {*big}
    assert False, 'expected set unpack of an int to raise'
except TypeError as e:
    assert str(e) == "'int' object is not iterable"

try:
    _p, _q = big
    assert False, 'expected sequence unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'cannot unpack non-iterable int object'

try:
    (*big,)
    assert False, 'expected tuple unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'Value after * must be an iterable, not int'

try:
    _r, *_s = big
    assert False, 'expected starred-target unpack of an int to raise'
except TypeError as e:
    assert str(e) == 'cannot unpack non-iterable int object'

# `f(*non_iterable)` is deliberately not asserted here: Monty reports the
# list-literal message where CPython names the function, a divergence that
# predates this change (see limitations/language.md).

# The blocks above pin the exact wording; this pins it across the kinds of value
# that reach the check. `5`/`None`/`1.5`/`True` are immediates while a big int
# lives on the heap, and each site used to answer the two from a separate arm
# with its own copy of the message.
# typed `object` so the unpacks below are checked the same way as the helpers
# further down, rather than being flagged for the `None` case alone
bad_values: list[tuple[object, str]] = [(5, 'int'), (big, 'int'), (None, 'NoneType'), (1.5, 'float'), (True, 'bool')]

for bad, name in bad_values:
    try:
        [*bad]
        raise AssertionError('expected list unpack to raise')
    except TypeError as e:
        assert str(e) == f'Value after * must be an iterable, not {name}'
    try:
        (*bad,)
        raise AssertionError('expected tuple unpack to raise')
    except TypeError as e:
        assert str(e) == f'Value after * must be an iterable, not {name}'
    try:
        {*bad}
        raise AssertionError('expected set unpack to raise')
    except TypeError as e:
        assert str(e) == f"'{name}' object is not iterable"
    try:
        _p, _q = bad
        raise AssertionError('expected sequence unpack to raise')
    except TypeError as e:
        assert str(e) == f'cannot unpack non-iterable {name} object'
    try:
        _r, *_s = bad
        raise AssertionError('expected starred-target unpack to raise')
    except TypeError as e:
        assert str(e) == f'cannot unpack non-iterable {name} object'

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
heap_bytes = b'a' + b'b'
heap_str = 'a' + 'b'


def add2(a, b):
    return a + b


assert [*heap_bytes] == [97, 98]
assert [*heap_str] == ['a', 'b']
assert {*heap_bytes} == {97, 98}
assert (*heap_bytes,) == (97, 98)
assert add2(*heap_bytes) == 195
hb1, hb2 = heap_bytes
assert (hb1, hb2) == (97, 98)
hb3, *hbrest = heap_bytes
assert (hb3, hbrest) == (97, [98])

# === The five types that used to have a fast path, at every site ===
# list/tuple/set/dict/str were read straight out of their backing storage by
# each unpacking site; they now go through the iteration protocol like anything
# else. These pin the items themselves, not just that iteration was accepted —
# the rest of this file leans on types that never had a fast path to lose.
lst = [1, 2]
tup = (1, 2)
st = {1, 2}
dct = {1: 'a', 2: 'b'}
txt = 'ab'

# a set has no defined order, so compare sorted
assert [*lst] == [1, 2]
assert [*tup] == [1, 2]
assert sorted([*st]) == [1, 2]
assert [*dct] == [1, 2]
assert [*txt] == ['a', 'b']

assert (*lst,) == (1, 2)
assert (*tup,) == (1, 2)
assert tuple(sorted((*st,))) == (1, 2)
assert (*dct,) == (1, 2)
assert (*txt,) == ('a', 'b')

assert {*lst} == {1, 2}
assert {*tup} == {1, 2}
assert {*st} == {1, 2}
assert {*dct} == {1, 2}
assert {*txt} == {'a', 'b'}

assert add2(*lst) == 3
assert add2(*tup) == 3
assert add2(*st) == 3
assert add2(*dct) == 3
assert add2(*txt) == 'ab'

u1, u2 = lst
assert (u1, u2) == (1, 2)
u1, u2 = tup
assert (u1, u2) == (1, 2)
u1, u2 = st
assert {u1, u2} == {1, 2}
u1, u2 = dct
assert (u1, u2) == (1, 2)
u1, u2 = txt
assert (u1, u2) == ('a', 'b')

u1, *urest = lst
assert (u1, urest) == (1, [2])
u1, *urest = tup
assert (u1, urest) == (1, [2])
u1, *urest = st
assert {u1, *urest} == {1, 2}
assert len(urest) == 1
u1, *urest = dct
assert (u1, urest) == (1, [2])
u1, *urest = txt
assert (u1, urest) == ('a', ['b'])

# Emptiness and single elements go down the same path
assert [*[]] == []
assert [*()] == []
assert [*{}] == []
assert [*''] == []
assert [*[1]] == [1]

# Nested containers are yielded by reference, not flattened or copied
inner = [1]
assert [*[inner]][0] is inner
assert [*(inner,)][0] is inner
assert [*{'k': inner}.values()][0] is inner

# === Every unpacking form agrees with `list()` on what is iterable ===
# The property that matters: iterability is one answer, not six. Each form is a
# separate site in the VM, so a type that reports iterable in one place and not
# another is exactly the drift this guards against.


def accepts_list(v: object) -> bool:
    try:
        list(v)
        return True
    except TypeError:
        return False


def accepts_list_star(v: object) -> bool:
    try:
        [*v]
        return True
    except TypeError:
        return False


def accepts_tuple_star(v: object) -> bool:
    try:
        (*v,)
        return True
    except TypeError:
        return False


def accepts_set_star(v: object) -> bool:
    try:
        {*v}
        return True
    except TypeError:
        return False


def accepts_call_star(v: object) -> bool:
    try:
        varargs(*v)
        return True
    except TypeError:
        return False


def accepts_seq_unpack(v: object) -> bool:
    try:
        (_only,) = v
        return True
    except TypeError:
        return False
    except ValueError:
        # Iterated fine, just not one item - still iterable.
        return True


def accepts_ex_unpack(v: object) -> bool:
    try:
        (_head, *_tail) = v
        return True
    except TypeError:
        return False
    except ValueError:
        return True


def varargs(*args: object) -> object:
    return args


# Built fresh per probe so a one-shot iterator is not exhausted by an earlier form.
def probe_values() -> list[object]:
    d = {'a': 1, 'b': 2}
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
        d.keys(),
        d.values(),
        d.items(),
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


forms = [
    accepts_list_star,
    accepts_tuple_star,
    accepts_set_star,
    accepts_call_star,
    accepts_seq_unpack,
    accepts_ex_unpack,
]

for i in range(len(probe_values())):
    expected = accepts_list(probe_values()[i])
    for form in forms:
        got = form(probe_values()[i])
        assert got == expected, 'every unpacking form must agree with list() on iterability'
