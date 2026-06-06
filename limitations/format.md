# Format mini-language (f-string specs)

Monty implements CPython's
`[[fill]align][sign][#][0][width][grouping][.precision][type]` format
mini-language for f-string interpolations. The following parts are **not**
implemented or diverge from CPython.

The mini-language is only reachable through f-strings. The other CPython
formatting mechanisms are not implemented:

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

## Unsupported types

- The `n` type (locale-aware number) is not implemented; it raises
  `ValueError: Unknown format code 'n' for object of type '...'`. CPython
  formats the number using the active locale. This is the only presentation
  type CPython supports that Monty does not.

## Width / precision bounds

- A `width` or `precision` whose decimal value overflows `usize` raises
  `SyntaxError: Invalid format specifier '...': width or precision overflows
  usize` rather than being accepted. (CPython is bounded only by memory.)
- Very large widths/precisions are additionally bounded by the resource
  tracker — see [resource_limits.md](resource_limits.md).
