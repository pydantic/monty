import copy
from dataclasses import dataclass
from typing import Annotated, Any, Dict, FrozenSet, List, Literal, Mapping, Optional, Set, Tuple

from pydantic import PydanticSchemaGenerationError, PydanticUserError, TypeAdapter, ValidationError


@dataclass
class Foo:
    x: int


def make_list():
    return []


# === repr ===
assert repr(TypeAdapter(int)) == 'TypeAdapter(int)'
assert repr(TypeAdapter(list[int])) == 'TypeAdapter(list[int])'
assert repr(TypeAdapter(Any)) == 'TypeAdapter(Any)'
assert repr(TypeAdapter(int | None)) == 'TypeAdapter(Union[int, NoneType])'
assert repr(TypeAdapter(Optional[str])) == 'TypeAdapter(Union[str, NoneType])'
assert repr(TypeAdapter(Foo)) == 'TypeAdapter(Foo)'
assert repr(TypeAdapter(list[Foo])) == 'TypeAdapter(list[Foo])'
assert repr(TypeAdapter(Literal['a', 1])) == "TypeAdapter(Literal['a', 1])"
assert repr(TypeAdapter(dict)) == 'TypeAdapter(dict)'
assert repr(TypeAdapter(tuple[int, ...])) == 'TypeAdapter(tuple[int, ...])'
assert repr(TypeAdapter(dict[str, list[int]])) == 'TypeAdapter(dict[str, list[int]])'
assert repr(TypeAdapter(tuple[()])) == 'TypeAdapter(tuple[])'
assert repr(TypeAdapter(List[int])) == 'TypeAdapter(List[int])'
assert repr(TypeAdapter(Tuple[()])) == 'TypeAdapter(Tuple[])'
assert repr(TypeAdapter(Dict)) == 'TypeAdapter(Dict)'
assert repr(TypeAdapter(Mapping[str, int])) == 'TypeAdapter(Mapping[str, int])'
assert repr(TypeAdapter(Optional[List[int]])) == 'TypeAdapter(Union[List[int], NoneType])'
# `Annotated` metadata that is not a type shows its class, a function its name
assert repr(TypeAdapter(Annotated[int, 'x'])) == 'TypeAdapter(Annotated[int, str])'
assert (
    repr(TypeAdapter(Annotated[int, list[int], 3, None, ...]))
    == 'TypeAdapter(Annotated[int, list[int], int, NoneType, ...])'
)
assert (
    repr(TypeAdapter(Annotated[int, Foo, Foo(1), len, make_list]))
    == 'TypeAdapter(Annotated[int, Foo, Foo, len, make_list])'
)
assert repr(TypeAdapter(Annotated[int, Literal[1, 'a'], int | str])) == (
    "TypeAdapter(Annotated[int, Literal[1, 'a'], Union[int, str]])"
)
# metadata nested past the recursion limit fails `repr()`, not the adapter
deep_hint = int
for _ in range(3000):
    deep_hint = list[deep_hint]
deep = TypeAdapter(Annotated[int, deep_hint])
assert deep.validate_python('2') == 2
try:
    repr(deep)
    assert False, 'expected RecursionError'
except RecursionError:
    # the message is CPython's stack-overflow one under the harness's thread stack
    pass

# === typing aliases validate as their builtins ===
assert TypeAdapter(List[int]).validate_python(('1', 2)) == [1, 2]
assert TypeAdapter(Dict[str, int]).validate_python({'a': '1'}) == {'a': 1}
assert TypeAdapter(Mapping[str, int]).validate_python({'a': '1'}) == {'a': 1}
assert TypeAdapter(Set[int]).validate_python(['1']) == {1}
assert TypeAdapter(FrozenSet[int]).validate_python(['1']) == frozenset({1})
assert TypeAdapter(Tuple[int, ...]).validate_python(['1', 2]) == (1, 2)
assert TypeAdapter(Tuple[int, str]).validate_python([1, 'a']) == (1, 'a')
assert TypeAdapter(List).validate_python((1, 'a')) == [1, 'a']
assert TypeAdapter(Dict).validate_python({1: 2}) == {1: 2}
assert TypeAdapter(Mapping).validate_python({1: 2}) == {1: 2}
assert TypeAdapter(Tuple).validate_python([1]) == (1,)
assert TypeAdapter(Set).validate_python([1]) == {1}
assert TypeAdapter(FrozenSet).validate_python([1]) == frozenset({1})
try:
    TypeAdapter(Tuple[int, ...]).validate_python(['x'])
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert str(exc) == (
        '1 validation error for tuple[int, ...]\n0\n  Input should be a valid integer, unable to parse string as an '
        "integer [type=int_parsing, input_value='x', input_type=str]\n"
        '    For further information visit https://errors.pydantic.dev/2.13/v/int_parsing'
    )
try:
    TypeAdapter(Mapping[str, int]).validate_python({'a': 'x'})
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'dict[str,int]'
try:
    TypeAdapter(Tuple[()]).validate_python([1])
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.title == 'tuple[]'
    assert exc.errors()[0]['type'] == 'too_long'

# === Types with no validator ===
SCHEMA_ERROR = (
    'Unable to generate pydantic-core schema for {}. Set `arbitrary_types_allowed=True` in the model_config to ignore '
    'this error or implement `__get_pydantic_core_schema__` on your type to fully support it.\n\n'
    'If you got this error by calling handler(<some type>) within `__get_pydantic_core_schema__` then you likely need '
    'to call `handler.generate_schema(<some type>)` since we do not call `__get_pydantic_core_schema__` on '
    '`<some type>` otherwise to avoid infinite recursion.\n\n'
    'For further information visit https://errors.pydantic.dev/2.13/u/schema-for-unknown-type'
)
for bad in [5, 1.5, (1, 2), ...]:
    try:
        TypeAdapter(bad)
        assert False, 'expected PydanticSchemaGenerationError'
    except PydanticSchemaGenerationError as exc:
        assert str(exc) == SCHEMA_ERROR.format(repr(bad))
        assert isinstance(exc, PydanticUserError)
        assert isinstance(exc, RuntimeError)
try:
    TypeAdapter(list[...])
    assert False, 'expected PydanticSchemaGenerationError'
except PydanticSchemaGenerationError as exc:
    assert str(exc) == SCHEMA_ERROR.format('Ellipsis')
try:
    TypeAdapter(tuple[..., int])
    assert False, 'expected PydanticSchemaGenerationError'
except PydanticSchemaGenerationError as exc:
    assert str(exc) == SCHEMA_ERROR.format('Ellipsis')

# === Generic arguments ===
assert TypeAdapter(list[int, str]).validate_python(['1']) == [1]
assert TypeAdapter(dict[str, int, int]).validate_python({'a': '1'}) == {'a': 1}
try:
    TypeAdapter(dict[str])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "Expected two type arguments for <class 'dict'>, got 1"
try:
    TypeAdapter(tuple[int, str, ...])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Variable tuples can only have one type'
for empty in [Literal[()], list[Literal[()]]]:
    try:
        TypeAdapter(empty)
        assert False, 'expected AssertionError'
    except AssertionError as exc:
        assert str(exc) == 'literal "expected" cannot be empty, obj=typing.Literal[()]'

# === Arguments ===
adapter = TypeAdapter(int)
assert adapter.validate_python('1') == 1
assert adapter.validate_python('1', strict=None) == 1
assert adapter.validate_python('1', strict=False) == 1
assert adapter.validate_python(1, strict=True) == 1
try:
    adapter.validate_python('1', strict=True)
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.errors()[0]['type'] == 'int_type'
try:
    adapter.validate_python()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "TypeAdapter.validate_python() missing 1 required positional argument: 'object'"
try:
    adapter.validate_python(1, bogus=2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "TypeAdapter.validate_python() got an unexpected keyword argument 'bogus'"
try:
    TypeAdapter()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "TypeAdapter.__init__() missing 1 required positional argument: 'type'"
try:
    TypeAdapter(int, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'TypeAdapter.__init__() takes 2 positional arguments but 3 were given'
assert TypeAdapter(type=int).validate_python('1') == 1

# `experimental_allow_partial` accepts the modes that validate whole inputs
assert adapter.validate_python('1', experimental_allow_partial=False) == 1
assert adapter.validate_python('1', experimental_allow_partial='off') == 1
assert adapter.validate_json('1', experimental_allow_partial='off') == 1
try:
    adapter.validate_python('1', experimental_allow_partial='bad')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == "Invalid partial mode, should be `'off'`, `'on'`, `'trailing-strings'` or a `bool`"
try:
    adapter.validate_json('1', experimental_allow_partial=None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == (
        "argument 'allow_partial': Invalid partial mode, should be `'off'`, `'on'`, `'trailing-strings'` or a `bool`"
    )

# dump options are extracted as pydantic-core's `usize`, `bool` and `WarningsArg`
assert TypeAdapter(list[int]).dump_json([1, 2], indent=True) == b'[\n 1,\n 2\n]'
assert TypeAdapter(list[int]).dump_json([1], indent=False) == b'[\n1\n]'
try:
    adapter.dump_json(1, indent=10**30)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'int too big to convert'
try:
    adapter.dump_json(1, indent=-(10**30))
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == "can't convert negative int to unsigned"
try:
    adapter.dump_json(1, exclude_none=None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'exclude_none': 'None' is not an instance of 'bool'"
for warnings in [True, False, 'none', 'warn', 'error']:
    assert adapter.dump_python(1, warnings=warnings) == 1
    assert adapter.dump_json(1, warnings=warnings) == b'1'
WARNINGS_ERROR = (
    "argument 'warnings': failed to extract enum WarningsArg ('Bool | Literal')\n"
    '- variant Bool (Bool): TypeError: failed to extract field WarningsArg::Bool.0, caused by TypeError: {} is not '
    "an instance of 'bool'\n"
    '- variant Literal (Literal): TypeError: failed to extract field WarningsArg::Literal.0, caused by {}: Invalid '
    "warnings parameter, should be `'none'`, `'warn'`, `'error'` or a `bool`"
)
for warnings, subject, cause in [
    (3, "'int' object", 'TypeError'),
    (None, "'None'", 'TypeError'),
    ('bad', "'str' object", 'ValueError'),
]:
    try:
        adapter.dump_json(1, warnings=warnings)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == WARNINGS_ERROR.format(subject, cause)

# === The adapter object ===
assert type(adapter).__name__ == 'TypeAdapter'
assert type(adapter).__qualname__ == 'TypeAdapter'
assert type(adapter).__module__ == 'pydantic.type_adapter'
assert adapter.__module__ == 'pydantic.type_adapter'
assert repr(TypeAdapter) == "<class 'pydantic.type_adapter.TypeAdapter'>"
try:
    adapter.bogus
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'TypeAdapter' object has no attribute 'bogus'"
# adapters hash and compare by identity
other = TypeAdapter(int)
assert hash(adapter) == hash(adapter)
assert {adapter: 1, other: 2}[other] == 2
assert adapter == adapter
assert adapter != other
# a copy is a new adapter that validates the same way
for copied in [copy.copy(adapter), copy.deepcopy(adapter)]:
    assert copied is not adapter
    assert copied.validate_python('3') == 3
    assert repr(copied) == 'TypeAdapter(int)'
dataclass_copy = copy.deepcopy(TypeAdapter(list[Foo]))
assert dataclass_copy.validate_python([{'x': '1'}]) == [Foo(1)]

# === The adapter holds its type alive and is reusable ===
adapters = [TypeAdapter(list[Foo]) for _ in range(3)]
for adapter in adapters:
    assert adapter.validate_python([{'x': '1'}]) == [Foo(1)]


def make_adapter():
    @dataclass
    class Local:
        x: int

    return TypeAdapter(list[Local])


# the class is only reachable through the adapter once the function returns
local_adapter = make_adapter()
assert local_adapter.validate_python([{'x': '1'}])[0].x == 1
assert local_adapter.validate_python([{'x': '2'}])[0].x == 2
