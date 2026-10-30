import annotationlib
import typing
from annotationlib import Format, ForwardRef, get_annotations
from typing import Optional

NoneType = type(None)


class Foo:
    pass


def owner():
    pass


# === Format members compare as their int values ===
assert Format.VALUE == 1
assert Format.VALUE_WITH_FAKE_GLOBALS == 2
assert Format.FORWARDREF == 3
assert Format.STRING == 4
assert typing.ForwardRef is ForwardRef
assert annotationlib.ForwardRef is ForwardRef

# === Construction and attributes ===
fr = ForwardRef('Later')
assert fr.__forward_arg__ == 'Later'
assert fr.__resolved_str__ == 'Later'
assert fr.__forward_module__ is None
assert fr.__forward_is_argument__ is True
assert fr.__forward_is_class__ is False
assert fr.__owner__ is None
assert repr(fr) == "ForwardRef('Later')"
assert str(fr) == "ForwardRef('Later')"
assert repr(ForwardRef('a', is_class=True)) == "ForwardRef('a', is_class=True)"
assert repr(ForwardRef('a', module='m')) == "ForwardRef('a', module='m')"
assert repr(ForwardRef('a', module='m', is_class=True)) == "ForwardRef('a', module='m', is_class=True)"
assert ForwardRef('a', owner=Foo).__owner__ is Foo
assert ForwardRef('a', is_argument=False).__forward_is_argument__ is False
assert ForwardRef('a', is_class=True).__forward_is_class__ is True
assert type(fr) is ForwardRef
assert isinstance(fr, ForwardRef)
assert not isinstance('Later', ForwardRef), 'a str is not a ForwardRef'

for bad in [1, ['x'], None]:
    try:
        ForwardRef(bad)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f'Forward reference must be a string -- got {bad!r}'

# === Equality and hashing ===
assert ForwardRef('a') == ForwardRef('a')
assert ForwardRef('a') != ForwardRef('b')
assert ForwardRef('a') != 'a'
assert ForwardRef('a', is_argument=False) == ForwardRef('a')
assert ForwardRef('a', is_class=True) != ForwardRef('a')
assert ForwardRef('a', module='m') != ForwardRef('a')
assert ForwardRef('a', module='m') == ForwardRef('a', module='m')
assert ForwardRef('a', owner=Foo) != ForwardRef('a')
assert ForwardRef('a', owner=Foo) == ForwardRef('a', owner=Foo)
assert hash(ForwardRef('a')) == hash(ForwardRef('a'))
assert hash(ForwardRef('a', owner=Foo)) == hash(ForwardRef('a', owner=Foo))
assert {ForwardRef('a'): 1}[ForwardRef('a')] == 1
assert ForwardRef('a') in [ForwardRef('a')]
assert ForwardRef('a') in {ForwardRef('a')}

# === Unions ===
assert ForwardRef('a') | None == Optional[ForwardRef('a')]
assert (ForwardRef('a') | int).__args__ == (ForwardRef('a'), int)
assert (int | ForwardRef('a')).__args__ == (int, ForwardRef('a'))
assert repr(ForwardRef('a') | None) == "ForwardRef('a') | None"
assert repr(None | ForwardRef('a')) == "None | ForwardRef('a')"
assert repr(ForwardRef('a') | 1) == "ForwardRef('a') | 1"
assert repr(ForwardRef('a') | ForwardRef('b')) == "ForwardRef('a') | ForwardRef('b')"
assert ForwardRef('a') | ForwardRef('a') == ForwardRef('a')

# === Not subscriptable or callable, no other attributes ===
try:
    ForwardRef('a')[int]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'ForwardRef' object is not subscriptable"
try:
    ForwardRef('a')()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'ForwardRef' object is not callable"
try:
    ForwardRef('a').missing
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'ForwardRef' object has no attribute 'missing'"

# === evaluate() ===
assert ForwardRef('Later').evaluate(globals={'Later': int}) is int
assert ForwardRef('Later').evaluate(locals={'Later': str}) is str
assert ForwardRef('Later').evaluate(globals={'Later': int}, locals={'Later': str}) is str
assert ForwardRef('list[Later]').evaluate(globals={'Later': int}) == list[int]
assert ForwardRef('Later | None').evaluate(globals={'Later': Foo}) == Foo | None
assert ForwardRef('int').evaluate() is int
assert ForwardRef('None').evaluate() is None
assert ForwardRef('1 + 1').evaluate() == 2
assert ForwardRef('Later').evaluate(format=Format.STRING) == 'Later'
assert ForwardRef('Later').evaluate(format=Format.VALUE, globals={'Later': bytes}) is bytes

# without an owner the reference evaluates against empty globals
try:
    ForwardRef('Later').evaluate()
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Later' is not defined"

Later = float
try:
    ForwardRef('Later').evaluate()
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Later' is not defined"
# a function owner supplies its globals
assert ForwardRef('Later', owner=owner).evaluate() is float
assert ForwardRef('Later').evaluate(owner=owner) is float
assert ForwardRef('list[Later]', owner=owner).evaluate() == list[float]
assert ForwardRef('Later', owner=owner).evaluate(globals={'Later': int}) is int

# FORWARDREF keeps a bare unresolved name as the reference itself
unresolved = ForwardRef('Missing')
assert unresolved.evaluate(format=Format.FORWARDREF) is unresolved
assert unresolved.evaluate(format=Format.FORWARDREF, owner=owner) is unresolved
assert ForwardRef('Missing').evaluate(format=Format.FORWARDREF, globals={'Missing': int}) is int
# a name inside a larger expression becomes a new reference, remembering the
# namespace it was evaluated against, so it equals no constructed reference
nested = ForwardRef('list[Missing]').evaluate(format=Format.FORWARDREF)
assert nested.__origin__ is list
assert nested.__args__[0].__forward_arg__ == 'Missing'
assert nested.__args__[0].__owner__ is None
assert nested.__args__[0] != ForwardRef('Missing')
assert ForwardRef('list[Missing]').evaluate(format=Format.FORWARDREF, globals={'Missing': int}) == list[int]
assert ForwardRef('Missing | None').evaluate(format=Format.FORWARDREF).__forward_arg__ == 'Missing | None'
composed = ForwardRef('Missing.attr', owner=owner).evaluate(format=Format.FORWARDREF)
assert composed.__forward_arg__ == 'Missing.attr'
assert composed.__owner__ is owner
assert composed.__forward_is_class__ is False
# evaluated against the same owner's globals, two such references are equal
assert composed == ForwardRef('Missing.attr', owner=owner).evaluate(format=Format.FORWARDREF)
assert composed != ForwardRef('Missing.attr', owner=owner)
assert (
    ForwardRef('Missing.attr', is_class=True, owner=owner).evaluate(format=Format.FORWARDREF).__forward_is_class__
    is True
)
# an evaluation that raises for another reason is also kept
broken = ForwardRef('Foo.missing')
assert broken.evaluate(format=Format.FORWARDREF, globals={'Foo': Foo}) is broken
assert ForwardRef('1 / 0').evaluate(format=Format.FORWARDREF) is not None
try:
    ForwardRef('1 / 0').evaluate()
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'

for fmt in [Format.VALUE, Format.FORWARDREF]:
    try:
        ForwardRef('x =').evaluate(format=fmt)
        assert False, 'expected SyntaxError'
    except SyntaxError as exc:
        assert str(exc) == "Forward reference must be an expression -- got 'x ='"
for fmt in [Format.VALUE_WITH_FAKE_GLOBALS, 7]:
    try:
        ForwardRef('Later').evaluate(format=fmt)
        assert False, 'expected NotImplementedError'
    except NotImplementedError as exc:
        assert str(exc) == str(fmt)


# === get_annotations() ===
def f(x: int, y: 'Foo', z: None, w: list['Later2']) -> 'Later2':
    pass


assert get_annotations(f) == {'x': int, 'y': 'Foo', 'z': None, 'w': list['Later2'], 'return': 'Later2'}
assert get_annotations(f, format=Format.STRING) == {
    'x': 'int',
    'y': 'Foo',
    'z': 'None',
    'w': "list['Later2']",
    'return': 'Later2',
}
assert get_annotations(f, format=Format.FORWARDREF) == get_annotations(f)
assert get_annotations(f, format=1) == get_annotations(f)
assert get_annotations(f, eval_str=True, globals={'Foo': Foo, 'Later2': int}) == {
    'x': int,
    'y': Foo,
    'z': None,
    'w': list['Later2'],
    'return': int,
}
assert get_annotations(f, eval_str=True, locals={'Later2': bytes})['return'] is bytes
try:
    get_annotations(f, eval_str=True)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Later2' is not defined"
try:
    get_annotations(f, eval_str=True, format=Format.STRING)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'eval_str=True is only supported with format=Format.VALUE'
for fmt, message in [
    (None, 'Unsupported format None'),
    (2, 'The VALUE_WITH_FAKE_GLOBALS format is for internal use only'),
    (9, 'Unsupported format 9'),
    ('x', "Unsupported format 'x'"),
]:
    try:
        get_annotations(f, format=fmt)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == message

assert get_annotations(f) is not get_annotations(f)


class C:
    x: int
    y: 'Later2'
    z: None


assert get_annotations(C) == {'x': int, 'y': 'Later2', 'z': None}
assert get_annotations(C, format=Format.STRING) == {'x': 'int', 'y': 'Later2', 'z': 'None'}
assert get_annotations(C, eval_str=True, locals={'Later2': str}) == {'x': int, 'y': str, 'z': None}
assert get_annotations(C, eval_str=True, globals={'Later2': bytes}) == {'x': int, 'y': bytes, 'z': None}


def unannotated(a, b=1):
    pass


assert get_annotations(unannotated) == {}
assert get_annotations(len) == {}
assert get_annotations(int) == {}
assert get_annotations(Foo) == {}
for bad in [5, 'abc', None, [1]]:
    try:
        get_annotations(bad)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f'{bad!r} does not have annotations'


# an unresolved name is a ForwardRef owned by the function in FORWARDREF format
def partial(a: Missing2, b: list[Missing2], c: 'Missing2'):
    pass


try:
    get_annotations(partial)
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Missing2' is not defined"
hints = get_annotations(partial, format=Format.FORWARDREF)
assert list(hints) == ['a', 'b', 'c']
assert hints['a'].__forward_arg__ == 'Missing2'
assert hints['a'].__owner__ is partial
assert hints['b'] == list[hints['a']]
assert hints['c'] == 'Missing2'
