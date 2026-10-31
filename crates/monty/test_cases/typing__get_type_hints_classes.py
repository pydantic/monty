import typing
from typing import Any, Optional, Union, get_type_hints

NoneType = type(None)


# The CPython harness runs this file in a bare dict, so a class's string forward
# references cannot see module globals there; those cases pass `globalns`.
class Foo:
    pass


# === Annotation kinds ===
class Builtins:
    a: int
    b: float
    c: str
    d: bytes
    e: bool
    f: complex
    g: list
    h: dict
    i: set
    j: frozenset
    k: tuple
    z: type
    m: object


assert get_type_hints(Builtins) == {
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
}


class Generics:
    a: list[int]
    b: dict[str, list[int]]
    c: tuple[int, ...]
    d: tuple[int, str]
    e: set[Foo]
    f: type[Foo]
    g: frozenset[str]


assert get_type_hints(Generics) == {
    'a': list[int],
    'b': dict[str, list[int]],
    'c': tuple[int, ...],
    'd': tuple[int, str],
    'e': set[Foo],
    'f': type[Foo],
    'g': frozenset[str],
}


class Unions:
    a: int | None
    b: Optional[str]
    c: Union[int, str, None]
    d: Foo | list[Foo]
    e: None
    f: 'None'


assert get_type_hints(Unions) == {
    'a': int | None,
    'b': str | None,
    'c': int | str | None,
    'd': Foo | list[Foo],
    'e': NoneType,
    'f': NoneType,
}


class Markers:
    a: Any
    b: typing.List
    c: typing.Mapping


assert get_type_hints(Markers) == {'a': Any, 'b': typing.List, 'c': typing.Mapping}


class Strings:
    a: 'int'
    b: 'list[str]'
    c: 'int | None'
    d: list['int']
    e: dict['str', list['bytes']]
    f: Optional['float']
    g: 'Optional[list[int]]'


assert get_type_hints(Strings, {'Optional': Optional}) == {
    'a': int,
    'b': list[str],
    'c': int | None,
    'd': list[int],
    'e': dict[str, list[bytes]],
    'f': float | None,
    'g': list[int] | None,
}


class NonTypes:
    a: 1
    b: [int, str]
    c: ...


assert get_type_hints(NonTypes) == {'a': 1, 'b': [int, str], 'c': ...}


# === What counts as annotated ===
class Mixed:
    with_value: int = 5
    without_value: str
    unannotated = 3.0
    later_value: bytes

    def method(self, a: int) -> str:
        return ''

    later_value = b''


assert get_type_hints(Mixed) == {'with_value': int, 'without_value': str, 'later_value': bytes}
assert list(get_type_hints(Mixed)) == ['with_value', 'without_value', 'later_value']
assert Mixed.with_value == 5
assert get_type_hints(Mixed.method) == {'a': int, 'return': str}


class Empty:
    pass


class OnlyMethods:
    def method(self, a: int) -> int:
        return a


class OnlyUnannotated:
    x = 1
    y = 'two'


assert get_type_hints(Empty) == {}
assert get_type_hints(OnlyMethods) == {}
assert get_type_hints(OnlyUnannotated) == {}


# a name annotated twice keeps its first position and its last annotation
class Reannotated:
    a: int
    b: str
    a: bytes


assert get_type_hints(Reannotated) == {'a': bytes, 'b': str}
assert list(get_type_hints(Reannotated)) == ['a', 'b']


# === Names from the class namespace ===
class UsesClassNames:
    Alias = int
    Other = list[str]
    plain: Alias
    forward: 'Alias'
    nested: list['Alias']
    generic: Other
    optional: Optional['Other']


assert get_type_hints(UsesClassNames) == {
    'plain': int,
    'forward': int,
    'nested': list[int],
    'generic': list[str],
    'optional': list[str] | None,
}


# forward references read the class namespace when asked, so rebinding is seen
class Rebinding:
    Alias = int
    forward: 'Alias'


assert get_type_hints(Rebinding) == {'forward': int}
Rebinding.Alias = str
assert get_type_hints(Rebinding) == {'forward': str}


# === Self and mutual references ===
class Node:
    value: int
    parent: Optional['Node']
    children: list['Node']
    lookup: dict[str, 'Node']
    me: 'Node'


assert get_type_hints(Node, {'Node': Node}) == {
    'value': int,
    'parent': Node | None,
    'children': list[Node],
    'lookup': dict[str, Node],
    'me': Node,
}


class Left:
    right: 'Right'
    rights: 'list[Right]'


class Right:
    left: 'Left'


assert get_type_hints(Left, {'Right': Right}) == {'right': Right, 'rights': list[Right]}
assert get_type_hints(Right, {'Left': Left}) == {'left': Left}


# a plain annotation naming a later class evaluates when asked
class PlainLater:
    later: DefinedAfter
    many: list[DefinedAfter]


class DefinedAfter:
    pass


assert get_type_hints(PlainLater) == {'later': DefinedAfter, 'many': list[DefinedAfter]}


# === Classes made in functions ===
def make_class():
    class Made:
        a: int
        b: list[str]
        c: 'bytes'

    return Made


made = make_class()
assert get_type_hints(made) == {'a': int, 'b': list[str], 'c': bytes}
assert get_type_hints(make_class()) == get_type_hints(made)
assert make_class() is not made


# === Classes as hints ===
class Holder:
    exc: ValueError
    kind: type[Foo]
    cls: Foo


assert get_type_hints(Holder) == {'exc': ValueError, 'kind': type[Foo], 'cls': Foo}
assert get_type_hints(Holder)['cls'] is Foo


# === Instances have no annotations ===
class WithRepr:
    x: int

    def __repr__(self):
        return 'WithRepr()'


try:
    get_type_hints(WithRepr())
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'WithRepr() does not have annotations'

# === The result is a fresh dict ===
hints = get_type_hints(Builtins)
hints['extra'] = 1
assert 'extra' not in get_type_hints(Builtins)
assert get_type_hints(Builtins) is not get_type_hints(Builtins)


# === Errors ===
class Undefined:
    ok: int
    missing: 'NotDefinedAnywhere'


try:
    get_type_hints(Undefined)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'NotDefinedAnywhere' is not defined"


class UndefinedPlain:
    missing: NotDefinedPlain


try:
    get_type_hints(UndefinedPlain)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'NotDefinedPlain' is not defined"


class Raises:
    bad: 1 / 0


try:
    get_type_hints(Raises)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'


ns = {}
exec("class NotAnExpression:\n    bad: 'x = 1'", ns)
try:
    get_type_hints(ns['NotAnExpression'])
    assert False, 'expected SyntaxError'
except SyntaxError as exc:
    assert str(exc) == "Forward reference must be an expression -- got 'x = 1'"
