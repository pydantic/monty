from typing import Annotated, Any

from pydantic import ConfigDict, Field, ValidationError, validate_call


def errors(fn):
    try:
        fn()
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


# === Arguments are validated and coerced before the call ===
@validate_call
def add(a: int, b: int = 2, *, c: str = 'x') -> str:
    return f'{a + b}{c}'


assert add('1', c='y') == '3y'
assert add(1, '3') == '4x'
assert add.__name__ == 'add'
assert type(add).__name__ == 'function'
assert repr(add).startswith('<function add at 0x')
assert add.raw_function('1', 'x') == '1xx', 'raw_function skips validation'
try:
    add('a')
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'add'

# positional arguments are located by index, keywords by name
assert errors(lambda: add('a')) == [
    {
        'type': 'int_parsing',
        'loc': (0,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'a',
    }
]
assert errors(lambda: add(b='a', a=1)) == [
    {
        'type': 'int_parsing',
        'loc': ('b',),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'a',
    }
]
assert errors(lambda: add(1, 2, 'c')) == [
    {'type': 'unexpected_positional_argument', 'loc': (2,), 'msg': 'Unexpected positional argument', 'input': 'c'}
]
assert errors(lambda: add(1, d=3)) == [
    {'type': 'unexpected_keyword_argument', 'loc': ('d',), 'msg': 'Unexpected keyword argument', 'input': 3}
]
assert errors(lambda: add(1, a=2)) == [
    {'type': 'multiple_argument_values', 'loc': ('a',), 'msg': 'Got multiple values for argument', 'input': 2}
]
assert errors(lambda: add(1, c=5)) == [
    {'type': 'string_type', 'loc': ('c',), 'msg': 'Input should be a valid string', 'input': 5}
]
missing = errors(lambda: add())
assert [(e['type'], e['loc'], e['msg']) for e in missing] == [('missing_argument', ('a',), 'Missing required argument')]


# === Positional-only, *args and **kwargs ===
@validate_call
def posonly(a: int, /, b: int):
    return a + b


assert posonly(1, b='2') == 3
assert posonly('1', 2) == 3
by_keyword = errors(lambda: posonly(a=1, b=2))
assert [(e['type'], e['loc'], e['msg']) for e in by_keyword] == [
    ('missing_positional_only_argument', (0,), 'Missing required positional only argument'),
    ('unexpected_keyword_argument', ('a',), 'Unexpected keyword argument'),
]


@validate_call
def var(a: int, *args: int, **kwargs: str):
    return a, args, kwargs


assert var(1, '2', 3, x='y') == (1, (2, 3), {'x': 'y'})
assert errors(lambda: var(1, 'x')) == [
    {
        'type': 'int_parsing',
        'loc': (1,),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
    }
]
assert errors(lambda: var(1, z=1)) == [
    {'type': 'string_type', 'loc': ('z',), 'msg': 'Input should be a valid string', 'input': 1}
]


@validate_call
def kw_only(*, n: int = 1, m: int):
    return n, m


assert kw_only(m='2') == (1, 2)
assert [(e['type'], e['loc'], e['msg']) for e in errors(lambda: kw_only())] == [
    ('missing_keyword_only_argument', ('m',), 'Missing required keyword only argument')
]


@validate_call
def untyped(a, b=1):
    return a, b


assert untyped('x', b='y') == ('x', 'y'), 'an unannotated parameter accepts anything'


# === validate_return, config and Field defaults ===
@validate_call(validate_return=True)
def ret(a: int) -> int:
    text: Any = str(a)
    return text


assert ret(1) == 1, 'the result is coerced'


@validate_call(validate_return=True)
def ret_str(a: int) -> str:
    number: Any = a
    return number


assert errors(lambda: ret_str(1)) == [
    {'type': 'string_type', 'loc': (), 'msg': 'Input should be a valid string', 'input': 1}
]


@validate_call(config=ConfigDict(strict=True))
def strict(a: int):
    return a


assert errors(lambda: strict('1')) == [
    {'type': 'int_type', 'loc': (0,), 'msg': 'Input should be a valid integer', 'input': '1'}
]
assert strict(1) == 1

checked = validate_call(config=ConfigDict(str_to_upper=True))


@checked
def shout(s: str):
    return s


assert shout('ab') == 'AB', 'a configured decorator can be kept and applied later'


@validate_call
def annotated(
    a: Annotated[int, Field(gt=0)], b: list[int] = Field(default_factory=list), c: int = Field(default=5, le=10)
):
    return a, b, c


assert annotated(1) == (1, [], 5)
assert annotated(1, ['2'], c='7') == (1, [2], 7)
assert errors(lambda: annotated(0)) == [
    {'type': 'greater_than', 'loc': (0,), 'msg': 'Input should be greater than 0', 'input': 0, 'ctx': {'gt': 0}}
]
assert errors(lambda: annotated(1, c=11)) == [
    {
        'type': 'less_than_equal',
        'loc': ('c',),
        'msg': 'Input should be less than or equal to 10',
        'input': 11,
        'ctx': {'le': 10},
    }
]


# === Other callables ===
assert validate_call(lambda x: x)(1) == 1


def outer():
    @validate_call
    def inner(a: int):
        return a

    return inner


assert outer()('2') == 2

try:
    validate_call(5)
    assert False, 'expected a PydanticUserError'
except Exception as exc:
    assert type(exc).__name__ == 'PydanticUserError'
    assert str(exc) == (
        'Unable to validate 5: `validate_call` should be applied to one of the following: function, method, partial, '
        'or lambda\n\nFor further information visit https://errors.pydantic.dev/2.13/u/validate-call-type'
    )
