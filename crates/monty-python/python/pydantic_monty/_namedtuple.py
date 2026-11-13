"""The one host class every namedtuple decoded from the sandbox is an instance of.

Building a class per sandbox type, as `collections.namedtuple` does, would let the sandbox spend host memory
outside its limits (~5.5 KB per type, and field names compiled into code are immortal on 3.12+).
`MontyNamedTuple` instead carries its type name and field names on each instance.
"""

import sys
from collections.abc import Iterable
from typing import Any

# `_replace` with an unknown field raised `ValueError` until 3.13
_REPLACE_ERROR = TypeError if sys.version_info >= (3, 13) else ValueError

_set_attribute = object.__setattr__


class MontyNamedTuple(tuple[Any, ...]):
    """A namedtuple returned by the sandbox: a tuple whose fields are also attributes.

    It supports the instance API of `collections.namedtuple` (`p.x`, `_fields`, `_asdict()`, `_replace()`,
    `_make()`), but every sandbox namedtuple type shares this one class, so `type(p)` is `MontyNamedTuple`
    and the sandbox's type name is `p._type_name`, such as `'Point'` or `'os.stat_result'`.
    """

    _type_name: str
    """The sandbox type name, qualified by its module unless it is a built-in or `__main__` type."""
    _fields: tuple[str, ...]
    """The field names, in order."""

    def __new__(cls, type_name: str, fields: tuple[str, ...], values: Iterable[Any]) -> 'MontyNamedTuple':
        self = super().__new__(cls, values)
        if len(self) != len(fields):
            raise TypeError(f'Expected {len(fields)} arguments, got {len(self)}')
        _set_attribute(self, '_type_name', type_name)
        _set_attribute(self, '_fields', fields)
        return self

    def __getattr__(self, name: str) -> Any:
        # only reached when normal lookup fails; an instance missing its own
        # attributes (never built by `__new__`) raises rather than recursing
        if name in ('_type_name', '_fields'):
            raise AttributeError(name)
        if name in self._fields:
            return self[self._fields.index(name)]
        raise AttributeError(f'{self._simple_name()!r} object has no attribute {name!r}')

    def __setattr__(self, name: str, value: Any) -> None:
        raise AttributeError(f'{self._simple_name()!r} object is immutable')

    def __delattr__(self, name: str) -> None:
        raise AttributeError(f'{self._simple_name()!r} object is immutable')

    def __repr__(self) -> str:
        fields = ', '.join(f'{name}={value!r}' for name, value in zip(self._fields, self))
        return f'{self._simple_name()}({fields})'

    def __reduce__(self) -> tuple[Any, ...]:
        return MontyNamedTuple, (self._type_name, self._fields, tuple(self))

    def __getnewargs__(self) -> tuple[Any, ...]:
        # as on `collections.namedtuple`; pickling itself goes through `__reduce__`
        return tuple(self)

    def __dir__(self) -> list[str]:
        return sorted({*super().__dir__(), *self._fields})

    @property
    def _field_defaults(self) -> dict[str, Any]:
        """Always empty: sandbox namedtuples cross without their defaults."""
        return {}

    def _make(self, iterable: Iterable[Any]) -> 'MontyNamedTuple':
        """A new namedtuple of the same type from a sequence or iterable."""
        return MontyNamedTuple(self._type_name, self._fields, iterable)

    def _replace(self, /, **kwds: Any) -> 'MontyNamedTuple':
        """A new namedtuple of the same type with the given fields replaced."""
        result = self._make(map(kwds.pop, self._fields, self))
        if kwds:
            raise _REPLACE_ERROR(f'Got unexpected field names: {list(kwds)!r}')
        return result

    def _asdict(self) -> dict[str, Any]:
        """A new dict mapping field names to values."""
        return dict(zip(self._fields, self))

    if sys.version_info >= (3, 13):
        __replace__ = _replace  # `copy.replace` support, new in 3.13

    def _simple_name(self) -> str:
        """The type name without its module, as `type(p).__name__` would give for a real namedtuple."""
        return self._type_name.rpartition('.')[2]

    # tuple methods a field of the same name shadows, as on a real namedtuple
    @property
    def count(self) -> Any:  # pyright: ignore[reportIncompatibleMethodOverride]
        return self._field_or_method('count')

    @property
    def index(self) -> Any:  # pyright: ignore[reportIncompatibleMethodOverride]
        return self._field_or_method('index')

    def _field_or_method(self, name: str) -> Any:
        """The field `name`'s value if there is one, else the bound tuple method."""
        if name in self._fields:
            return self[self._fields.index(name)]
        return getattr(tuple, name).__get__(self)


MontyNamedTuple.__module__ = 'pydantic_monty'
