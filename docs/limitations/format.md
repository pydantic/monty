# String formatting

Monty implements CPython 3.14's format mini-language for f-string
interpolations, `str.format()` replacement fields and the `format()`
builtin. `str.format()` supports
positional and keyword fields, automatic and manual numbering, attribute and
item access, `!s` / `!r` / `!a` conversions, nested replacement fields in
format specs, and escaped braces.

Attribute access is limited to lookups that complete inside the sandbox.
If a replacement field needs a host operation, such as
`'{0.environ}'.format(os)`, Monty raises
`NotImplementedError: str.format attribute access cannot suspend` instead of
returning the attribute as CPython does.

`str.format_map()` is not implemented and raises `AttributeError`.

## Printf-style `%` formatting

`str % args` and `bytes % args` implement CPython's printf-style directives:
`%s`, `%r`, `%a`, `%c`, `%d` / `%i` / `%u`, `%o`, `%x` / `%X`, `%e` / `%E`,
`%f` / `%F`, `%g` / `%G` and `%%` (plus `%b` for `bytes`), with the `-`, `+`,
space, `#` and `0` flags, a width and precision given literally or as `*`
arguments, `%(key)s` mapping lookups, and the ignored `h` / `l` / `L` length
modifiers. The divergences:

- **`bytes` `%s` / `%b` accept only `bytes`.** Monty has no `bytearray`,
    `memoryview` or `__bytes__` protocol, so `b'%s' % obj` raises
    `TypeError: %b requires a bytes-like object, or an object that implements __bytes__, not 'C'`
    even when `C` defines `__bytes__`.

- **Operands are coerced through `__index__` only.** A class that defines just
    `__int__` raises `TypeError: %d format: a real number is required, not C`
    where CPython would call it; likewise one defining just `__float__` under
    `%f` raises `must be real number, not C`.

- **A user class is never a mapping.** CPython lets `%(key)s` index any object
    with `__getitem__` and skips the leftover-arguments check for it; Monty
    recognises only `dict` (and its `collections` subclasses), `list`, `bytes`
    and `range`, so `'%(k)s' % instance` raises
    `TypeError: format requires a mapping` and `'abc' % instance` raises
    `not all arguments converted during string formatting`.

- **`%c` rejects surrogate code points.** `'%c' % 0xD800` raises
    `OverflowError: %c arg not in range(0x110000)`, the same error as an
    out-of-range code point, because Monty strings cannot hold lone surrogates
    (CPython returns `'\ud800'`).

## Custom `__format__`

There is no general `__format__` protocol: user-defined methods are ignored
(see [classes.md](classes.md)).
A nonempty spec on a user-class instance raises
`TypeError: unsupported format string passed to Foo.__format__`, even if the
class defines a `__format__` method that would accept it in CPython.
An empty spec uses `str(obj)` instead of calling the method.

## The `n` type uses the C locale only

`n` always behaves as in the C/POSIX locale (Monty has no locale support):
like `d` for integers and `g` for floats, with no digit grouping. CPython
under a grouping locale would insert locale-specific separators; Monty never
does.

## `repr` of non-printable Unicode

`repr` escapes non-printable code points via the `unicode-general-category`
crate, whose Unicode version may lag CPython's, so a code point assigned in a
newer Unicode release than the crate ships could be escaped by Monty while
CPython prints it literally, or the reverse. Common text is unaffected.

## Width / precision bounds

- A `width` or `precision` whose decimal value overflows `usize` raises
    `SyntaxError: Invalid format specifier '...': width or precision overflows usize` in a literal f-string spec.
    Runtime specs for types that support the format mini-language, including
    `str.format()` and `format()`, raise `ValueError` instead, with an additional
    `for object of type '...'` suffix.
- Very large widths/precisions are additionally bounded by the resource
    tracker; see [resource_limits.md](resource_limits.md).

## When spec errors are raised

CPython validates a *static* (literal) f-string spec only when the f-string
executes, so a malformed spec in dead code never raises. Monty validates
literal f-string specs at **compile time** for the structurally-malformed cases:
two or more trailing
characters after the type field (`f'{1:kk}'`, `f'{1:10xyz}'`) and `usize`
overflow, raising `SyntaxError` instead of CPython's runtime `ValueError`. The
message text otherwise matches, minus CPython's `for object of type '...'`
suffix, which needs the runtime value type. Specs whose error *is*
value-type-dependent or only resolvable at format time (`Unknown format code 'k'`, the `Cannot specify …` grouping conflicts, and `Format specifier missing precision`) are deferred to runtime and raise the exact CPython `ValueError`
for types that support the format mini-language. Malformed dynamically-built
specs (`f'{1:{spec}}'`), `str.format()` specs and `format()` specs also raise
`ValueError` for those types. Types that do not support nonempty specs raise
`TypeError` before runtime spec parsing, even when the spec is malformed.
