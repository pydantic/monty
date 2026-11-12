"""Builds the host classes for namedtuples decoded from the sandbox.

Mirrors `collections.namedtuple` (CPython 3.14) without `sys.intern` or
`eval`: it compiles the field names into `__new__`, and on 3.12+ compiled
identifiers are immortal, so every new sandbox-chosen field name leaked host memory.
`__new__` takes `*args, **kwargs` instead and binds them with CPython's messages.
"""

import collections
import sys
from collections.abc import Callable, Iterable
from inspect import Parameter, Signature
from keyword import iskeyword
from operator import itemgetter
from typing import Any


def _property_getter(index: int, doc: str) -> Any:
    """The pure-Python field accessor, for implementations without `_tuplegetter`."""
    return property(itemgetter(index), doc=doc)


# the C field accessor `collections.namedtuple` uses
_tuplegetter: Callable[[int, str], Any] = getattr(collections, '_tuplegetter', _property_getter)


# `_replace` with an unknown field raised `ValueError` until 3.13
_REPLACE_ERROR = TypeError if sys.version_info >= (3, 13) else ValueError


def make_namedtuple(typename: str, field_names: tuple[str, ...], module: str | None) -> type[tuple[Any, ...]]:
    """A tuple subclass with named fields, like `collections.namedtuple(typename, field_names, module=module)`."""
    for name in (typename, *field_names):
        if not name.isidentifier():
            raise ValueError(f'Type names and field names must be valid identifiers: {name!r}')
        if iskeyword(name):
            raise ValueError(f'Type names and field names cannot be a keyword: {name!r}')
    seen: set[str] = set()
    for name in field_names:
        if name.startswith('_'):
            raise ValueError(f'Field names cannot start with an underscore: {name!r}')
        if name in seen:
            raise ValueError(f'Encountered duplicate field name: {name!r}')
        seen.add(name)

    num_fields = len(field_names)
    arg_list = ', '.join(field_names) + (',' if num_fields == 1 else '')
    repr_fmt = '(' + ', '.join(f'{name}=%r' for name in field_names) + ')'
    tuple_new: Callable[[type[Any], Iterable[Any]], tuple[Any, ...]] = getattr(tuple, '__new__')

    def __new__(cls: type[tuple[Any, ...]], *args: Any, **kwargs: Any) -> tuple[Any, ...]:
        return tuple_new(cls, _bind(f'{typename}.__new__()', field_names, args, kwargs))

    def _make(cls: type[tuple[Any, ...]], iterable: Any) -> tuple[Any, ...]:
        result = tuple_new(cls, iterable)
        if len(result) != num_fields:
            raise TypeError(f'Expected {num_fields} arguments, got {len(result)}')
        return result

    def _replace(self: tuple[Any, ...], /, **kwds: Any) -> tuple[Any, ...]:
        result = _make(type(self), map(kwds.pop, field_names, self))
        if kwds:
            raise _REPLACE_ERROR(f'Got unexpected field names: {list(kwds)!r}')
        return result

    def __repr__(self: tuple[Any, ...]) -> str:
        return self.__class__.__name__ + repr_fmt % self

    def _asdict(self: tuple[Any, ...]) -> dict[str, Any]:
        return dict(zip(field_names, self))

    def __getnewargs__(self: tuple[Any, ...]) -> tuple[Any, ...]:
        return tuple(self)

    __new__.__doc__ = f'Create new instance of {typename}({arg_list})'
    __new__.__dict__['__signature__'] = Signature(
        [Parameter(name, Parameter.POSITIONAL_OR_KEYWORD) for name in ('_cls', *field_names)]
    )
    _make.__doc__ = f'Make a new {typename} object from a sequence or iterable'
    _replace.__doc__ = f'Return a new {typename} object replacing specified fields with new values'
    __repr__.__doc__ = 'Return a nicely formatted representation string'
    _asdict.__doc__ = 'Return a new dict which maps field names to their values.'
    __getnewargs__.__doc__ = 'Return self as a plain tuple.  Used by copy and pickle.'
    for method in (__new__, _make, _replace, __repr__, _asdict, __getnewargs__):
        method.__qualname__ = f'{typename}.{method.__name__}'

    namespace: dict[str, Any] = {
        '__doc__': f'{typename}({arg_list})',
        '__module__': module if module is not None else '__main__',
        '__slots__': (),
        '_fields': field_names,
        '_field_defaults': {},
        '__new__': __new__,
        '_make': classmethod(_make),
        '__replace__': _replace,
        '_replace': _replace,
        '__repr__': __repr__,
        '_asdict': _asdict,
        '__getnewargs__': __getnewargs__,
        '__match_args__': field_names,
    }
    for index, name in enumerate(field_names):
        namespace[name] = _tuplegetter(index, f'Alias for field number {index}')
    return type(typename, (tuple,), namespace)


def _bind(func: str, field_names: tuple[str, ...], args: tuple[Any, ...], kwargs: dict[str, Any]) -> list[Any]:
    """Binds `__new__` arguments to the fields, raising CPython's message for a bad call.

    Checks run in CPython's order: keywords first, then the positional count, then missing fields.
    """
    for name in kwargs:
        if name not in field_names:
            raise TypeError(f"{func} got an unexpected keyword argument '{name}'")
        if field_names.index(name) < len(args):
            raise TypeError(f"{func} got multiple values for argument '{name}'")
    if len(args) > len(field_names):
        raise TypeError(f'{func} takes {len(field_names) + 1} positional arguments but {len(args) + 1} were given')
    missing = [name for name in field_names[len(args) :] if name not in kwargs]
    if missing:
        raise TypeError(f'{func} missing {len(missing)} required positional {_plural(missing)}: {_names(missing)}')
    return [*args, *(kwargs[name] for name in field_names[len(args) :])]


def _plural(missing: list[str]) -> str:
    """`argument` or `arguments`, as CPython words a missing-arguments error."""
    return 'argument' if len(missing) == 1 else 'arguments'


def _names(missing: list[str]) -> str:
    """Quoted names joined as CPython lists them: `'a'`, `'a' and 'b'`, `'a', 'b', and 'c'`."""
    quoted = [f"'{name}'" for name in missing]
    if len(quoted) <= 2:
        return ' and '.join(quoted)
    return ', '.join(quoted[:-1]) + ', and ' + quoted[-1]
