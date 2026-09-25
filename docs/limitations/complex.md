# `complex`

Monty implements the `complex` type: imaginary literals (`2j`), the `complex()` constructor including its string
form, arithmetic with CPython 3.14's mixed-mode rules, `abs()`, `==` and `hash()` across the numeric tower, the
`real`, `imag` and `conjugate()` members, `complex.from_number()`, and the format mini-language.
A `complex` crosses the host boundary as a Python `complex` and as a `{ __monty_type__: 'Complex', real, imag }`
marker in JavaScript.
The divergences are the ones the rest of the numeric tower already has.

## Construction

- **`complex(obj)` on a class instance** never calls `__complex__`, `__float__` or `__index__`;
    it raises `TypeError: complex() argument must be a string or a number, not X`, as `float()` and `int()` do
    (see [classes.md](classes.md)).
- **Non-ASCII digits in the string form** are rejected: `complex('١j')` raises
    `ValueError: complex() arg is a malformed string`, where CPython accepts any Unicode decimal digit.
    This is the same restriction as `int()` and `float()` (see [builtins.md](builtins.md)).

## Members

- **`z.conjugate` as a value** (without calling it) raises `AttributeError`; only the call `z.conjugate()` works.
    Builtin methods are not first-class values in Monty (see [builtins.md](builtins.md)).
- **`complex.real`, `complex.imag` and `complex.conjugate` on the type** raise `AttributeError`; CPython returns
    descriptors.
- **`real`, `imag` and `conjugate()` on `int` and `float`** raise `AttributeError`; only `complex` carries them.
- **Dunder methods** such as `z.__abs__()` or `z.__complex__()` raise `AttributeError`; use `abs(z)` and `complex(z)`.

## Operators

- **`hash(z)`** differs from CPython's value unless `z` equals a `bool` or a small `int`, because the parts hash
    with Monty's float algorithm (see [builtins.md](builtins.md)); it always agrees with the hash of an equal `int` or
    `float`, so mixed-type dict keys behave.
- **`[1] * 1j`** and **`1j * [1]`** raise `TypeError: unsupported operand type(s) for *: 'list' and 'complex'`;
    CPython says `can't multiply sequence by non-int of type 'complex'` (see [collections.md](collections.md)).
