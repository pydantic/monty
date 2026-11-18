from collections import deque, namedtuple
from dataclasses import dataclass, field
from typing import Any, Literal, Optional, Union

from pydantic import TypeAdapter, ValidationError


def errors(adapter, value, **kwargs):
    try:
        adapter.validate_python(value, **kwargs)
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


# === list ===
Ints = TypeAdapter(list[int])
assert Ints.validate_python([1, '2']) == [1, 2]
assert Ints.validate_python((3,)) == [3]
assert Ints.validate_python({4}) == [4]
assert Ints.validate_python(frozenset([5])) == [5]
assert Ints.validate_python(deque([6, '7'])) == [6, 7]
assert Ints.validate_python([]) == []
source = [1, 2]
validated = Ints.validate_python(source)
assert validated == source
assert validated is not source
assert errors(Ints, 'ab') == [{'type': 'list_type', 'loc': (), 'msg': 'Input should be a valid list', 'input': 'ab'}]
assert errors(Ints, b'ab')[0]['type'] == 'list_type'
assert errors(Ints, {'a': 1})[0]['type'] == 'list_type'
assert errors(Ints, 5)[0]['type'] == 'list_type'
assert errors(Ints, None)[0]['type'] == 'list_type'
assert errors(Ints, [1, 'x', None]) == [
    {
        'type': 'int_parsing',
        'loc': (1,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    },
    {'type': 'int_type', 'loc': (2,), 'msg': 'Input should be a valid integer', 'input': None},
]
assert errors(Ints, (1,), strict=True)[0]['type'] == 'list_type'
# a deque is accepted only lax, by any sequence validator
assert [(e['type'], e['loc']) for e in errors(Ints, deque([1]), strict=True)] == [('list_type', ())]
assert [(e['type'], e['loc']) for e in errors(TypeAdapter(tuple[int, ...]), deque([1]), strict=True)] == [
    ('tuple_type', ())
]
assert TypeAdapter(set[int]).validate_python(deque([1, 1])) == {1}
# so `Any` (a strict match) beats the lax list, and the deque passes through
queue = deque([1])
assert TypeAdapter(Union[list[int], Any]).validate_python(queue) is queue
assert Ints.validate_python([1], strict=True) == [1]
assert errors(Ints, ['1'], strict=True)[0]['loc'] == (0,)
assert TypeAdapter(list).validate_python((1, 'a')) == [1, 'a']
assert TypeAdapter(list[Any]).validate_python([None, [1]]) == [None, [1]]
assert TypeAdapter(list[list[int]]).validate_python([['1'], []]) == [[1], []]
assert errors(TypeAdapter(list[list[int]]), [[1], ['x']])[0]['loc'] == (1, 0)

# === tuple ===
Pair = TypeAdapter(tuple[int, str])
assert Pair.validate_python((1, 'a')) == (1, 'a')
assert Pair.validate_python([1, 'a']) == (1, 'a')
assert type(Pair.validate_python([1, 'a'])) is tuple
assert errors(Pair, (1,)) == [{'type': 'missing', 'loc': (1,), 'msg': 'Field required', 'input': (1,)}]
assert errors(Pair, (1, 'a', 2)) == [
    {
        'type': 'too_long',
        'loc': (),
        'msg': 'Tuple should have at most 2 items after validation, not 3',
        'input': (1, 'a', 2),
        'ctx': {'field_type': 'Tuple', 'max_length': 2, 'actual_length': 3},
    }
]
assert errors(Pair, ('x', 1)) == [
    {
        'type': 'int_parsing',
        'loc': (0,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    },
    {'type': 'string_type', 'loc': (1,), 'msg': 'Input should be a valid string', 'input': 1},
]
assert errors(Pair, 'ab') == [{'type': 'tuple_type', 'loc': (), 'msg': 'Input should be a valid tuple', 'input': 'ab'}]
assert errors(Pair, [1, 'a'], strict=True)[0]['type'] == 'tuple_type'
Variadic = TypeAdapter(tuple[int, ...])
assert Variadic.validate_python((1, '2')) == (1, 2)
assert Variadic.validate_python([]) == ()
assert errors(Variadic, ['x', 'y']) == [
    {
        'type': 'int_parsing',
        'loc': (0,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    },
    {
        'type': 'int_parsing',
        'loc': (1,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'y',
    },
]
assert TypeAdapter(tuple).validate_python([1, 'a']) == (1, 'a')
assert TypeAdapter(tuple[()]).validate_python([]) == ()
assert errors(TypeAdapter(tuple[()]), [1])[0]['type'] == 'too_long'

# === set and frozenset ===
IntSet = TypeAdapter(set[int])
assert IntSet.validate_python([1, '2', 1]) == {1, 2}
assert type(IntSet.validate_python((1,))) is set
assert IntSet.validate_python(frozenset([3])) == {3}
assert errors(IntSet, 'ab') == [{'type': 'set_type', 'loc': (), 'msg': 'Input should be a valid set', 'input': 'ab'}]
assert errors(IntSet, [[1]]) == [
    {'type': 'int_type', 'loc': (0,), 'msg': 'Input should be a valid integer', 'input': [1]}
]
assert errors(TypeAdapter(set[Any]), [[1]]) == [
    {'type': 'set_item_not_hashable', 'loc': (0,), 'msg': 'Set items should be hashable', 'input': [1]}
]
# unhashable items and invalid items are reported together, in item order
not_hashable = {'type': 'set_item_not_hashable', 'msg': 'Set items should be hashable'}
bad_int = {'type': 'int_parsing', 'msg': 'Input should be a valid integer, unable to parse string as an integer'}
assert errors(TypeAdapter(set[list[int]]), [[1], ['x']]) == [
    not_hashable | {'loc': (0,), 'input': [1]},
    bad_int | {'loc': (1, 0), 'input': 'x'},
]
assert errors(TypeAdapter(frozenset[list[int]]), [['x'], [1], [2]]) == [
    bad_int | {'loc': (0, 0), 'input': 'x'},
    not_hashable | {'loc': (1,), 'input': [1]},
    not_hashable | {'loc': (2,), 'input': [2]},
]


# only a `TypeError` from hashing means unhashable; any other error propagates
class HashFails:
    def __hash__(self):
        raise ValueError('boom')


try:
    TypeAdapter(set[Any]).validate_python([HashFails()])
    assert False, 'expected a ValueError'
except ValueError as exc:
    assert repr(exc) == "ValueError('boom')"
assert errors(IntSet, [1], strict=True)[0]['type'] == 'set_type'
assert IntSet.validate_python({1}, strict=True) == {1}
Frozen = TypeAdapter(frozenset[str])
assert Frozen.validate_python(('a', b'b')) == frozenset({'a', 'b'})
assert type(Frozen.validate_python(['a'])) is frozenset
assert errors(Frozen, 1) == [
    {'type': 'frozen_set_type', 'loc': (), 'msg': 'Input should be a valid frozenset', 'input': 1}
]
assert errors(Frozen, {'a'}, strict=True)[0]['type'] == 'frozen_set_type'

# === dict ===
StrInt = TypeAdapter(dict[str, int])
assert StrInt.validate_python({'a': '1'}) == {'a': 1}
assert StrInt.validate_python({}) == {}
assert errors(StrInt, [('a', 1)]) == [
    {'type': 'dict_type', 'loc': (), 'msg': 'Input should be a valid dictionary', 'input': [('a', 1)]}
]
assert errors(StrInt, {1: 'x', 'a': 'b'}) == [
    {'type': 'string_type', 'loc': (1, '[key]'), 'msg': 'Input should be a valid string', 'input': 1},
    {
        'type': 'int_parsing',
        'loc': (1,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    },
    {
        'type': 'int_parsing',
        'loc': ('a',),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'b',
    },
]
assert errors(StrInt, {(1, 2): 1})[0]['loc'] == ('(1, 2)', '[key]')
# a bool key is located as the int it is; an int beyond `i64` and a float by their text
assert errors(TypeAdapter(dict[int, int]), {True: 'x'}) == [bad_int | {'loc': (1,), 'input': 'x'}]
assert errors(StrInt, {False: 1})[0]['loc'] == (0, '[key]')
assert errors(TypeAdapter(dict[int, int]), {10**30: 'x'})[0]['loc'] == ('1000000000000000000000000000000',)
assert [e['loc'] for e in errors(TypeAdapter(dict[int, int]), {1.5: 'x'})] == [('1.5', '[key]'), ('1.5',)]
assert errors(StrInt, {'a.b': 'x'})[0]['loc'] == ('a.b',)
assert TypeAdapter(dict).validate_python({1: [2]}) == {1: [2]}
assert TypeAdapter(dict[int, list[str]]).validate_python({'1': ('a',)}) == {1: ['a']}
try:
    StrInt.validate_python({'a.b': 'x', 'c': 'y'})
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert str(exc).splitlines()[1] == '`a.b`'
    assert str(exc).splitlines()[4] == 'c'

# === unions ===
IntOrStr = TypeAdapter(int | str)
assert IntOrStr.validate_python(1) == 1
assert IntOrStr.validate_python('1') == '1'
assert IntOrStr.validate_python(True) == 1
# a lax match on the first member loses to an exact match on the second
assert TypeAdapter(str | int).validate_python(1) == 1
assert TypeAdapter(str | int).validate_python('1') == '1'
# with no exact match, the most exact member wins
assert IntOrStr.validate_python(1.0) == 1
assert IntOrStr.validate_python(b'x') == 'x'
assert TypeAdapter(int | float).validate_python('1.5') == 1.5
assert TypeAdapter(int | float).validate_python('2') == 2
assert TypeAdapter(list[int] | dict[str, int]).validate_python({'a': '1'}) == {'a': 1}
assert errors(IntOrStr, []) == [
    {'type': 'int_type', 'loc': ('int',), 'msg': 'Input should be a valid integer', 'input': []},
    {'type': 'string_type', 'loc': ('str',), 'msg': 'Input should be a valid string', 'input': []},
]
assert errors(IntOrStr, None)[1]['loc'] == ('str',)
assert errors(TypeAdapter(list[int] | dict[str, int]), {'a': 'x'}) == [
    {'type': 'list_type', 'loc': ('list[int]',), 'msg': 'Input should be a valid list', 'input': {'a': 'x'}},
    {
        'type': 'int_parsing',
        'loc': ('dict[str,int]', 'a'),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    },
]
try:
    IntOrStr.validate_python([])
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'union[int,str]'
    assert str(exc).startswith('2 validation errors for union[int,str]\nint\n')

# === Optional ===
MaybeInt = TypeAdapter(Optional[int])
assert MaybeInt.validate_python(None) is None
assert MaybeInt.validate_python('1') == 1
assert errors(MaybeInt, 'x') == [
    {
        'type': 'int_parsing',
        'loc': (),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    }
]
try:
    MaybeInt.validate_python('x')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'nullable[int]'
Multi = TypeAdapter(int | str | None)
assert Multi.validate_python(None) is None
assert Multi.validate_python('x') == 'x'
assert errors(Multi, []) == [
    {'type': 'int_type', 'loc': ('int',), 'msg': 'Input should be a valid integer', 'input': []},
    {'type': 'string_type', 'loc': ('str',), 'msg': 'Input should be a valid string', 'input': []},
]
try:
    Multi.validate_python([])
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'nullable[union[int,str]]'
assert TypeAdapter(list[int | None]).validate_python([1, None, '2']) == [1, None, 2]
assert TypeAdapter(list[int | None]).validate_python([1], strict=True) == [1]

# === Literal ===
Choice = TypeAdapter(Literal['a', 1, True, None])
assert Choice.validate_python('a') == 'a'
assert Choice.validate_python(1) == 1
assert Choice.validate_python(True) is True
assert Choice.validate_python(None) is None
# `1.0` equals the choices `1` and `True`; the last of them wins
assert Choice.validate_python(1.0) is True
assert errors(Choice, 'b') == [
    {
        'type': 'literal_error',
        'loc': (),
        'msg': "Input should be 'a', 1, True or None",
        'input': 'b',
        'ctx': {'expected': "'a', 1, True or None"},
    }
]
assert errors(Choice, 0)[0]['type'] == 'literal_error'
assert errors(Choice, False)[0]['type'] == 'literal_error'
assert errors(TypeAdapter(Literal['x']), 'y')[0]['msg'] == "Input should be 'x'"
try:
    Choice.validate_python('b')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == "literal['a',1,True,None]"
assert TypeAdapter(Literal[1]).validate_python(True) == 1
assert TypeAdapter(Literal[True]).validate_python(1) is True
assert TypeAdapter(Literal['a']).validate_python('a', strict=True) == 'a'
assert errors(TypeAdapter(Literal['a']), b'a')[0]['type'] == 'literal_error'
assert TypeAdapter(list[Literal['a', 'b']]).validate_python(['a', 'b']) == ['a', 'b']

# === Nesting ===
Nested = TypeAdapter(dict[str, list[Optional[list[int]]]])
assert Nested.validate_python({'a': [None, ['1']], 'b': []}) == {'a': [None, [1]], 'b': []}
assert errors(Nested, {'a': [['x']]}) == [
    {
        'type': 'int_parsing',
        'loc': ('a', 0, 0),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    }
]

# === A container holding itself ===
# `Any` passes the inner list through, and `list[int]` reports it as a bad item
loop = []
loop.append(loop)
assert TypeAdapter(list[Any]).validate_python(loop) == [loop]
assert errors(TypeAdapter(list[list[int]]), loop)[0]['type'] == 'int_type'
assert errors(TypeAdapter(list[list[int]]), loop)[0]['loc'] == (0, 0)
loop.clear()

# === namedtuple keyword keys ===
# a key that is not a string is `invalid_key`, on the key, in entry order
Pt = TypeAdapter(namedtuple('Pt', ['a', 'b']))
invalid_key = {'type': 'invalid_key', 'msg': 'Keys should be strings'}
assert errors(Pt, {1: 2, 'a': 3, 'b': 4}) == [invalid_key | {'loc': (1,), 'input': 1}]
assert errors(Pt, {None: 2, 'c': 3, 'a': 1, 'b': 2}) == [
    invalid_key | {'loc': ('None',), 'input': None},
    {'type': 'unexpected_keyword_argument', 'loc': ('c',), 'msg': 'Unexpected keyword argument', 'input': 3},
]
assert [e['type'] for e in errors(Pt, {1: 2, 'a': 3})] == ['missing_argument', 'invalid_key']

# === Dataclass fields validate in declaration order ===
# whatever order the input gives them in: nested `__post_init__` hooks and factories run field by field
order = []


def factory():
    order.append('factory')
    return 0


@dataclass
class First:
    x: int

    def __post_init__(self):
        order.append('first')


@dataclass
class Second:
    x: int

    def __post_init__(self):
        order.append('second')


@dataclass
class Ordered:
    a: First
    f: int = field(default_factory=factory)
    b: Optional[Second] = None
    c: int = 0


OrderedAdapter = TypeAdapter(Ordered)
expected = Ordered(First(1), 0, Second(1), 0)
order.clear()
assert OrderedAdapter.validate_python({'b': {'x': 1}, 'a': {'x': 1}}) == expected
assert order == ['first', 'factory', 'second']
order.clear()
assert OrderedAdapter.validate_json('{"b": {"x": 1}, "zz": [1, {"y": 2}], "a": {"x": 1}}') == expected
assert order == ['first', 'factory', 'second']
order.clear()
# errors come in field order too
for value in [
    {'c': 'x', 'b': {'x': 'y'}, 'a': {'x': 'z'}, 'zz': 1},
    '{"c": "x", "b": {"x": "y"}, "a": {"x": "z"}, "zz": 1}',
]:
    try:
        if isinstance(value, str):
            OrderedAdapter.validate_json(value)
        else:
            OrderedAdapter.validate_python(value)
        assert False, 'expected a ValidationError'
    except ValidationError as exc:
        assert [(e['loc'], e['input']) for e in exc.errors()] == [(('a', 'x'), 'z'), (('b', 'x'), 'y'), (('c',), 'x')]
assert order == ['factory', 'factory']
order.clear()
# a field held for later is still read as JSON, malformed or not
try:
    OrderedAdapter.validate_json('{"b": {"x": 1,}, "a": {"x": 1}}')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.errors()[0]['type'] == 'json_invalid'
assert order == []
