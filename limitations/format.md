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

Every other presentation type — `b`/`c`/`d`/`o`/`x`/`X` (integers, including
big integers), `e`/`E`/`f`/`F`/`g`/`G`/`%` (floats), `s` (strings), and the
type-less default — is implemented, with sign, `#`, `0`, width, `,`/`_`
grouping, and `.precision` applied as in CPython. `bool` formats as its `int`
value under any non-empty spec (`f'{True:d}'` → `'1'`), and as `'True'`/`'False'`
with no spec. Non-finite floats follow the presentation case
(`f'{float("inf"):F}'` → `'INF'`).

## Type-less float with an explicit precision

A float formatted with a precision but no type char (`f'{x:.3}'`) does not
exactly match CPython. CPython's type-less-with-precision mode is a variant of
`g` with its own significant-digit/threshold rules (e.g.
`format(100.0, '.3')` → `'1e+02'`, distinct from `format(100.0, '.3g')` →
`'100'`); Monty currently routes it through plain `g`. Use an explicit
`g`/`e`/`f` type for predictable output. (The *no-precision* type-less default —
`f'{x}'`, `f'{x:>10}'` — matches CPython's `repr`/shortest digits.)

## Error message ordering

When a spec combines several illegal options (e.g. a sign *and* a precision on an
integer), Monty and CPython both raise `ValueError`, but the *which-error-first*
ordering can differ for some rare combinations. The error type is always
correct; only the message may name a different offending option.

## Width / precision bounds

- A `width` or `precision` whose decimal value overflows `usize` raises
  `SyntaxError: Invalid format specifier '...': width or precision overflows
  usize` rather than being accepted. (CPython is bounded only by memory.)
- Very large widths/precisions are additionally bounded by the resource
  tracker — see [resource_limits.md](resource_limits.md).
