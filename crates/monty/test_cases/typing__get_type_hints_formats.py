from annotationlib import Format, ForwardRef
from typing import Optional, get_type_hints

NoneType = type(None)


class Foo:
    pass


def f(
    a: Later,
    b: 'Later',
    c: list[Later],
    d: 'list[Later]',
    e: Later | None,
    g: Optional['Later'],
    h: Foo.missing,
    i: 'Foo.missing',
    j: int | Later,
    k: Later[int],
    m: Later(1, x=Foo),
    n: 'Foo',
    o: None,
    p: 1,
    q: list['Foo'],
    r: Later.attr,
    s: -Later,
    t: Later + 1,
    u: (Later | None)[int],
    v: Later[1, 2],
    w: Later[1:2],
    x: dict[str, Later | None],
    y: list[Later] | None,
    z: Later**2 * 3,
) -> dict[str, Later]:
    pass


# === STRING: the annotation source, a string literal unquoted ===
strings = {
    'a': 'Later',
    'b': 'Later',
    'c': 'list[Later]',
    'd': 'list[Later]',
    'e': 'Later | None',
    'g': "Optional['Later']",
    'h': 'Foo.missing',
    'i': 'Foo.missing',
    'j': 'int | Later',
    'k': 'Later[int]',
    'm': 'Later(1, x=Foo)',
    'n': 'Foo',
    'o': 'None',
    'p': '1',
    'q': "list['Foo']",
    'r': 'Later.attr',
    's': '-Later',
    't': 'Later + 1',
    'u': '(Later | None)[int]',
    'v': 'Later[1, 2]',
    'w': 'Later[1:2]',
    'x': 'dict[str, Later | None]',
    'y': 'list[Later] | None',
    'z': 'Later ** 2 * 3',
    'return': 'dict[str, Later]',
}
assert get_type_hints(f, format=Format.STRING) == strings
assert get_type_hints(f, format=4) == strings
assert get_type_hints(f, format=Format.STRING, include_extras=True) == strings
assert get_type_hints(f, {'Later': int}, format=Format.STRING) == strings


# === FORWARDREF: an unresolved name becomes a ForwardRef, at any depth ===
# A reference made while evaluating remembers the namespace it was evaluated
# against and equals no constructed reference, so those are checked by text.
def text(ref):
    return (ref.__resolved_str__, ref.__owner__, ref.__forward_is_class__)


hints = get_type_hints(f, format=Format.FORWARDREF)
assert list(hints) == list(strings)
# a name in an evaluated annotation is owned by the function, a string annotation is not
assert text(hints['a']) == ('Later', f, False)
assert hints['a'] != ForwardRef('Later', owner=f)
assert hints['b'] == ForwardRef('Later')
assert hints['b'].__owner__ is None
assert hints['c'] == list[hints['a']]
assert hints['d'] == list[hints['a']]
# operators on an unresolved name build the expression's text
assert text(hints['e']) == ('Later | None', f, False)
assert hints['g'] == ForwardRef('Later') | None
assert text(hints['h']) == ('Foo.missing', f, False)
assert hints['i'] == ForwardRef('Foo.missing')
assert text(hints['j']) == ('int | Later', f, False)
assert text(hints['k']) == ('Later[int]', f, False)
assert hints['n'] is Foo
assert hints['o'] is NoneType
assert hints['p'] == 1
assert hints['q'] == list[Foo]
assert text(hints['r']) == ('Later.attr', f, False)
assert text(hints['s']) == ('-Later', f, False)
assert text(hints['t']) == ('Later + 1', f, False)
assert text(hints['u']) == ('(Later | None)[int]', f, False)
assert text(hints['v']) == ('Later[1, 2]', f, False)
assert text(hints['w']) == ('Later[1:2]', f, False)
assert hints['x'] == dict[str, hints['e']]
assert hints['y'].__args__ == (list[hints['a']], NoneType)
assert text(hints['z']) == ('Later ** 2 * 3', f, False)
assert hints['return'] == dict[str, hints['a']]
# (CPython's union equality misses a nested alias holding a reference, so `y` is left out)
comparable = {k: v for k, v in hints.items() if k != 'y'}
assert {k: v for k, v in get_type_hints(f, format=3).items() if k != 'y'} == comparable
assert {
    k: v for k, v in get_type_hints(f, format=Format.FORWARDREF, include_extras=True).items() if k != 'y'
} == comparable

# explicit namespaces resolve the references
resolved = get_type_hints(f, {'Later': float, 'Foo': Foo}, format=Format.FORWARDREF)
assert resolved['a'] is float
assert resolved['b'] is float
assert resolved['c'] == list[float]
assert resolved['d'] == list[float]
assert resolved['e'] == float | None
assert resolved['g'] == float | None
assert text(resolved['h']) == ('Foo.missing', f, False)
assert resolved['i'] == ForwardRef('Foo.missing')
assert resolved['j'] == int | float
assert text(resolved['k']) == ('Later[int]', f, False)
assert resolved['x'] == dict[str, float | None]
assert resolved['y'] == list[float] | None
assert resolved['return'] == dict[str, float]

# so does defining the name; what stays a reference now remembers the module
# globals rather than the dict passed above, so those compare by text
Later = float
defined = get_type_hints(f, format=Format.FORWARDREF)
assert {k: v for k, v in defined.items() if not isinstance(v, ForwardRef)} == {
    k: v for k, v in resolved.items() if not isinstance(v, ForwardRef)
}
assert defined['i'] == ForwardRef('Foo.missing')
for key in ['h', 'k', 'm', 'r', 's', 't', 'u', 'v', 'w', 'z']:
    assert text(defined[key]) == text(resolved[key])


# === FORWARDREF on classes ===
class C:
    x: Later2
    y: 'Later2'
    z: list['Later2']
    w: None
    v: int


class_hints = get_type_hints(C, format=Format.FORWARDREF)
assert text(class_hints['x']) == ('Later2', C, True)
assert {k: v for k, v in class_hints.items() if k != 'x'} == {
    'y': ForwardRef('Later2', is_class=True),
    'z': list[ForwardRef('Later2')],
    'w': NoneType,
    'v': int,
}
assert get_type_hints(C, format=Format.STRING) == {
    'x': 'Later2',
    'y': 'Later2',
    'z': "list['Later2']",
    'w': 'None',
    'v': 'int',
}
assert get_type_hints(C, localns={'Later2': int}, format=Format.FORWARDREF) == {
    'x': int,
    'y': int,
    'z': list[int],
    'w': NoneType,
    'v': int,
}
assert get_type_hints(C, {'Later2': bytes}, format=Format.FORWARDREF)['x'] is bytes


# === An annotation that raises falls back to its text ===
def raising_attr(x: Foo.missing, y: list[int]):
    pass


raising_hints = get_type_hints(raising_attr, format=Format.FORWARDREF)
assert text(raising_hints['x']) == ('Foo.missing', raising_attr, False)
assert raising_hints['y'] == list[int]
try:
    get_type_hints(raising_attr)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "type object 'Foo' has no attribute 'missing'"


def raising_zero(x: 1 / 0):
    pass


try:
    get_type_hints(raising_zero, format=Format.FORWARDREF)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'


def raising_strings(x: 'Later3[int]', y: 'Foo.missing', z: 'list[Foo.missing]', w: '1/0'):
    pass


string_hints = get_type_hints(raising_strings, format=Format.FORWARDREF)
assert text(string_hints['x']) == ('Later3[int]', raising_strings, False)
assert string_hints['y'] == ForwardRef('Foo.missing')
assert string_hints['z'] == ForwardRef('list[Foo.missing]')
assert string_hints['w'] == ForwardRef('1/0')

# === A string naming itself is a ForwardRef in every format ===
a = 'a'


def self_named(x: 'a'):
    pass


assert get_type_hints(self_named) == {'x': ForwardRef('a', owner=self_named)}
assert get_type_hints(self_named, format=Format.FORWARDREF) == {'x': ForwardRef('a', owner=self_named)}
assert get_type_hints(self_named, format=Format.STRING) == {'x': 'a'}


# === The format argument ===
def ok(x: int, y: 'Foo') -> None:
    pass


value_hints = {'x': int, 'y': Foo, 'return': NoneType}
assert get_type_hints(ok, format=None) == value_hints
assert get_type_hints(ok, format=Format.VALUE) == value_hints
assert get_type_hints(ok, format=1) == value_hints
assert get_type_hints(ok, format=True) == value_hints
assert get_type_hints(ok, format=1.0) == value_hints
assert get_type_hints(ok, format=Format.FORWARDREF) == value_hints
assert get_type_hints(ok, format=Format.STRING) == {'x': 'int', 'y': 'Foo', 'return': 'None'}
assert get_type_hints(ok, format=4.0) == get_type_hints(ok, format=Format.STRING)
for fmt, message in [
    (2, 'The VALUE_WITH_FAKE_GLOBALS format is for internal use only'),
    (5, 'Unsupported format 5'),
    ('x', "Unsupported format 'x'"),
    (4.5, 'Unsupported format 4.5'),
]:
    for obj in [ok, C, 5]:
        try:
            get_type_hints(obj, format=fmt)
            assert False, 'expected ValueError'
        except ValueError as exc:
            assert str(exc) == message


class NoTypeCheck:
    __no_type_check__ = True
    x: 'Missing'


# `__no_type_check__` is read before the format
assert get_type_hints(NoTypeCheck, format=7) == {}
