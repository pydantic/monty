import collections
import typing

# === Building unions with | ===
Maybe = int | None
assert repr(Maybe) == 'int | None'
assert str(Maybe) == 'int | None'
assert f'{Maybe}' == 'int | None'
assert repr(type(Maybe)) == "<class 'typing.Union'>"
assert type(Maybe).__name__ == 'Union'
assert type(Maybe) is typing.Union
assert typing.Union.__name__ == 'Union'
assert repr(int | str | None) == 'int | str | None'
assert repr(None | int) == 'None | int'
assert repr(int | type(None)) == 'int | None'
assert repr(int | ValueError) == 'int | ValueError'
assert repr(ValueError | TypeError) == 'ValueError | TypeError'
assert repr(collections.deque | int) == 'collections.deque | int'
assert repr(type | int) == 'type | int'
assert repr(int | typing.Any) == 'int | typing.Any'
assert repr(None | typing.Any) == 'None | typing.Any'
assert repr(int | typing.List) == 'int | typing.List'

# === Unions and generic aliases nest ===
assert repr(list[int] | None) == 'list[int] | None'
assert repr(int | list[str]) == 'int | list[str]'
assert repr(dict[str, int] | list[int] | None) == 'dict[str, int] | list[int] | None'
assert repr(type[int] | int) == 'type[int] | int'
assert repr(int | tuple[()]) == 'int | tuple[()]'
assert repr(list[int | None]) == 'list[int | None]'
assert repr(tuple[int | str, ...]) == 'tuple[int | str, ...]'
assert dict[str, int | None].__args__ == (str, int | None)

# === Flattening and deduplication ===
assert repr((int | str) | None) == 'int | str | None'
assert repr(int | (str | None)) == 'int | str | None'
assert repr((int | str) | (str | bytes)) == 'int | str | bytes'
assert repr(int | str | int) == 'int | str'
assert repr(int | None | None) == 'int | None'
assert int | int is int
assert type(None) | None is type(None)
assert (int | str | None).__args__ == (int, str, type(None))

# === __args__, __origin__, __parameters__ ===
assert Maybe.__args__ == (int, type(None))
assert Maybe.__args__[0] is int
assert Maybe.__origin__ is typing.Union
assert Maybe.__parameters__ == ()
try:
    (int | str).foo
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'typing.Union' object has no attribute 'foo'"

# === Equality and hashing are order-insensitive ===
assert int | str == str | int
assert int | str == int | str
assert not (int | None != int | None)
assert int | str != int | bytes
assert int | str != int | str | bytes
assert int | str != int
assert (int | str) != (int, str)
assert int | None is not int | None
assert hash(int | str) == hash(str | int)
assert {int | str: 1}[str | int] == 1
assert len({int | str, str | int, int | None}) == 2
try:
    hash(int | list[[1]])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc).startswith('unhashable type: ')

# === isinstance ===
assert isinstance(1, int | str)
assert isinstance('a', int | str)
assert isinstance(None, int | None)
assert not isinstance(1.0, int | str)
assert not isinstance(None, int | str)
assert isinstance(True, int | None)
assert isinstance(ValueError(), ValueError | TypeError)
assert not isinstance(KeyError(), ValueError | TypeError)
assert isinstance(1, (str, int | None))
assert isinstance(None, (str, int | None))
assert not isinstance(1.0, (str, int | None))
assert isinstance(1, int | list[int])
try:
    isinstance([], int | list[int])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'isinstance() argument 2 cannot be a parameterized generic'


class Foo:
    pass


assert isinstance(Foo(), Foo | None)
assert isinstance(None, Foo | None)
assert not isinstance(1, Foo | None)
assert (Foo | None).__args__ == (Foo, type(None))

# === A union's own | accepts anything, as in CPython 3.14 ===
assert repr((int | str) | 1) == 'int | str | 1'
assert repr(1 | (int | str)) == '1 | int | str'
assert repr(None | (int | str)) == 'None | int | str'

# === Operands that are not types ===
for build, message in [
    (lambda: int | 1, "unsupported operand type(s) for |: 'type' and 'int'"),
    (lambda: int | 'x', "unsupported operand type(s) for |: 'type' and 'str'"),
    (lambda: 1 | int, "unsupported operand type(s) for |: 'int' and 'type'"),
    (lambda: int | Foo(), "unsupported operand type(s) for |: 'type' and 'Foo'"),
    (lambda: None | None, "unsupported operand type(s) for |: 'NoneType' and 'NoneType'"),
    (lambda: int | ..., "unsupported operand type(s) for |: 'type' and 'ellipsis'"),
    (lambda: list[int] | 1, "unsupported operand type(s) for |: 'types.GenericAlias' and 'int'"),
]:
    try:
        build()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message

# === Unions cannot be called, subscripted, ordered or measured ===
try:
    (int | str)()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'typing.Union' object is not callable"
try:
    (int | str)[bytes]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'int | str is not a generic class'
try:
    (int | str) < (int | str)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'typing.Union' and 'typing.Union'"
try:
    len(int | str)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'typing.Union' has no len()"
try:
    iter(int | str)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'typing.Union' object is not iterable"
try:
    typing.Union()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "cannot create 'typing.Union' instances"
try:
    (int | str).x = 1
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'typing.Union' object has no attribute 'x' and no __dict__ for setting new attributes"
assert bool(int | None) is True

# === typing.Union[...] and typing.Optional[...] ===
assert repr(typing.Union[int, str]) == 'int | str'
assert repr(typing.Union[int, None]) == 'int | None'
assert typing.Union[int, str] == int | str
assert typing.Union[str, int] == typing.Union[int, str]
assert typing.Union[int] is int
assert typing.Union[int, int] is int
assert typing.Union[None, None] is type(None)
assert repr(typing.Union[int, 1]) == 'int | 1'
assert repr(typing.Union[list[int], None]) == 'list[int] | None'
assert repr(typing.Union[typing.Any, int]) == 'typing.Any | int'
assert repr(typing.Union[Foo, None]).endswith('Foo | None')
assert typing.Union[int, str, None].__args__ == (int, str, type(None))
# a namedtuple key is one member, not a tuple of members
pair = collections.namedtuple('Pair', 'first second')(int, str)
assert typing.Union[pair] is pair
assert typing.Union[pair, None].__args__ == (pair, type(None))
assert isinstance(1, typing.Union[int, str])
assert repr(typing.Optional[int]) == 'int | None'
assert typing.Optional[int] == int | None
assert repr(typing.Optional[1]) == '1 | None'
assert typing.Optional[None] is type(None)
assert repr(typing.Optional[list[int]]) == 'list[int] | None'
assert repr(typing.Optional[int] | str) == 'int | None | str'
try:
    typing.Union[()]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Cannot take a Union of no types.'
try:
    typing.Optional[int, str]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "typing.Optional requires a single type. Got (<class 'int'>, <class 'str'>)."
try:
    typing.Union[int, str][bytes]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'int | str is not a generic class'

# === | on other types is unchanged ===
assert {1} | {2} == {1, 2}
assert 3 | 4 == 7
assert (True | False) is True


# === Unions as runtime values and in annotations ===
def first(values: list[int] | None) -> int | None:
    return None if values is None else values[0]


assert first([1, 2]) == 1
assert first(None) is None
