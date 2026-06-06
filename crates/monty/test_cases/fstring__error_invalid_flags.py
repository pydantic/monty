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
