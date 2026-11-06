"""Monty's `urllib.parse`: the `str` surface of CPython's module. Results are plain
named tuples, so the result classes are not importable and are private here."""

from collections.abc import Iterable, Mapping, Sequence
from typing import AnyStr, NamedTuple, Protocol, overload, type_check_only

from typing_extensions import TypeAlias

__all__ = [
    'urlparse',
    'urlunparse',
    'urljoin',
    'urldefrag',
    'urlsplit',
    'urlunsplit',
    'urlencode',
    'parse_qs',
    'parse_qsl',
    'quote',
    'quote_plus',
    'quote_from_bytes',
    'unquote',
    'unquote_plus',
    'unquote_to_bytes',
]

@type_check_only
class _NetlocResultMixin:
    @property
    def username(self) -> str | None: ...
    @property
    def password(self) -> str | None: ...
    @property
    def hostname(self) -> str | None: ...
    @property
    def port(self) -> int | None: ...

class _DefragResultBase(NamedTuple):
    url: str
    fragment: str

class _SplitResultBase(NamedTuple):
    scheme: str
    netloc: str
    path: str
    query: str
    fragment: str

class _ParseResultBase(NamedTuple):
    scheme: str
    netloc: str
    path: str
    params: str
    query: str
    fragment: str

@type_check_only
class _DefragResult(_DefragResultBase):
    def geturl(self) -> str: ...

@type_check_only
class _SplitResult(_SplitResultBase, _NetlocResultMixin):
    def geturl(self) -> str: ...

@type_check_only
class _ParseResult(_ParseResultBase, _NetlocResultMixin):
    def geturl(self) -> str: ...

def parse_qs(
    qs: AnyStr | None,
    keep_blank_values: bool = False,
    strict_parsing: bool = False,
    encoding: str = 'utf-8',
    errors: str = 'replace',
    max_num_fields: int | None = None,
    separator: str = '&',
) -> dict[AnyStr, list[AnyStr]]: ...
def parse_qsl(
    qs: AnyStr | None,
    keep_blank_values: bool = False,
    strict_parsing: bool = False,
    encoding: str = 'utf-8',
    errors: str = 'replace',
    max_num_fields: int | None = None,
    separator: str = '&',
) -> list[tuple[AnyStr, AnyStr]]: ...
@overload
def quote(
    string: str, safe: str | Iterable[int] = '/', encoding: str | None = None, errors: str | None = None
) -> str: ...
@overload
def quote(string: bytes, safe: str | Iterable[int] = '/') -> str: ...
def quote_from_bytes(bs: bytes, safe: str | Iterable[int] = '/') -> str: ...
@overload
def quote_plus(
    string: str, safe: str | Iterable[int] = '', encoding: str | None = None, errors: str | None = None
) -> str: ...
@overload
def quote_plus(string: bytes, safe: str | Iterable[int] = '') -> str: ...
def unquote(string: str | bytes, encoding: str = 'utf-8', errors: str = 'replace') -> str: ...
def unquote_to_bytes(string: str | bytes) -> bytes: ...
def unquote_plus(string: str, encoding: str = 'utf-8', errors: str = 'replace') -> str: ...
def urldefrag(url: str) -> _DefragResult: ...

# The values are passed through `str()` (unless they are bytes), so anything is valid.
_QueryType: TypeAlias = (
    Mapping[str, object]
    | Mapping[bytes, object]
    | Mapping[str | bytes, object]
    | Mapping[str, Sequence[object]]
    | Mapping[bytes, Sequence[object]]
    | Mapping[str | bytes, Sequence[object]]
    | Sequence[tuple[str | bytes, object]]
    | Sequence[tuple[str | bytes, Sequence[object]]]
)

@type_check_only
class _QuoteVia(Protocol):
    @overload
    def __call__(self, string: str, safe: str | bytes, encoding: str, errors: str, /) -> str: ...
    @overload
    def __call__(self, string: bytes, safe: str | bytes, /) -> str: ...

def urlencode(
    query: _QueryType,
    doseq: bool = False,
    safe: str | bytes = '',
    encoding: str | None = None,
    errors: str | None = None,
    quote_via: _QuoteVia = ...,
) -> str: ...
def urljoin(base: str, url: str | None, allow_fragments: bool = True) -> str: ...
def urlparse(url: str, scheme: str = '', allow_fragments: bool = True) -> _ParseResult: ...
def urlsplit(url: str, scheme: str = '', allow_fragments: bool = True) -> _SplitResult: ...

# Requires an iterable of length 6
def urlunparse(components: Iterable[str | None]) -> str: ...

# Requires an iterable of length 5
def urlunsplit(components: Iterable[str | None]) -> str: ...
