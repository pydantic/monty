# === maketrans: dict form ===
assert str.maketrans({'a': 'b', 98: None, 'c': 100}) == {97: 'b', 98: None, 99: 100}
assert str.maketrans({}) == {}
assert str.maketrans({'a': 1.5}) == {97: 1.5}

# === maketrans: string forms ===
assert str.maketrans('abc', 'xyz') == {97: 120, 98: 121, 99: 122}
assert str.maketrans('ab', 'xy', 'cd') == {97: 120, 98: 121, 99: None, 100: None}
assert str.maketrans('a', 'b', 'a') == {97: None}
assert str.maketrans('', '') == {}
assert 'abc'.maketrans('a', 'b') == {97: 98}
assert str.maketrans('é', '€') == {233: 8364}

# === maketrans: errors ===
try:
    str.maketrans('ab', 'xyz')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'the first two maketrans arguments must have equal length'

try:
    str.maketrans({'ab': 1})
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'string keys in translatetable must be of length 1'

try:
    str.maketrans([1])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'if you give only one argument to maketrans it must be a dict'

try:
    str.maketrans(1, 'a')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'first maketrans argument must be a string if there is a second argument'

try:
    str.maketrans('a', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'maketrans() argument 2 must be str, not int'

try:
    str.maketrans('a', 'b', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'maketrans() argument 3 must be str, not int'

try:
    str.maketrans()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'maketrans expected at least 1 argument, got 0'

try:
    str.maketrans('a', 'b', 'c', 'd')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'maketrans expected at most 3 arguments, got 4'

try:
    str.maketrans({1.5: 'a'})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'keys in translate table mustbe strings or integers'

try:
    str.maketrans(x='a')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'str.maketrans() takes no keyword arguments'

# === translate: mappings ===
assert 'hello'.translate({104: 'J', 101: None, 108: 'LL'}) == 'JLLLLo'
assert 'abc'.translate(['x', 'y']) == 'abc'
assert 'abc'.translate('XYZ') == 'abc'
assert 'aaa'.translate({97: 'xy'}) == 'xyxyxy'
assert ''.translate({}) == ''
assert 'abc'.translate({97: True}) == '\x01bc'
assert 'abc'.translate({97: 120}) == 'xbc'
assert 'abc'.translate(str.maketrans('abc', 'xyz')) == 'xyz'
assert 'abc'.translate(str.maketrans('', '', 'b')) == 'ac'
assert 'é€😀'.translate({233: 'e', 8364: 'E', 128512: None}) == 'eE'
assert 'ab'.translate({97: 0x10FFFF}) == '\U0010ffffb'


class Table:
    def __getitem__(self, key):
        if key == 97:
            return 'A'
        raise KeyError('missing')


assert 'abca'.translate(Table()) == 'AbcA'

# === translate: errors ===
try:
    'abc'.translate({97: 1.5})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'character mapping must return integer, None or str'

try:
    'abc'.translate({97: b'x'})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'character mapping must return integer, None or str'

for bad in (0x110000, -1, 2**70):
    try:
        'abc'.translate({97: bad})
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == 'character mapping must be in range(0x110000)'

try:
    'abc'.translate(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not subscriptable"

try:
    'abc'.translate()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'str.translate() takes exactly one argument (0 given)'

try:
    'abc'.translate(table={})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'str.translate() takes no keyword arguments'


class Boom:
    def __getitem__(self, key):
        raise RuntimeError('boom')


try:
    'abc'.translate(Boom())
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'boom'
