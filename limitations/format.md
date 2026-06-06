# Format mini-language (f-string specs)

Monty implements CPython's
`[[fill]align][sign][#][0][width][grouping][.precision][type]` format
mini-language for f-string interpolations. The following parts are **not**
implemented or diverge from CPython.

The mini-language is only reachable through f-strings: the `format()` builtin
and the `str.format()` method are not implemented (see
[builtins.md](builtins.md)) — `format` raises `NameError` and `str.format`
raises `AttributeError`.

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
