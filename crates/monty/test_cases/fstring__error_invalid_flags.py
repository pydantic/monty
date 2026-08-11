# Format flags (`,`/`_` grouping and `#` alternate form) are only valid for
# certain presentation types. Illegal combinations raise ValueError at format
# time, matching CPython.

# comma is not allowed with integer base presentations
try:
    f'{255:,x}'
    assert False, 'expected comma with x to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify ',' with 'x'.", str(exc)

try:
    f'{255:,b}'
    assert False, 'expected comma with b to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify ',' with 'b'.", str(exc)

try:
    f'{255:,o}'
    assert False, 'expected comma with o to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify ',' with 'o'.", str(exc)

# neither separator is allowed with the character presentation
try:
    f'{65:,c}'
    assert False, 'expected comma with c to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify ',' with 'c'.", str(exc)

try:
    f'{65:_c}'
    assert False, 'expected underscore with c to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify '_' with 'c'.", str(exc)

# neither separator is allowed when formatting a string
try:
    f'{"hi":,}'
    assert False, 'expected comma with str to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify ',' with 's'.", str(exc)

try:
    f'{"hi":_s}'
    assert False, 'expected underscore with s to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify '_' with 's'.", str(exc)

# the alternate form (`#`) is not allowed with the character presentation
try:
    f'{65:#c}'
    assert False, 'expected # with c to fail'
except ValueError as exc:
    assert str(exc) == "Alternate form (#) not allowed with integer format specifier 'c'", str(exc)

# the alternate form is not allowed when formatting a string
try:
    f'{"hi":#}'
    assert False, 'expected # with str to fail'
except ValueError as exc:
    assert str(exc) == 'Alternate form (#) not allowed in string format specifier', str(exc)

try:
    f'{"hi":#s}'
    assert False, 'expected # with s to fail'
except ValueError as exc:
    assert str(exc) == 'Alternate form (#) not allowed in string format specifier', str(exc)

# === Unknown format code ===
# A single unrecognised trailing char is reported as an unknown code (with the
# value's type), not the generic "Invalid format specifier". Other valid spec
# fields before it (width, sign, `#`, `0`, precision) don't change the message.
try:
    f'{1:k}'
    assert False, 'expected unknown code k to fail'
except ValueError as exc:
    assert str(exc) == "Unknown format code 'k' for object of type 'int'", str(exc)

try:
    f'{1:5k}'
    assert False, 'expected unknown code with width to fail'
except ValueError as exc:
    assert str(exc) == "Unknown format code 'k' for object of type 'int'", str(exc)

try:
    f'{"hi":k}'
    assert False, 'expected unknown code k on str to fail'
except ValueError as exc:
    assert str(exc) == "Unknown format code 'k' for object of type 'str'", str(exc)

try:
    f'{1.5:k}'
    assert False, 'expected unknown code k on float to fail'
except ValueError as exc:
    assert str(exc) == "Unknown format code 'k' for object of type 'float'", str(exc)

# A dynamic (runtime-built) spec reports the same unknown-code message.
try:
    f'{1:{"k"}}'
    assert False, 'expected dynamic unknown code to fail'
except ValueError as exc:
    assert str(exc) == "Unknown format code 'k' for object of type 'int'", str(exc)

# Two or more trailing chars are genuinely malformed (kept distinct from the
# single-char unknown-code case); a runtime spec surfaces it as a ValueError.
try:
    f'{1:{"kk"}}'
    assert False, 'expected malformed spec to fail'
except ValueError as exc:
    assert str(exc) == "Invalid format specifier 'kk' for object of type 'int'", str(exc)

# === Missing precision ===
# A `.` must be followed by precision digits or a fractional grouping option;
# `.f`/`.`/`.d` are "missing precision" (CPython raises this, not silent format).
try:
    f'{1.5:.f}'
    assert False, 'expected missing precision to fail'
except ValueError as exc:
    assert str(exc) == 'Format specifier missing precision', str(exc)

try:
    f'{1:.}'
    assert False, 'expected bare dot to fail'
except ValueError as exc:
    assert str(exc) == 'Format specifier missing precision', str(exc)

# A `.` followed by a grouping option is valid (not missing precision).
assert f'{1.5:._f}' == '1.500_000'
assert f'{1.5:.,f}' == '1.500,000'

# === Grouping conflicts with an unknown trailing char ===
# A grouping option next to an unrecognised char reports the grouping conflict
# (which takes precedence over the unknown-code error), matching CPython.
try:
    f'{1:,k}'
    assert False, 'expected comma+unknown to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify ',' with 'k'.", str(exc)

# Doubled separators and mixed separators each have their own wording.
try:
    f'{1:,,}'
    assert False, 'expected doubled comma to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify ',' with ','.", str(exc)

try:
    f'{1:__}'
    assert False, 'expected doubled underscore to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify '_' with '_'.", str(exc)

try:
    f'{1:,_}'
    assert False, 'expected mixed separators to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify both ',' and '_'.", str(exc)

try:
    f'{1:_,}'
    assert False, 'expected mixed separators to fail'
except ValueError as exc:
    assert str(exc) == "Cannot specify both ',' and '_'.", str(exc)
