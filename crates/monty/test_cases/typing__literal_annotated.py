import typing
from typing import Annotated, Literal, Optional


class Foo:
    def method(self):
        return 'called'


# === Literal: building ===
assert repr(Literal['a', 'b']) == "typing.Literal['a', 'b']"
assert str(Literal['a']) == "typing.Literal['a']"
assert repr(Literal[1]) == 'typing.Literal[1]'
assert repr(Literal[()]) == 'typing.Literal[()]'
assert repr(Literal[None, int, ...]) == 'typing.Literal[None, int, ...]'
assert repr(Literal[b'x', 1.5, True]) == "typing.Literal[b'x', 1.5, True]"
assert repr(type(Literal['a'])) == "<class 'typing._LiteralGenericAlias'>"
assert type(Literal[1]) is type(Literal['a', 'b'])
assert repr(typing.Literal[1]) == 'typing.Literal[1]'

# a tuple key is the argument list, as for a generic alias
assert Literal[(1, 2)] == Literal[1, 2]

# === Literal: attributes ===
assert Literal[1].__origin__ is Literal
assert Literal[1, 'x'].__args__ == (1, 'x')
assert Literal[1].__parameters__ == ()
assert Literal[1].__name__ == 'Literal'
try:
    Literal[1].foo
    assert False, 'expected AttributeError'
except AttributeError:
    pass

# === Literal: deduplication and flattening ===
# duplicates drop by value and type, so 1, True and 1.0 all stay
assert Literal[1, 1, True, 1.0, 'a', 'a'].__args__ == (1, True, 1.0, 'a')
assert Literal[Literal[1, 2], 3, Literal[3]].__args__ == (1, 2, 3)
assert Literal[[1], [1]].__args__ == ([1],)

# === Literal: equality and hashing ===
assert Literal[1, 2] == Literal[2, 1]
assert Literal[1] != Literal[True]
assert Literal[1] != Literal[1.0]
assert Literal[1] != Literal[1, 2]
assert Literal[1] != 1
assert Literal[1] != Annotated[int, 1]
assert hash(Literal[1, 2]) == hash(Literal[2, 1])
assert {Literal[1], Literal[1]} == {Literal[1]}
assert {Literal['a']: 1}[Literal['a']] == 1
try:
    hash(Literal[[1]])
    assert False, 'expected TypeError'
except TypeError:
    pass
assert list[Literal[1]] == list[Literal[1]]
assert repr(list[Literal[1]]) == 'list[typing.Literal[1]]'

# === Literal: not a type ===
try:
    Literal[1]()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Cannot instantiate typing.Literal'

try:
    Literal[1][2]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'typing.Literal[1] is not a generic class'

try:
    isinstance(1, Literal[1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Subscripted generics cannot be used with class and instance checks'

try:
    isinstance(1, Literal[1] | None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Subscripted generics cannot be used with class and instance checks'

try:
    len(Literal[1])
    assert False, 'expected TypeError'
except TypeError:
    pass
assert bool(Literal[()]) is True

# === Literal: unions ===
assert repr(Literal[1] | None) == 'typing.Literal[1] | None'
assert repr(None | Literal[1]) == 'None | typing.Literal[1]'
assert repr(Optional[Literal[1]]) == 'typing.Literal[1] | None'
assert (Literal[1] | None) == Optional[Literal[1]]
assert (Literal[1] | Literal[2]).__args__ == (Literal[1], Literal[2])
assert (Literal[1] | Literal[1]) == Literal[1]
assert type(Literal[1] | str) is type(int | str)

# === Annotated: building ===
assert repr(Annotated[int, 'meta', 2]) == "typing.Annotated[int, 'meta', 2]"
assert repr(type(Annotated[int, 1])) == "<class 'typing._AnnotatedAlias'>"
assert type(Annotated[int, 1]) is not type(Literal[1])
assert repr(Annotated[Annotated[int, 1], 2]) == 'typing.Annotated[int, 1, 2]'
assert repr(Annotated[list[int], 1]) == 'typing.Annotated[list[int], 1]'
assert repr(Annotated[Literal[1], 3]) == 'typing.Annotated[typing.Literal[1], 3]'
assert repr(Annotated[typing.Any, 1]) == 'typing.Annotated[typing.Any, 1]'
assert repr(Annotated[Foo, 1]) == 'typing.Annotated[Foo, 1]'
assert repr(Annotated[int | None, 1]) == 'typing.Annotated[int | None, 1]'
# the type prints by name, the metadata by repr
assert repr(Annotated[int, str, None, ...]) == "typing.Annotated[int, <class 'str'>, None, Ellipsis]"
# the type is not validated, except that a tuple is refused and None becomes NoneType
assert repr(Annotated[1, 2]) == 'typing.Annotated[1, 2]'
assert repr(Annotated[..., 3]) == 'typing.Annotated[..., 3]'
assert repr(Annotated[None, 3]) == 'typing.Annotated[NoneType, 3]'
assert Annotated[None, 3].__origin__ is type(None)
try:
    Annotated[(1, 2), 3]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Annotated[t, ...]: t must be a type. Got (1, 2).'

for bad in [int, (int,), ()]:
    try:
        Annotated[bad]
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'Annotated[...] should be used with at least two arguments (a type and an annotation).'

# === Annotated: attributes ===
assert Annotated[int, 1].__origin__ is int
assert Annotated[int, 1].__args__ == (int,)
assert Annotated[int, 1, 2].__metadata__ == (1, 2)
assert Annotated[Annotated[int, 1], 2].__metadata__ == (1, 2)
assert Annotated[int, 1].__parameters__ == ()
assert Annotated[int, 1].__name__ == 'Annotated'
assert Annotated[list[int], 1].__origin__ == list[int]

# other names resolve on the annotated type
assert Annotated[dict, 1].fromkeys('ab') == {'a': None, 'b': None}
try:
    Annotated[int, 1].foo
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "type object 'int' has no attribute 'foo'"

# === Annotated: calls construct the annotated type ===
assert Annotated[int, 1]('5') == 5
assert Annotated[list, 1]('ab') == ['a', 'b']
assert Annotated[Foo, 'meta']().method() == 'called'

# === Annotated: equality and hashing ===
assert Annotated[int, 1] == Annotated[int, 1]
assert Annotated[int, 1] != Annotated[int, 2]
assert Annotated[int, 1] != Annotated[str, 1]
assert Annotated[int, 1, 2] != Annotated[int, 1]
assert Annotated[int, 1] != int
assert Annotated[int, 1] == Annotated[int, True]
assert hash(Annotated[int, 1]) == hash(Annotated[int, 1])
assert {Annotated[int, 1]: 'x'}[Annotated[int, 1]] == 'x'
try:
    hash(Annotated[int, [1]])
    assert False, 'expected TypeError'
except TypeError:
    pass
assert repr(list[Annotated[int, 1]]) == 'list[typing.Annotated[int, 1]]'

# === Annotated: not generic, not a class for isinstance ===
try:
    Annotated[int, 1][str]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'typing.Annotated[int, 1] is not a generic class'

try:
    isinstance(1, Annotated[int, 1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Subscripted generics cannot be used with class and instance checks'

try:
    len(Annotated[int, 1])
    assert False, 'expected TypeError'
except TypeError:
    pass

# === Annotated: unions ===
assert repr(Annotated[int, 1] | None) == 'typing.Annotated[int, 1] | None'
assert repr(None | Annotated[int, 1]) == 'None | typing.Annotated[int, 1]'
assert (Annotated[int, 1] | Annotated[int, 1]) == Annotated[int, 1]
assert (Annotated[int, 1] | Annotated[int, 2]).__args__ == (Annotated[int, 1], Annotated[int, 2])
assert (Annotated[int, 1] | int).__args__ == (Annotated[int, 1], int)
assert Optional[Annotated[int, 1]] == Annotated[int, 1] | None

# === Reprs of cycles through the arguments ===
cyclic = []
cyclic.append(Literal[cyclic])
assert repr(cyclic) == '[typing.Literal[[...]]]'
cyclic.clear()
