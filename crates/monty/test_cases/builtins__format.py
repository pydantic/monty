# === Basic values ===
assert format(3) == '3'
assert format(3, '') == '3'
assert format(12.5, '>8.1f') == '    12.5'
assert format('a', '^5') == '  a  '
assert format(None) == 'None'
assert format([1, 2], '') == '[1, 2]'
assert format(True) == 'True'
assert format(True, 'd') == '1'
assert format(1e100, 'e') == '1.000000e+100'
assert format(2**70, 'x') == '400000000000000000'
assert format(255, '#010b') == '0b11111111'
assert format(1234567, ',') == '1,234,567'
assert format(0.5, '.0%') == '50%'
assert format('x', '*<4') == 'x***'

# === Same output as f-strings and str.format ===
spec = '>+8.2f'
assert format(3.14159, spec) == f'{3.14159:{spec}}' == '{:>+8.2f}'.format(3.14159)
assert format('hi', '5') == f'{"hi":5}'


# === Objects without a format mini-language ===
class Unformatted:
    def __str__(self):
        return 'plain'


for value, type_name, text in (
    ([1], 'list', '[1]'),
    (None, 'NoneType', 'None'),
    ((1,), 'tuple', '(1,)'),
    ({'a': 1}, 'dict', "{'a': 1}"),
    ({1}, 'set', '{1}'),
    (frozenset([1]), 'frozenset', 'frozenset({1})'),
    (range(1), 'range', 'range(0, 1)'),
    (slice(1), 'slice', 'slice(None, 1, None)'),
    (b'x', 'bytes', "b'x'"),
    (Unformatted(), 'Unformatted', 'plain'),
):
    assert format(value) == text
    assert format(value, '') == text
    for spec in ('5', '>5', '0', '０', 's', '+', '#', '.', 'q', 'not-a-spec'):
        try:
            format(value, spec)
            assert False, 'expected unsupported format spec to fail'
        except TypeError as e:
            assert str(e) == f'unsupported format string passed to {type_name}.__format__'

# === Heap spec strings ===
assert format(7, ''.join(['0', '3', 'd'])) == '007'
assert format(7, '0' + '5d') == '00007'

# === Errors ===
try:
    format()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'format expected at least 1 argument, got 0'

try:
    format(1, '', 3)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'format expected at most 2 arguments, got 3'

try:
    format(1, 2)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'format() argument 2 must be str, not int'

try:
    format(1, None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'format() argument 2 must be str, not None'

try:
    format(1, b'd')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'format() argument 2 must be str, not bytes'

try:
    format(1, format_spec='d')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'format() takes no keyword arguments'

try:
    format('x', 'd')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "Unknown format code 'd' for object of type 'str'"

try:
    format(1, 'q')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "Unknown format code 'q' for object of type 'int'"

try:
    format(1.5, '.')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'Format specifier missing precision'
