# /// monty
# cpython-site-packages = true
# ///
from dataclasses import dataclass
from typing import Any, Literal, Optional

from pydantic import PydanticSchemaGenerationError, PydanticUserError, TypeAdapter, ValidationError


@dataclass
class Foo:
    x: int


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

# === The adapter holds its type alive and is reusable ===
adapters = [TypeAdapter(list[Foo]) for _ in range(3)]
for adapter in adapters:
    assert adapter.validate_python([{'x': '1'}]) == [Foo(1)]
