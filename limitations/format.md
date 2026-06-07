# Format mini-language (f-string specs)

Monty implements CPython 3.14's format mini-language for f-string
interpolations. The mini-language is only reachable through f-strings; the
divergences and unsupported mechanisms are listed below.

The other CPython formatting mechanisms are not implemented:

- The `format()` builtin raises `NameError` and the `str.format()` method
  raises `AttributeError` (see [builtins.md](builtins.md)).
- Printf-style `%` formatting (`'%5.3f' % math.pi`, `'%s %s' % (a, b)`) is not
  implemented — `str` has no `__mod__`, so `str % value` raises
  `TypeError: unsupported operand type(s) for %: 'str' and '...'`. Use an
  f-string instead.

## Custom `__format__`

f-strings dispatch to a type's `__format__` only for `date`/`datetime`, which
interpret the spec as a `strftime` string (`f'{dt:%Y-%m-%d}'`) — see
[datetime.md](datetime.md). There is no general `__format__` protocol: user
classes can't customise formatting (Monty has no `class` statement anyway —
see [classes.md](classes.md)), and all other types use the builtin
mini-language formatter.

## The `z` coercion flag is not supported

CPython's `z` flag (`f'{-0.0:z}'` → `'0.0'`) coerces a negative zero
floating-point result to positive zero. Monty does not implement it: the flag
has no spare bit in the compact spec encoding, and the parser rejects it with
`SyntaxError: Invalid format specifier 'z'`. Negative zero therefore formats as
`-0.0` regardless.

## The `n` type uses the C locale only

`n` always behaves as in the C/POSIX locale (Monty has no locale support): like
`d` for integers and `g` for floats, with no digit grouping. CPython under a
grouping locale would insert locale-specific separators; Monty never does.

## `repr` of non-printable Unicode

`repr` escapes non-printable code points via the `unicode-general-category`
crate, whose Unicode version may lag CPython's — so a code point assigned in a
newer Unicode release than the crate ships could be escaped by Monty while
CPython prints it literally (or vice versa). Common text is unaffected.

## Width / precision bounds

- A `width` or `precision` whose decimal value overflows `usize` raises
  `SyntaxError: Invalid format specifier '...': width or precision overflows
  usize` rather than being accepted. (CPython is bounded only by memory.)
- Very large widths/precisions are additionally bounded by the resource
  tracker — see [resource_limits.md](resource_limits.md).
