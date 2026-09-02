# `base64` module

The base64, base32, base16, base85 and ascii85 codecs plus the MIME helpers
`encodebytes`/`decodebytes` are implemented: `b64encode`, `b64decode`,
`standard_b64encode`, `standard_b64decode`, `urlsafe_b64encode`,
`urlsafe_b64decode`, `b32encode`, `b32decode`, `b32hexencode`, `b32hexdecode`,
`b16encode`, `b16decode`, `b85encode`, `b85decode`, `z85encode`, `z85decode`,
`a85encode`, `a85decode`, `encodebytes`, `decodebytes`. Output bytes, error
types and error messages match CPython 3.14 subject to the divergences below.

## Not implemented

- `base64.encode(input, output)` and `base64.decode(input, output)`, which read
  and write binary file objects. These are pure Python in CPython, so they call
  `read`/`readline`/`write` on whatever they are handed; a Monty builtin
  reaching back into Python runs the call to completion and cannot pause, and
  reading a real file is a suspension out to the host. They would therefore
  work on a duck-typed object and fail on the file `open()` returns — the same
  restriction `filter` and `sorted(key=)` carry. Accessing them raises
  `AttributeError`.
- The `python -m base64` command line interface.

## `a85encode` / `a85decode`

- `wrapcol` reaches CPython's `max()` and then a slice expression. Every
  non-`int` fails at one or the other, so what differs is which message you
  get, not whether the call fails. Monty matches CPython for a `float`
  (`'float' object cannot be interpreted as an integer`) and for a type that
  cannot be ordered against an `int`. A class defining both `__gt__` and
  `__index__` diverges: CPython gets past `max()` and `range()` to fail on
  `i + wrapcol` (`unsupported operand type(s) for +`), while Monty rejects it
  at the comparison, since it does not dispatch ordering dunders at all
  (see [classes.md](classes.md)).

## Input types

Encoders take `bytes`; decoders take `bytes` or an ASCII-only `str`. Monty has
no `bytearray`, `memoryview` or `array`, so the "bytes-like object" the CPython
docs describe is always `bytes` here.

## Module attributes

The attributes CPython leaks as a side effect of how `base64` is written —
`bytes_types`, and the `binascii` and `re` modules it imports — are absent and
raise `AttributeError`. They are not documented API.

## `binascii`

Every name CPython's `binascii` exposes is implemented, so the divergence below
is all that separates the two.

`repr()` of a `binascii.Error` or `binascii.Incomplete` uses the qualified name:
`binascii.Error('Non-hexadecimal digit found')` where CPython gives
`Error('Non-hexadecimal digit found')`. `type(exc).__name__` and
`str(type(exc))` both match CPython — see [exceptions.md](exceptions.md).
