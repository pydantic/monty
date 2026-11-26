# pickle

Monty provides `dumps`, `loads`, `DEFAULT_PROTOCOL` and `HIGHEST_PROTOCOL`.
Both constants are 5.
`dumps` accepts protocols 4 and 5, and a negative protocol selects 5.
`loads` accepts `bytes` with a protocol 4 or 5 header. It also reads the frames CPython emits.
CPython may produce different bytes for the same value.

Supported values are `None`, `bool`, `int`, `float`, `str`, `bytes`, `list`, `tuple`, `dict`,
`set` and `frozenset`.
The encoder limits nesting to 128 levels and raises `RecursionError` when exceeded.

The decoder accepts opcodes that construct built-in values and manage references.
Other opcodes, including global lookups, calls, instance restoration, and external buffers, raise `ValueError`.
Invalid lengths, stack operands and memo indexes raise `ValueError`, as does truncated input.
CPython can raise `pickle.UnpicklingError` or `EOFError` in these cases.
The encoder raises `TypeError` for values outside the supported types.
Strings must be representable as UTF-8 without lone surrogates.

File APIs, `Pickler`, `Unpickler`, pickle-specific exception classes, and the optional compatibility and buffer arguments are absent.
