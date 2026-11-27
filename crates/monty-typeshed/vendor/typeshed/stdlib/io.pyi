# Modified from python/typeshed: Monty's `io` re-exports its `_io` and has no ABCs or `Reader`/`Writer`.

import sys
from _io import (
    DEFAULT_BUFFER_SIZE as DEFAULT_BUFFER_SIZE,
    BlockingIOError as BlockingIOError,
    BufferedRandom as BufferedRandom,
    BufferedReader as BufferedReader,
    BufferedWriter as BufferedWriter,
    BytesIO as BytesIO,
    StringIO as StringIO,
    TextIOWrapper as TextIOWrapper,
    _WrappedBuffer as _WrappedBuffer,  # used elsewhere in typeshed
    open as open,
    open_code as open_code,
)
from typing import Final

__all__ = [
    "BlockingIOError",
    "open",
    "open_code",
    "BytesIO",
    "StringIO",
    "BufferedReader",
    "BufferedWriter",
    "BufferedRandom",
    "TextIOWrapper",
    "UnsupportedOperation",
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
]

if sys.version_info >= (3, 11):
    from _io import text_encoding as text_encoding

    __all__ += ["DEFAULT_BUFFER_SIZE", "text_encoding"]

SEEK_SET: Final = 0
SEEK_CUR: Final = 1
SEEK_END: Final = 2

class UnsupportedOperation(OSError, ValueError): ...
