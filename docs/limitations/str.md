# `str` case methods

`upper()`, `lower()`, `title()`, `capitalize()`, `swapcase()`, `isupper()`, `islower()` and `istitle()` take their
case mappings from Rust's standard library, whose Unicode tables can be newer than CPython 3.14's (Unicode 16.0.0).
`casefold()`, and the titlecase exceptions `title()` and `capitalize()` apply, are pinned to Unicode 16.0.0.

## Unicode version skew

With the current toolchain (Unicode 17.0.0) these 58 code points have case mappings in Monty and none in CPython 3.14,
so `upper()`, `lower()` and `title()` change them and `isupper()`, `islower()` and `istitle()` can return `True` where
CPython returns `False`:

- `U+0295`, `U+A7CE`–`U+A7CF`, `U+A7D2`–`U+A7D5`, `U+A7F1` (Latin Extended-D additions)
- `U+16EA0`–`U+16EB8`, `U+16EBB`–`U+16ED3` (Beria Erfe)

`casefold()` leaves them unchanged, as CPython does.
Code points assigned in Unicode 16.0.0 or earlier are unaffected.
