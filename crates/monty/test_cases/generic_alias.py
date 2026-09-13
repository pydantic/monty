import collections
import functools
import re

# === Building aliases ===
Record = tuple[int, int, int, int, int]
assert repr(Record) == 'tuple[int, int, int, int, int]'
assert str(Record) == 'tuple[int, int, int, int, int]'
assert f'{Record}' == 'tuple[int, int, int, int, int]'
assert repr(type(Record)) == "<class 'types.GenericAlias'>"
assert type(Record).__name__ == 'GenericAlias'
assert type(list[int]) is type(dict[str, int])

assert repr(list[int]) == 'list[int]'
assert repr(dict[str, int]) == 'dict[str, int]'
assert repr(set[float]) == 'set[float]'
assert repr(frozenset[bytes]) == 'frozenset[bytes]'
assert repr(type[int]) == 'type[int]'
assert repr(tuple[int, ...]) == 'tuple[int, ...]'
assert repr(tuple[()]) == 'tuple[()]'
assert repr(list[list[int]]) == 'list[list[int]]'
assert repr(dict[str, list[tuple[int, str]]]) == 'dict[str, list[tuple[int, str]]]'
assert repr(list[type]) == 'list[type]'
assert repr(list[type[int]]) == 'list[type[int]]'
assert repr(list[ValueError]) == 'list[ValueError]'

# === Arguments are not validated ===
assert repr(list[None]) == 'list[None]'
assert repr(list[...]) == 'list[...]'
assert repr(list[3.5]) == 'list[3.5]'
assert repr(list[1, 'a', None]) == "list[1, 'a', None]"
assert repr(dict[str, 'Foo']) == "dict[str, 'Foo']"
assert repr(dict[str]) == 'dict[str]'
assert repr(list[int, str]) == 'list[int, str]'

# === stdlib types ===
assert repr(collections.deque[int]) == 'collections.deque[int]'
assert repr(collections.defaultdict[str, int]) == 'collections.defaultdict[str, int]'
assert repr(functools.partial[int]) == 'functools.partial[int]'
assert repr(re.Pattern[str]) == 're.Pattern[str]'
assert repr(re.Match[bytes]) == 're.Match[bytes]'

# === __origin__, __args__, __parameters__ ===
assert Record.__origin__ is tuple
assert Record.__args__ == (int, int, int, int, int)
assert Record.__parameters__ == ()
assert dict[str, int].__origin__ is dict
assert dict[str, int].__args__ == (str, int)
assert list[int].__args__ == (int,)
assert tuple[()].__args__ == ()
assert tuple[int, ...].__args__ == (int, ...)
assert type[int].__origin__ is type
assert list[int].__origin__ is list
alias = list[int]
assert alias.__args__ is alias.__args__

# the tuple written in the subscript is `__args__` itself
args = (int, str)
assert dict[args].__args__ is args

# === Other attributes come from the origin ===
assert dict[str, int].fromkeys('ab') == {'a': None, 'b': None}
assert list[int].__origin__() == []
assert Record.__origin__((1, 2)) == (1, 2)
try:
    list[int].__args__()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'tuple' object is not callable"
assert list[int].__class_getitem__(str) == list[str]
assert repr(list[int].__class_getitem__(str)) == 'list[str]'
try:
    list[int].nope()
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "type object 'list' has no attribute 'nope'"
assert list[int].__name__ == 'list'
assert type[int].__name__ == 'type'
assert collections.deque[int].__name__ == 'deque'
try:
    list[int].__name__()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object is not callable"
try:
    list.__name__()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object is not callable"
try:
    list[int].foo
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "type object 'list' has no attribute 'foo'"

# === Equality and hashing ===
assert list[int] == list[int]
assert not (list[int] != list[int])
assert list[int] != list[str]
assert list[int] != list
assert list != list[int]
assert set[int] != frozenset[int]
assert tuple[int, str] != tuple[str, int]
assert tuple[int, ...] == tuple[int, ...]
assert list[int] is not list[int]
assert hash(list[int]) == hash(list[int])
assert {list[int]: 1}[list[int]] == 1
assert list[int] in {list[int], dict[str, int]}
assert len({list[int], list[int], list[str]}) == 2
try:
    hash(list[[1]])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc).startswith('unhashable type: ')
# Two aliases whose arguments cycle back to them: the comparison recurses
# until the recursion limit, as for two self-referential lists.
cyclic_a = []
cyclic_x = list[cyclic_a]
cyclic_a.append(cyclic_x)
cyclic_b = []
cyclic_y = list[cyclic_b]
cyclic_b.append(cyclic_y)
try:
    cyclic_x == cyclic_y
    assert False, 'expected RecursionError'
except RecursionError:
    pass
assert cyclic_x == cyclic_x

# === Calling an alias calls the origin ===
assert list[int]() == []
assert list[int]([1, 2]) == [1, 2]
assert list[int](range(3)) == [0, 1, 2]
assert dict[str, int](a=1) == {'a': 1}
assert set[int]('ab') == {'a', 'b'}
assert tuple[int]((1, 2)) == (1, 2)
assert type(tuple[int]((1, 2))) is tuple
assert collections.deque[int]([1, 2], maxlen=1) == collections.deque([2])
assert type[int](3) is int
try:
    type[int]()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'type() takes 1 or 3 arguments'

# === __class_getitem__ ===
assert list.__class_getitem__(int) == list[int]
assert list.__class_getitem__((int, str)) == list[int, str]
assert repr(list.__class_getitem__((int, str))) == 'list[int, str]'
assert collections.deque.__class_getitem__(int) == collections.deque[int]
try:
    list.__class_getitem__()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'list.__class_getitem__() takes exactly one argument (0 given)'
try:
    collections.deque.__class_getitem__(1, 2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'deque.__class_getitem__() takes exactly one argument (2 given)'

# === An alias is truthy and has no length ===
assert bool(tuple[()]) is True
assert bool(list[int]) is True
try:
    len(list[int])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'types.GenericAlias' has no len()"

# === Aliases cannot be subscripted again or ordered ===
try:
    list[int][str]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'list[int] is not a generic class'
try:
    tuple[int, ...][0]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'tuple[int, ...] is not a generic class'
try:
    list[int] < list[str]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'types.GenericAlias' and 'types.GenericAlias'"

# === Not usable with isinstance ===
try:
    isinstance([], list[int])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'isinstance() argument 2 cannot be a parameterized generic'
try:
    isinstance([], (int, list[int]))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'isinstance() argument 2 cannot be a parameterized generic'

# === Attributes cannot be set ===
try:
    list[int].x = 1
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'types.GenericAlias' object has no attribute 'x' and no __dict__ for setting new attributes"

# === Types without __class_getitem__ ===
for ty, name in [
    (int, 'int'),
    (str, 'str'),
    (float, 'float'),
    (bool, 'bool'),
    (bytes, 'bytes'),
    (range, 'range'),
    (slice, 'slice'),
    (object, 'object'),
    (ValueError, 'ValueError'),
]:
    try:
        ty[int]
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f"type '{name}' is not subscriptable"


# === User classes are not subscriptable ===
class Foo:
    pass


try:
    Foo[int]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "type 'Foo' is not subscriptable"

# === Aliases in annotations and as runtime values ===
Pair = dict[str, list[int]]


def first(record: Record) -> int:
    return record[0]


table: Pair = {'a': [1]}
assert first((1, 2, 3, 4, 5)) == 1
assert table == {'a': [1]}
