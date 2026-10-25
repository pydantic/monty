# Based on python/typeshed's `stdlib/csv.pyi` and `_csv.pyi`, folded into one
# module and cut down to what Monty implements at runtime: `register_dialect`,
# `unregister_dialect`, `Sniffer` and `__version__` are absent and
# `field_size_limit` takes no argument, so type checking rejects them up front
# instead of letting them fail at runtime. The built-in dialects are `Dialect`
# instances rather than subclasses, since `csv.excel()` is not callable in Monty.
# Extend in lockstep with `crates/monty/src/modules/csv.rs`.

from collections.abc import Collection, Iterable, Mapping, Sequence
from typing import Any, Final, Generic, Literal, TypeVar

from _typeshed import SupportsWrite
from typing_extensions import Self, TypeAlias

__all__ = [
    'QUOTE_MINIMAL',
    'QUOTE_ALL',
    'QUOTE_NONNUMERIC',
    'QUOTE_NONE',
    'QUOTE_STRINGS',
    'QUOTE_NOTNULL',
    'Error',
    'Dialect',
    'excel',
    'excel_tab',
    'unix_dialect',
    'field_size_limit',
    'reader',
    'writer',
    'get_dialect',
    'list_dialects',
    'DictReader',
    'DictWriter',
]

_T = TypeVar('_T')

QUOTE_MINIMAL: Final = 0
QUOTE_ALL: Final = 1
QUOTE_NONNUMERIC: Final = 2
QUOTE_NONE: Final = 3
QUOTE_STRINGS: Final = 4
QUOTE_NOTNULL: Final = 5

_QuotingType: TypeAlias = Literal[0, 1, 2, 3, 4, 5]
_DialectLike: TypeAlias = str | Dialect | type | object

class Error(Exception): ...

class Dialect:
    delimiter: str
    quotechar: str | None
    escapechar: str | None
    doublequote: bool
    skipinitialspace: bool
    lineterminator: str
    quoting: _QuotingType
    strict: bool
    def __init__(
        self,
        dialect: _DialectLike | None = None,
        *,
        delimiter: str = ',',
        doublequote: bool = True,
        escapechar: str | None = None,
        lineterminator: str = '\r\n',
        quotechar: str | None = '"',
        quoting: _QuotingType = 0,
        skipinitialspace: bool = False,
        strict: bool = False,
    ) -> None: ...

excel: Dialect
excel_tab: Dialect
unix_dialect: Dialect

class Reader:
    @property
    def dialect(self) -> Dialect: ...
    line_num: int
    def __iter__(self) -> Self: ...
    def __next__(self) -> list[str]: ...

class Writer:
    @property
    def dialect(self) -> Dialect: ...
    def writerow(self, row: Iterable[Any], /) -> Any: ...
    def writerows(self, rows: Iterable[Iterable[Any]], /) -> None: ...

def reader(
    csvfile: Iterable[str],
    /,
    dialect: _DialectLike = 'excel',
    *,
    delimiter: str = ',',
    quotechar: str | None = '"',
    escapechar: str | None = None,
    doublequote: bool = True,
    skipinitialspace: bool = False,
    lineterminator: str = '\r\n',
    quoting: _QuotingType = 0,
    strict: bool = False,
) -> Reader: ...
def writer(
    csvfile: SupportsWrite[str],
    /,
    dialect: _DialectLike = 'excel',
    *,
    delimiter: str = ',',
    quotechar: str | None = '"',
    escapechar: str | None = None,
    doublequote: bool = True,
    skipinitialspace: bool = False,
    lineterminator: str = '\r\n',
    quoting: _QuotingType = 0,
    strict: bool = False,
) -> Writer: ...
def get_dialect(name: str) -> Dialect: ...
def list_dialects() -> list[str]: ...
def field_size_limit() -> int: ...

class DictReader(Generic[_T]):
    fieldnames: Sequence[_T] | None
    restkey: _T | None
    restval: str | Any | None
    reader: Reader
    dialect: _DialectLike
    line_num: int
    def __init__(
        self,
        f: Iterable[str],
        fieldnames: Sequence[_T] | None = None,
        restkey: _T | None = None,
        restval: str | Any | None = None,
        dialect: _DialectLike = 'excel',
        *,
        delimiter: str = ',',
        quotechar: str | None = '"',
        escapechar: str | None = None,
        doublequote: bool = True,
        skipinitialspace: bool = False,
        lineterminator: str = '\r\n',
        quoting: _QuotingType = 0,
        strict: bool = False,
    ) -> None: ...
    def __iter__(self) -> Self: ...
    def __next__(self) -> dict[_T | Any, str | Any]: ...

class DictWriter(Generic[_T]):
    fieldnames: Collection[_T]
    restval: Any | None
    extrasaction: Literal['raise', 'ignore']
    writer: Writer
    def __init__(
        self,
        f: SupportsWrite[str],
        fieldnames: Collection[_T],
        restval: Any | None = '',
        extrasaction: Literal['raise', 'ignore'] = 'raise',
        dialect: _DialectLike = 'excel',
        *,
        delimiter: str = ',',
        quotechar: str | None = '"',
        escapechar: str | None = None,
        doublequote: bool = True,
        skipinitialspace: bool = False,
        lineterminator: str = '\r\n',
        quoting: _QuotingType = 0,
        strict: bool = False,
    ) -> None: ...
    def writeheader(self) -> Any: ...
    def writerow(self, rowdict: Mapping[_T, Any]) -> Any: ...
    def writerows(self, rowdicts: Iterable[Mapping[_T, Any]]) -> None: ...
