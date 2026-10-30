# Under PEP 563 every annotation is a string, so `get_type_hints` evaluates each
# one in the given namespaces only, not where it was written.
from __future__ import annotations

from annotationlib import Format, ForwardRef, get_annotations
from dataclasses import dataclass
from typing import get_type_hints

NoneType = type(None)


class Foo:
    pass


# === Functions ===
def f(a: Later, b: 'Later', c: list[Later], d: Foo, e: 'Foo', g: None, h: 1) -> Foo | None:
    pass


strings = {
    'a': 'Later',
    'b': "'Later'",
    'c': 'list[Later]',
    'd': 'Foo',
    'e': "'Foo'",
    'g': 'None',
    'h': '1',
    'return': 'Foo | None',
}
assert get_type_hints(f, format=Format.STRING) == strings
assert get_annotations(f) == strings
assert get_annotations(f, format=Format.FORWARDREF) == strings
assert get_annotations(f, format=Format.STRING) == strings

# a string naming an unresolved name stays its reference; one evaluating to a
# string makes a new reference owned by the function; a name inside a larger
# expression is a reference remembering the namespace, equal to no constructed one
forward = get_type_hints(f, format=Format.FORWARDREF)
assert list(forward) == list(strings)
assert forward['a'] == ForwardRef('Later')
assert forward['b'] == ForwardRef('Later', owner=f)
assert forward['c'].__origin__ is list
assert forward['c'].__args__[0].__forward_arg__ == 'Later'
assert forward['c'].__args__[0].__owner__ is f
assert forward['c'].__args__[0] != forward['b']
assert forward['d'] is Foo
assert forward['e'] is Foo
assert forward['g'] is NoneType
assert forward['h'] == 1
assert forward['return'] == Foo | None
try:
    get_type_hints(f)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Later' is not defined"

resolved = {'a': int, 'b': int, 'c': list[int], 'd': Foo, 'e': Foo, 'g': NoneType, 'h': 1, 'return': Foo | None}
assert get_type_hints(f, {'Later': int, 'Foo': Foo}) == resolved
assert get_type_hints(f, None, {'Later': int, 'Foo': Foo}) == resolved
assert get_type_hints(f, {'Later': str}, {'Later': int, 'Foo': Foo}) == resolved
Later = int
assert get_type_hints(f) == resolved
assert get_type_hints(f, format=Format.FORWARDREF) == resolved
assert get_annotations(f, eval_str=True) == {
    'a': int,
    'b': 'Later',
    'c': list[int],
    'd': Foo,
    'e': 'Foo',
    'g': None,
    'h': 1,
    'return': Foo | None,
}


# explicit namespaces are the only ones searched, unlike an evaluated annotation
def g(x: Foo, y: int):
    pass


assert get_type_hints(g, {'Foo': bytes}) == {'x': bytes, 'y': int}
assert get_type_hints(g, None, {'Foo': bytes}) == {'x': bytes, 'y': int}
try:
    get_type_hints(g, {})
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Foo' is not defined"


# === Classes ===
# The CPython harness runs this file in a bare dict, so a class's forward
# references cannot see module globals there; these cases pass namespaces.
class C:
    x: Later
    y: 'Later'
    z: int
    w: None
    v: Foo


class_strings = {'x': 'Later', 'y': "'Later'", 'z': 'int', 'w': 'None', 'v': 'Foo'}
assert get_type_hints(C, format=Format.STRING) == class_strings
assert get_annotations(C) == class_strings
assert get_type_hints(C, {'Later': int, 'Foo': Foo}) == {'x': int, 'y': int, 'z': int, 'w': NoneType, 'v': Foo}
assert get_type_hints(C, None, {'Later': str, 'Foo': Foo}) == {'x': str, 'y': str, 'z': int, 'w': NoneType, 'v': Foo}
class_forward = get_type_hints(C, {}, format=Format.FORWARDREF)
assert class_forward['x'] == ForwardRef('Later', is_class=True)
assert class_forward['y'] == ForwardRef('Later', is_class=True, owner=C)
assert class_forward['z'] is int
assert class_forward['w'] is NoneType
assert class_forward['v'] == ForwardRef('Foo', is_class=True)
try:
    get_type_hints(C, {})
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Later' is not defined"
assert get_annotations(C, eval_str=True, globals={'Later': int, 'Foo': Foo}) == {
    'x': int,
    'y': 'Later',
    'z': int,
    'w': None,
    'v': Foo,
}


# a class namespace still shadows the module for its own names
class Shadowed:
    Later = str
    plain: Later


assert get_type_hints(Shadowed, {'Later': int}) == {'plain': str}


# === Dataclasses keep working ===
@dataclass
class D:
    name: str
    tags: list[str]
    count: int = 0


assert D('a', []).tags == []
assert get_type_hints(D) == {'name': str, 'tags': list[str], 'count': int}
