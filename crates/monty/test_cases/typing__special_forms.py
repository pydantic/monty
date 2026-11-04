import typing
from typing import (
    Annotated,
    Any,
    Callable,
    ClassVar,
    Dict,
    Final,
    FrozenSet,
    Generator,
    Iterable,
    Iterator,
    List,
    Literal,
    Mapping,
    Sequence,
    Set,
    Tuple,
    Type,
    get_type_hints,
)

# === Subscripting a typing form builds an alias with CPython's repr ===
assert repr(List[int]) == 'typing.List[int]'
assert repr(Dict[str, int]) == 'typing.Dict[str, int]'
assert repr(Tuple[int, ...]) == 'typing.Tuple[int, ...]'
assert repr(Set[int]) == 'typing.Set[int]'
assert repr(FrozenSet[str]) == 'typing.FrozenSet[str]'
assert repr(Type[int]) == 'typing.Type[int]'
assert repr(Callable[[int], str]) == 'typing.Callable[[int], str]'
assert repr(Callable[..., str]) == 'typing.Callable[..., str]'
assert repr(Sequence[int]) == 'typing.Sequence[int]'
assert repr(Mapping[str, int]) == 'typing.Mapping[str, int]'
assert repr(Iterable[int]) == 'typing.Iterable[int]'
assert repr(Iterator[int]) == 'typing.Iterator[int]'
assert repr(Generator[int, None, None]) == 'typing.Generator[int, NoneType, NoneType]'
assert repr(ClassVar[int]) == 'typing.ClassVar[int]'
assert repr(Final[int]) == 'typing.Final[int]'
assert repr(Literal['a', 1]) == "typing.Literal['a', 1]"
assert repr(Annotated[int, 'meta']) == "typing.Annotated[int, 'meta']"
assert repr(Dict[str, List[int]]) == 'typing.Dict[str, typing.List[int]]'
assert repr(typing.List[int]) == 'typing.List[int]'

# === __origin__ and __args__ ===
assert List[int].__origin__ is list
assert Dict[str, int].__origin__ is dict
assert Tuple[int].__origin__ is tuple
assert Set[int].__origin__ is set
assert FrozenSet[int].__origin__ is frozenset
assert Type[int].__origin__ is type
assert ClassVar[int].__origin__ is ClassVar
assert Final[int].__origin__ is Final
assert Literal[1].__origin__ is Literal
assert List[int].__args__ == (int,)
assert Dict[str, int].__args__ == (str, int)
assert Tuple[int, ...].__args__ == (int, ...)
assert Callable[[int, str], bool].__args__ == (int, str, bool)
assert Callable[..., str].__args__ == (..., str)
assert Callable[[], str].__args__ == (str,)
assert repr(Callable[[], str]) == 'typing.Callable[[], str]'
assert Callable[int, str] == Callable[[int], str]
assert repr(Callable[int, str]) == 'typing.Callable[[int], str]'
assert Callable[[int], str] != Callable[[str], str]
assert Literal['a', 1].__args__ == ('a', 1)
assert ClassVar[int].__args__ == (int,)
assert List[int].__parameters__ == ()

# === Annotated: the type is the origin, the rest is metadata ===
assert Annotated[int, 'meta'].__origin__ is int
assert Annotated[int, 'meta'].__args__ == (int,)
assert Annotated[int, 'meta', 3].__metadata__ == ('meta', 3)
assert Annotated[int, 'meta'](5) == 5

# === Equality and hashing follow origin and arguments ===
assert List[int] == List[int]
assert List[int] != List[str]
assert List[int] != Set[int]
assert List[int] != list[int]
assert {List[int]: 1, Dict[str, int]: 2}[List[int]] == 1
assert ClassVar[int] == ClassVar[int]
assert Annotated[int, 'a'] == Annotated[int, 'a']
assert Annotated[int, 'a'] != Annotated[int, 'b']


# === Forms refuse to be instantiated, with CPython's messages ===
def expect_type_error(fn, message):
    try:
        fn()
        assert False, f'expected TypeError: {message}'
    except TypeError as exc:
        assert str(exc) == message


expect_type_error(lambda: List[int](), 'Type List cannot be instantiated; use list() instead')
expect_type_error(lambda: Dict[str, int](), 'Type Dict cannot be instantiated; use dict() instead')
expect_type_error(lambda: FrozenSet[int](), 'Type FrozenSet cannot be instantiated; use frozenset() instead')
expect_type_error(lambda: Type[int](), 'Type Type cannot be instantiated; use type() instead')
expect_type_error(lambda: ClassVar[int](), 'Cannot instantiate typing.ClassVar')
expect_type_error(lambda: Final[int](), 'Cannot instantiate typing.Final')
expect_type_error(lambda: Literal[1](), 'Cannot instantiate typing.Literal')
expect_type_error(
    lambda: Callable[[int], str](),
    "Can't instantiate abstract class Callable without an implementation for abstract method '__call__'",
)
expect_type_error(
    lambda: Iterable[int](),
    "Can't instantiate abstract class Iterable without an implementation for abstract method '__iter__'",
)
expect_type_error(
    lambda: Sequence[int](),
    "Can't instantiate abstract class Sequence without an implementation for abstract methods '__getitem__', '__len__'",
)
expect_type_error(
    lambda: Generator[int, None, None](),
    "Can't instantiate abstract class Generator without an implementation for abstract methods 'send', 'throw'",
)

# === Argument checks CPython makes ===
expect_type_error(
    lambda: Annotated[int],
    'Annotated[...] should be used with at least two arguments (a type and an annotation).',
)
expect_type_error(
    lambda: ClassVar[int, str],
    "typing.ClassVar accepts only single type. Got (<class 'int'>, <class 'str'>).",
)
expect_type_error(lambda: Any[int], "type 'Any' is not subscriptable")
expect_type_error(lambda: Callable[int], 'Callable must be used as Callable[[arg, ...], result].')
expect_type_error(lambda: Callable[[int], str, bool], 'Callable must be used as Callable[[arg, ...], result].')


# === In annotations ===
class C:
    count: ClassVar[int] = 0
    names: List[str]
    extra: Annotated[int, 'units']


assert C.__annotations__['count'] == ClassVar[int]
assert C.__annotations__['names'] == List[str]
assert C.__annotations__['extra'] == Annotated[int, 'units']


class Foo:
    pass


def f(a: List['Foo'], b: Annotated['Foo', 'meta'], c: Dict[str, Annotated[int, 1]]) -> Callable[['Foo'], 'Foo']:
    pass


# `get_type_hints` keeps the typing alias when resolving a forward reference
# inside it, and strips `Annotated` unless asked to keep it.
hints = get_type_hints(f)
assert hints['a'] == List[Foo]
assert hints['b'] is Foo
assert hints['c'] == Dict[str, int]
assert hints['return'] == Callable[[Foo], Foo]
extras = get_type_hints(f, include_extras=True)
assert extras['b'] == Annotated[Foo, 'meta']
assert extras['c'] == Dict[str, Annotated[int, 1]]


# === Literal: values are deduplicated by value and type, nested forms flattened ===
assert Literal[1, 1].__args__ == (1,)
assert Literal[1, 2, 1, 2].__args__ == (1, 2)
assert Literal[1, Literal[2, Literal[3]]].__args__ == (1, 2, 3)
assert Literal[1, True].__args__ == (1, True)
assert Literal[1, '1', 1.0, True].__args__ == (1, '1', 1.0, True)
assert repr(Literal[1, 1]) == 'typing.Literal[1]'
assert repr(Literal[1, Literal[2, Literal[3]]]) == 'typing.Literal[1, 2, 3]'

# === Literal: equality and hashing ignore order ===
assert Literal[1, 2] == Literal[2, 1]
assert Literal[1, 2] != Literal[1, 2, 3]
assert Literal[1] != Literal[True]
assert Literal[1] != Literal[1.0]
assert hash(Literal[1, 2]) == hash(Literal[2, 1])
assert {Literal[2, 1]: 1}[Literal[1, 2]] == 1
try:
    Literal[[1], 2] == Literal[2, [1]]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "cannot use 'tuple' as a set element (unhashable type: 'list')"

# === Annotated: nested forms flatten, innermost metadata first ===
nested = Annotated[Annotated[int, 'a'], 'b']
assert repr(nested) == "typing.Annotated[int, 'a', 'b']"
assert nested.__origin__ is int
assert nested.__metadata__ == ('a', 'b')
assert Annotated[Annotated[Annotated[int, 1], 2], 3].__metadata__ == (1, 2, 3)
assert nested == Annotated[int, 'a', 'b']


class Deep:
    a: Annotated[Annotated[int, 'x'], 'y']


assert get_type_hints(Deep, include_extras=True) == {'a': Annotated[int, 'x', 'y']}

# === A list argument that contains itself prints finitely ===
loop = []
loop.append(loop)
assert repr(list[loop]) == 'list[[[[...]]]]'

# === An unknown attribute on a typing alias is looked up on the origin; a dunder is not ===
try:
    List[int].missing
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "type object 'list' has no attribute 'missing'"
try:
    List[int].missing([1])
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "type object 'list' has no attribute 'missing'"
try:
    List[int].__missing__
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == '__missing__'

# === Literal: many values, deduplicated through their hashes, and an unhashable one ===
big = tuple(range(1000))
assert Literal[big] == Literal[big[::-1]]
assert hash(Literal[big]) == hash(Literal[big[::-1]])
assert len(Literal[big + big].__args__) == 1000
assert Literal[big].__args__[:3] == (0, 1, 2)
assert Literal[[1], 1, [1]].__args__ == ([1], 1)
