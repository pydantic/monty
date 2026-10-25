import functools
import typing
from dataclasses import dataclass
from typing import Optional, get_type_hints


class Foo:
    pass


# === Functions ===
def basic(a: int, b: 'str', c: 'Foo') -> None:
    pass


assert get_type_hints(basic) == {'a': int, 'b': str, 'c': Foo, 'return': type(None)}


def unannotated(a, b=1):
    pass


assert get_type_hints(unannotated) == {}
assert get_type_hints(lambda x: x) == {}


def partly(a, b: int, c=1):
    pass


assert get_type_hints(partly) == {'b': int}


# === Parameter kinds and key order ===
# regular parameters come before positional-only ones, then *args, kw-only, **kwargs
def kinds(a: int, /, b: str, *args: float, c: bytes, d: bool = True, **kw: list) -> 'Foo':
    pass


assert list(get_type_hints(kinds)) == ['b', 'a', 'args', 'c', 'd', 'kw', 'return']
assert get_type_hints(kinds) == {
    'b': str,
    'a': int,
    'args': float,
    'c': bytes,
    'd': bool,
    'kw': list,
    'return': Foo,
}


# === Nested forward references ===
def nested(
    a: list['Foo'],
    b: 'list["Foo"]',
    c: dict[str, list['Foo']],
    d: Optional['Foo'],
    e: 'int | None',
    f: 'None',
    g: None,
) -> 'Optional["Foo"]':
    pass


assert get_type_hints(nested) == {
    'a': list[Foo],
    'b': list[Foo],
    'c': dict[str, list[Foo]],
    'd': Foo | None,
    'e': int | None,
    'f': type(None),
    'g': type(None),
    'return': Foo | None,
}

# a string naming a string is followed
Alias = 'int'


def via_alias(x: 'Alias'):
    pass


assert get_type_hints(via_alias) == {'x': int}


# a non-type annotation is returned as it evaluates
def literal(x: 1, y: 'Foo()'):
    pass


hints = get_type_hints(literal)
assert hints['x'] == 1
assert type(hints['y']) is Foo


# === Methods ===
class WithMethod:
    def method(self, other: 'WithMethod') -> int:
        return 1


assert get_type_hints(WithMethod.method) == {'other': WithMethod, 'return': int}
assert get_type_hints(WithMethod().method) == {'other': WithMethod, 'return': int}


# === Classes ===
# The CPython harness runs this file in a bare dict, so a class's module globals are
# out of reach there; names resolve through the class namespace, builtins or `globalns`.
class Point:
    Unit = float
    x: int
    y: 'Unit'
    z: list['Unit']
    w: Optional['Unit'] = None


assert get_type_hints(Point) == {'x': int, 'y': float, 'z': list[float], 'w': float | None}
assert get_type_hints(Foo) == {}


class Forward:
    y: 'Later'
    z: list['Later']


class Later:
    pass


assert get_type_hints(Forward, {'Later': Later}) == {'y': Later, 'z': list[Later]}
# a plain annotation evaluates where it was written; only forward references
# use the namespaces given, and with `globalns` the class namespace is searched first
Target = int


class Shadowed:
    Target = str
    plain: Target
    forward: 'Target'


assert get_type_hints(Shadowed, {'Target': int}) == {'plain': str, 'forward': str}
assert get_type_hints(Shadowed, None, {'Target': bytes}) == {'plain': str, 'forward': bytes}


@dataclass
class Data:
    name: str
    tags: list[str]
    parent: 'Optional[Data]' = None


assert get_type_hints(Data, {'Optional': Optional, 'Data': Data}) == {
    'name': str,
    'tags': list[str],
    'parent': Data | None,
}


# === Wrappers ===
@functools.lru_cache
def cached(a: int) -> str:
    return str(a)


assert get_type_hints(cached) == {'a': int, 'return': str}

# === Objects without annotations ===
assert get_type_hints(len) == {}
assert get_type_hints(int) == {}
assert get_type_hints(typing) == {}
assert get_type_hints(functools.partial(basic, 1)) == {}

for value in [5, 'abc', None, [1]]:
    try:
        get_type_hints(value)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f'{value!r} does not have annotations'


# === Explicit namespaces ===
def missing(x: 'Missing') -> 'Other':
    pass


try:
    get_type_hints(missing)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Missing' is not defined"

assert get_type_hints(missing, {'Missing': int, 'Other': str}) == {'x': int, 'return': str}
assert get_type_hints(missing, None, {'Missing': bytes, 'Other': float}) == {'x': bytes, 'return': float}
assert get_type_hints(missing, {'Missing': int}, {'Other': str}) == {'x': int, 'return': str}
# a dict globalns still sees builtins, and only forward references use it
assert get_type_hints(basic, {'Foo': bytes}) == {'a': int, 'b': str, 'c': bytes, 'return': type(None)}


def plain(x: Foo, y: 'Foo'):
    pass


assert get_type_hints(plain, {'Foo': bytes}) == {'x': Foo, 'y': bytes}


class MissingAttr:
    a: 'Missing2'


try:
    get_type_hints(MissingAttr)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Missing2' is not defined"

assert get_type_hints(MissingAttr, localns={'Missing2': int}) == {'a': int}
assert get_type_hints(MissingAttr, {'Missing2': str}) == {'a': str}
assert get_type_hints(MissingAttr, {'Missing2': str}, {'Missing2': bytes}) == {'a': bytes}
assert get_type_hints(basic, include_extras=True) == get_type_hints(basic)

# === Functions defined by exec ===
ns = {}
exec('def exec_defined(x: "T") -> int: pass\nT = float', ns)
assert get_type_hints(ns['exec_defined']) == {'x': float, 'return': int}

# === Repeated calls ===
for _ in range(1000):
    hints = get_type_hints(kinds)
assert hints['return'] is Foo

# === Expressions CPython rejects in annotations ===
# lambda bodies and comprehensions are their own scopes, so they are not checked
rejected = [
    ('def f(x: (y := int)): pass', 'named expression cannot be used within an annotation (<string>, line 1)'),
    ('def f() -> (y := int): pass', 'named expression cannot be used within an annotation (<string>, line 1)'),
    ('def f(*a: (y := int)): pass', 'named expression cannot be used within an annotation (<string>, line 1)'),
    ('class C:\n    x: (y := int)', 'named expression cannot be used within an annotation (<string>, line 2)'),
    ('x: (y := int) = 1', 'named expression cannot be used within an annotation (<string>, line 1)'),
    (
        'def g():\n    def f(x: (yield)): pass',
        'yield expression cannot be used within an annotation (<string>, line 2)',
    ),
    (
        'def g():\n    def f(x: (yield from z)): pass',
        'yield expression cannot be used within an annotation (<string>, line 2)',
    ),
    (
        'async def g():\n    def f(x: await z): pass',
        'await expression cannot be used within an annotation (<string>, line 2)',
    ),
    ('def f(x: (yield) | (y := 1)): pass', 'yield expression cannot be used within an annotation (<string>, line 1)'),
]
for source, message in rejected:
    try:
        exec(source)
        assert False, 'expected SyntaxError'
    except SyntaxError as exc:
        assert str(exc) == message

exec('def f(x: lambda: (y := 1)): pass')
exec('def f(x: [(y := 1) for _ in z]): pass')
