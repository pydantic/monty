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


def f(a: List['Foo'], b: Annotated['Foo', 'meta'], c: Dict[str, Annotated[int, 1]]) -> Callable[[int], str]:
    pass


# `get_type_hints` keeps the typing alias when resolving a forward reference
# inside it, and strips `Annotated` unless asked to keep it.
hints = get_type_hints(f)
assert hints['a'] == List[Foo]
assert hints['b'] is Foo
assert hints['c'] == Dict[str, int]
assert hints['return'] == Callable[[int], str]
extras = get_type_hints(f, include_extras=True)
assert extras['b'] == Annotated[Foo, 'meta']
assert extras['c'] == Dict[str, Annotated[int, 1]]
