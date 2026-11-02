# Monty's `pydantic` subset: `TypeAdapter`, `Field`, `ConfigDict`, `validate_call` and `ValidationError`.
# See docs/limitations/pydantic.md for what each accepts at runtime.
from collections.abc import Callable
from typing import Any, Generic, Literal, TypedDict, TypeVar, overload

T = TypeVar('T')

VERSION: str
__version__: str

class FieldInfo:
    @property
    def default(self) -> Any: ...
    @property
    def default_factory(self) -> Any: ...
    @property
    def alias(self) -> str | None: ...
    @property
    def validation_alias(self) -> str | None: ...
    @property
    def serialization_alias(self) -> str | None: ...
    @property
    def title(self) -> str | None: ...
    @property
    def description(self) -> str | None: ...
    @property
    def examples(self) -> list[Any] | None: ...
    @property
    def exclude(self) -> bool | None: ...
    @property
    def frozen(self) -> bool | None: ...
    @property
    def json_schema_extra(self) -> dict[str, Any] | None: ...
    def is_required(self) -> bool: ...

def Field(
    default: Any = ...,
    *,
    default_factory: Callable[[], Any] | None = None,
    alias: str | None = None,
    validation_alias: str | None = None,
    serialization_alias: str | None = None,
    title: str | None = None,
    description: str | None = None,
    examples: list[Any] | None = None,
    exclude: bool | None = None,
    json_schema_extra: dict[str, Any] | None = None,
    frozen: bool | None = None,
    pattern: str | None = None,
    strict: bool | None = None,
    coerce_numbers_to_str: bool | None = None,
    gt: Any = None,
    ge: Any = None,
    lt: Any = None,
    le: Any = None,
    multiple_of: Any = None,
    min_length: int | None = None,
    max_length: int | None = None,
    **extra: Any,
) -> Any: ...

class ConfigDict(TypedDict, total=False):
    title: str | None
    strict: bool
    extra: Literal['allow', 'ignore', 'forbid'] | None
    populate_by_name: bool
    str_strip_whitespace: bool
    str_to_lower: bool
    str_to_upper: bool
    str_min_length: int
    str_max_length: int | None
    coerce_numbers_to_str: bool

_F = TypeVar('_F', bound=Callable[..., Any])

@overload
def validate_call(func: _F, /) -> _F: ...
@overload
def validate_call(*, config: ConfigDict | None = None, validate_return: bool = False) -> Callable[[_F], _F]: ...

class PydanticUserError(RuntimeError): ...
class PydanticSchemaGenerationError(PydanticUserError): ...

class ValidationError(ValueError):
    @property
    def title(self) -> str: ...
    def error_count(self) -> int: ...
    def errors(
        self, *, include_url: bool = True, include_context: bool = True, include_input: bool = True
    ) -> list[dict[str, Any]]: ...
    def json(
        self,
        *,
        indent: int | None = None,
        include_url: bool = True,
        include_context: bool = True,
        include_input: bool = True,
    ) -> str: ...
    @classmethod
    def from_exception_data(
        cls,
        title: str,
        line_errors: list[dict[str, Any]],
        input_type: Literal['python', 'json'] = 'python',
        hide_input: bool = False,
    ) -> ValidationError: ...

class TypeAdapter(Generic[T]):
    @overload
    def __init__(
        self, type: type[T], *, config: ConfigDict | None = None, _parent_depth: int = 2, module: None = None
    ) -> None: ...
    # special forms (`list[int]`, `int | None`, `Literal[...]`) have no `type[T]`, as in pydantic's stub
    @overload
    def __init__(
        self, type: Any, *, config: ConfigDict | None = None, _parent_depth: int = 2, module: None = None
    ) -> None: ...
    def validate_python(self, object: Any, /, *, strict: bool | None = None) -> T: ...
    def validate_json(self, data: str | bytes, /, *, strict: bool | None = None) -> T: ...
    def dump_python(
        self,
        instance: T,
        /,
        *,
        mode: Literal['json', 'python'] = 'python',
        by_alias: bool | None = None,
        exclude_none: bool = False,
        round_trip: bool = False,
        warnings: bool | Literal['none', 'warn', 'error'] = True,
    ) -> Any: ...
    def dump_json(
        self,
        instance: T,
        /,
        *,
        indent: int | None = None,
        ensure_ascii: bool = False,
        by_alias: bool | None = None,
        exclude_none: bool = False,
        round_trip: bool = False,
        warnings: bool | Literal['none', 'warn', 'error'] = True,
    ) -> bytes: ...
    def json_schema(
        self,
        *,
        by_alias: bool | None = None,
        ref_template: str = '#/$defs/{model}',
        union_format: Literal['any_of', 'primitive_type_union'] = 'any_of',
        schema_generator: Any = None,
        mode: Literal['validation', 'serialization'] = 'validation',
    ) -> dict[str, Any]: ...
