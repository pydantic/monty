import functools
import typing
from typing import Any, Optional, Union, get_type_hints

NoneType = type(None)


class Foo:
    pass


class Bar:
    pass


# === Builtin types ===
def builtins_(
    a: int,
    b: float,
    c: str,
    d: bytes,
    e: bool,
    f: complex,
    g: list,
    h: dict,
    i: set,
    j: frozenset,
    k: tuple,
    z: type,
    m: object,
    n: range,
    o: slice,
):
    pass


assert get_type_hints(builtins_) == {
    'a': int,
    'b': float,
    'c': str,
    'd': bytes,
    'e': bool,
    'f': complex,
    'g': list,
    'h': dict,
    'i': set,
    'j': frozenset,
    'k': tuple,
    'z': type,
    'm': object,
    'n': range,
    'o': slice,
}
assert get_type_hints(builtins_)['a'] is int


# === Generic aliases ===
def generics(
    a: list[int],
    b: dict[str, int],
    c: dict[str, list[int]],
    d: tuple[int, ...],
    e: tuple[int, str, bytes],
    f: tuple[()],
    g: set[str],
    h: frozenset[int],
    i: type[int],
    j: list[list[list[str]]],
    k: dict[tuple[int, int], set[Foo]],
) -> list[dict[str, Foo]]:
    pass


assert get_type_hints(generics) == {
    'a': list[int],
    'b': dict[str, int],
    'c': dict[str, list[int]],
    'd': tuple[int, ...],
    'e': tuple[int, str, bytes],
    'f': tuple[()],
    'g': set[str],
    'h': frozenset[int],
    'i': type[int],
    'j': list[list[list[str]]],
    'k': dict[tuple[int, int], set[Foo]],
    'return': list[dict[str, Foo]],
}
hints = get_type_hints(generics)
assert hints['a'].__origin__ is list
assert hints['a'].__args__ == (int,)
assert hints['c'].__args__ == (str, list[int])


# === Unions ===
def unions(
    a: int | str,
    b: int | None,
    c: Optional[int],
    d: Union[int, str],
    e: Union[int, None],
    f: Foo | Bar | None,
    g: list[int] | dict[str, int],
    h: Union[int],
    i: Optional[list[Foo]],
) -> int | None:
    pass


assert get_type_hints(unions) == {
    'a': int | str,
    'b': int | None,
    'c': int | None,
    'd': int | str,
    'e': int | None,
    'f': Foo | Bar | None,
    'g': list[int] | dict[str, int],
    'h': int,
    'i': list[Foo] | None,
    'return': int | None,
}
assert get_type_hints(unions)['b'] == Optional[int]
assert get_type_hints(unions)['d'] == Union[str, int]


# === None ===
def nones(a: None, b: 'None', c: list[None], d: None | int) -> None:
    pass


assert get_type_hints(nones) == {'a': NoneType, 'b': NoneType, 'c': list[None], 'd': None | int, 'return': NoneType}
assert get_type_hints(nones)['a'] is NoneType
assert get_type_hints(nones)['return'] is NoneType


# a None default does not make the hint optional
def none_default(a: int = None, b: 'str' = None):
    pass


assert get_type_hints(none_default) == {'a': int, 'b': str}


# === typing markers used bare ===
def markers(a: Any, b: typing.List, c: typing.Dict, d: typing.Sequence) -> typing.Any:
    pass


assert get_type_hints(markers) == {
    'a': Any,
    'b': typing.List,
    'c': typing.Dict,
    'd': typing.Sequence,
    'return': Any,
}


# === String annotations ===
def strings(
    a: 'int',
    b: 'list[int]',
    c: 'int | None',
    d: 'Optional[Foo]',
    e: 'dict[str, Foo]',
    f: 'int ',
    g: 'tuple[int, ...]',
    h: 'Union[Foo, Bar]',
    i: 'type[Foo]',
) -> 'Foo':
    pass


assert get_type_hints(strings) == {
    'a': int,
    'b': list[int],
    'c': int | None,
    'd': Foo | None,
    'e': dict[str, Foo],
    'f': int,
    'g': tuple[int, ...],
    'h': Foo | Bar,
    'i': type[Foo],
    'return': Foo,
}


# strings nested inside real annotations, at any depth
def nested_strings(
    a: list['int'],
    b: dict['str', list['Foo']],
    c: Optional['Foo'],
    d: Union['Foo', 'Bar', None],
    e: tuple['int', 'str'],
    f: type['Foo'],
    g: 'list["dict[str, Foo]"]',
):
    pass


assert get_type_hints(nested_strings) == {
    'a': list[int],
    'b': dict[str, list[Foo]],
    'c': Foo | None,
    'd': Foo | Bar | None,
    'e': tuple[int, str],
    'f': type[Foo],
    'g': list[dict[str, Foo]],
}


# === Lazy evaluation ===
# annotations evaluate when asked, so names defined later and rebound names resolve
def later(a: 'DefinedLater', b: list['DefinedLater']) -> 'DefinedLater':
    pass


class DefinedLater:
    pass


assert get_type_hints(later) == {'a': DefinedLater, 'b': list[DefinedLater], 'return': DefinedLater}

Rebound = int


def uses_rebound(a: 'Rebound'):
    pass


assert get_type_hints(uses_rebound) == {'a': int}
Rebound = str
assert get_type_hints(uses_rebound) == {'a': str}


# === Self reference ===
def recursive(n: int, f: 'Optional[type(recursive)]' = None) -> int:
    return n


assert get_type_hints(recursive) == {'n': int, 'f': type(recursive) | None, 'return': int}


# === Parameter shapes ===
def only_return() -> int:
    pass


assert get_type_hints(only_return) == {'return': int}


def only_varargs(*args: int, **kwargs: str):
    pass


assert get_type_hints(only_varargs) == {'args': int, 'kwargs': str}


def kw_only(*, a: int, b: str = 'x') -> bool:
    pass


assert get_type_hints(kw_only) == {'a': int, 'b': str, 'return': bool}


def pos_only(a: int, b: str, /) -> bytes:
    pass


assert get_type_hints(pos_only) == {'a': int, 'b': str, 'return': bytes}


def defaults(a: int = 1, b: list[int] = [], c: 'Foo' = Foo()):
    pass


assert get_type_hints(defaults) == {'a': int, 'b': list[int], 'c': Foo}


def no_return_hint(a: int):
    pass


assert 'return' not in get_type_hints(no_return_hint)


# === Kinds of function ===
async def coroutine_fn(a: int) -> str:
    return ''


assert get_type_hints(coroutine_fn) == {'a': int, 'return': str}


def make_closure():
    captured = 1

    def inner(a: int, b: 'Foo') -> list[int]:
        return [captured]

    return inner


assert get_type_hints(make_closure()) == {'a': int, 'b': Foo, 'return': list[int]}


def make_with_defaults(default):
    def inner(a: int = default) -> int:
        return a

    return inner


assert get_type_hints(make_with_defaults(5)) == {'a': int, 'return': int}

assert get_type_hints(lambda x, y=1: x) == {}


# === Decorated functions ===
def passthrough(func):
    return func


@passthrough
def decorated(a: int) -> str:
    return ''


assert get_type_hints(decorated) == {'a': int, 'return': str}


def plain_wrapping(func):
    def wrapper(*args: int) -> bool:
        return True

    return wrapper


@plain_wrapping
def replaced(a: str) -> str:
    return a


assert get_type_hints(replaced) == {'args': int, 'return': bool}


@functools.lru_cache(maxsize=None)
def cached(a: int, b: 'str' = 'x') -> 'list[int]':
    return [a]


assert get_type_hints(cached) == {'a': int, 'b': str, 'return': list[int]}


@functools.cache
def cache_decorated(a: Foo) -> Bar:
    pass


assert get_type_hints(cache_decorated) == {'a': Foo, 'return': Bar}


# === Methods ===
class Methods:
    def instance(self, a: int) -> 'Methods':
        return self

    async def async_method(self, a: float) -> None:
        pass


obj = Methods()
assert get_type_hints(Methods.instance) == {'a': int, 'return': Methods}
assert get_type_hints(obj.instance) == {'a': int, 'return': Methods}
assert get_type_hints(Methods.async_method) == {'a': float, 'return': NoneType}


# self is in the hints only when annotated
class AnnotatedSelf:
    def method(self: 'AnnotatedSelf', a: int) -> None:
        pass


assert get_type_hints(AnnotatedSelf.method) == {'self': AnnotatedSelf, 'a': int, 'return': NoneType}


# === Non-type annotations ===
def odd(a: 1, b: len('abc'), c: [int, str], d: (int, str), e: {'k': int}, f: ...):
    pass


assert get_type_hints(odd) == {'a': 1, 'b': 3, 'c': [int, str], 'd': (int, str), 'e': {'k': int}, 'f': ...}


def computed(a: 'int' if True else 'str', b: list[int][0] if False else bytes):
    pass


assert get_type_hints(computed) == {'a': int, 'b': bytes}

# === The result is a fresh, ordinary dict ===
first = get_type_hints(builtins_)
assert type(first) is dict
first['extra'] = 1
assert first.pop('a') is int
assert 'extra' not in get_type_hints(builtins_)
assert get_type_hints(builtins_)['a'] is int
assert get_type_hints(builtins_) is not get_type_hints(builtins_)


# === Errors evaluating annotations ===
def undefined_name(a: 'Nowhere'):
    pass


try:
    get_type_hints(undefined_name)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Nowhere' is not defined"


def undefined_nested(a: list['Nowhere2']):
    pass


try:
    get_type_hints(undefined_nested)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Nowhere2' is not defined"


def undefined_plain(a: Nowhere3):
    pass


try:
    get_type_hints(undefined_plain)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Nowhere3' is not defined"


def raising(a: 1 / 0):
    pass


try:
    get_type_hints(raising)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'


def bad_attribute(a: 'Foo.missing'):
    pass


try:
    get_type_hints(bad_attribute)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "type object 'Foo' has no attribute 'missing'"

# a forward reference must compile as an expression on its own
for source in [
    ' int',
    '\tint',
    '\n int',
    '\n\tint',
    '\r\n int',
    '#c\n int',
    '\x0c int',
    '',
    '   ',
    '\n',
    '#c',
    'x = 1',
    'int +',
    'await x',
    'def f(): pass',
]:
    ns = {}
    exec(f'def f(a: {source!r}): pass', ns)
    try:
        get_type_hints(ns['f'])
        assert False, 'expected SyntaxError'
    except SyntaxError as exc:
        assert str(exc) == f'Forward reference must be an expression -- got {source!r}'

# leading blank and comment lines are skipped, and a form feed resets the indentation
for source in ['\nint', 'int\n', '\x0cint', ' \x0cint', '#c\nint', '  #c\nint', '\n   \nint', '(\n int)']:
    ns = {}
    exec(f'def f(a: {source!r}): pass', ns)
    assert get_type_hints(ns['f']) == {'a': int}

# an error does not stop later calls from working
assert get_type_hints(only_return) == {'return': int}
