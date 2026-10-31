# Monty's `pydantic` subset: `TypeAdapter` and `ValidationError`.
# See docs/limitations/pydantic.md for what each accepts at runtime.
from typing import Any, Generic, Literal, TypeVar

T = TypeVar('T')

VERSION: str

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
    def __init__(self, type: Any, *, config: None = None, _parent_depth: int = 2, module: None = None) -> None: ...
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
