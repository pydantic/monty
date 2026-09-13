# `str` character predicates

The case methods (`lower`, `upper`, `casefold`, `capitalize`, `title`, `swapcase`, `isupper`, `islower`,
`istitle`) use tables generated from CPython 3.14 (`scripts/gen_case_data.py`) and match it for every code point.
The other character-class predicates use Rust's standard library and hand-written tables, and diverge as below.
Counts are code points, measured against CPython 3.14 (Unicode 16.0.0) with the Rust standard library's Unicode
17.0.0 data.

## `isalpha()` and `isalnum()`

Return `True` for 6393 and 6167 code points where CPython returns `False`: combining marks (`Mn`, `Mc`), letter
numbers (`Nl`, e.g. Roman numerals `Ⅰ`), a few symbols (`So`), and code points first assigned in Unicode 17.
Monty tests the Unicode `Alphabetic` property, CPython the `L*` general categories.

## `isdecimal()`, `isdigit()` and `isnumeric()`

- `isdecimal()` returns `False` for 200 `Nd` digits in blocks added since the table was written
    (e.g. Garay, Kirat Rai, Ol Onal, Sunuwar, mathematical digits `𝟎`–`𝟿`).
- `isdigit()` misses the same digits plus 52 `No` code points (e.g. Ethiopic `፩`–`፱`, dingbat circled digits `❶`),
    and returns `True` for circled numbers `⑩`–`⑳`, `⓾`, `➉` and `➓` where CPython returns `False`.
- `isnumeric()` returns `False` for the 91 CJK ideographs with a numeric value (`一`, `二`, `十`, `百`, `万` …),
    and `True` for 13 code points unassigned in Unicode 16.

## `isspace()`

Returns `False` for `\x1c`–`\x1f`, which CPython treats as whitespace.

## `isprintable()`

Not implemented; raises `AttributeError`.

## `isidentifier()`

Returns `True` for 4647 code points first assigned in Unicode 17, which CPython 3.14 rejects.
