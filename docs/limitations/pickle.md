# pickle

Monty provides `dumps`, `loads`, `DEFAULT_PROTOCOL` and `HIGHEST_PROTOCOL`.
Both constants are 5.
`dumps` accepts protocols 2 through 5, and a negative protocol selects 5.
`loads` accepts `bytes` with a protocol 2 through 5 header, including CPython's framed pickles.
CPython may produce different bytes for the same value.

Supported values are `None`, `bool`, `int`, `float`, `str`, `bytes`, `list`, `tuple`, `dict`,
`set` and `frozenset`.
The encoder limits nesting to 128 levels and raises `RecursionError` when exceeded.

The decoder accepts opcodes that construct built-in values and manage references.
Protocol 2 encodes `bytes` through a Latin-1 string. Protocols 2 and 3 encode sets and frozensets through lists.
Monty reconstructs these values directly.
Arbitrary globals, calls, instance restoration, and external buffers raise `ValueError`.
Binary Python 2 strings decode as `str` when they contain only ASCII. Other bytes raise `ValueError`.
Invalid lengths, stack operands and memo indexes raise `ValueError`, as does truncated input.
CPython can raise `pickle.UnpicklingError` or `EOFError` in these cases.
The encoder raises `TypeError` for values outside the supported types.
Strings must be representable as UTF-8 without lone surrogates.

File APIs, `Pickler`, `Unpickler`, pickle-specific exception classes, and the optional compatibility and buffer arguments are absent.
