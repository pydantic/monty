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
  and write binary file objects. Monty's file objects have no read position, so
  the chunked read these perform has nothing to implement against — see
  [open.md](open.md).
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

## `binascii`

`binascii.Error`, the hex pair (`hexlify`/`unhexlify` and their
`b2a_hex`/`a2b_hex` aliases), the base64 pair (`b2a_base64`/`a2b_base64`) and
`crc32` are implemented. The uuencode and quoted-printable conversions
(`a2b_uu`, `b2a_uu`, `a2b_qp`, `b2a_qp`) are absent and raise `AttributeError`.

`type(exc).__name__` reports the qualified name `binascii.Error` where CPython
reports the bare `Error` — Monty's general treatment of types whose CPython
`tp_name` is dotted, described in [itertools.md](itertools.md).
