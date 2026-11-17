from collections import Counter, defaultdict


def error(f):
    try:
        f()
    except Exception as e:
        return f'{type(e).__name__}: {e}'
    assert False, 'expected an exception'


# === str.maketrans with two or three strings ===
assert str.maketrans('ab', 'xy') == {97: 120, 98: 121}
assert str.maketrans('ab', 'xy', 'c') == {97: 120, 98: 121, 99: None}
assert str.maketrans('', '') == {}
assert str.maketrans('', '', '') == {}
assert str.maketrans('', '', 'ab') == {97: None, 98: None}
# a repeated character keeps the last mapping, and z overrides x
assert str.maketrans('aa', 'bc') == {97: 99}
assert str.maketrans('ab', 'cd', 'a') == {97: None, 98: 100}
# lengths are counted in characters, not bytes
assert str.maketrans('é😀', 'xy') == {233: 120, 128512: 121}
assert str.maketrans('ab', 'é😀') == {97: 233, 98: 128512}
# also reachable on instances
assert 'x'.maketrans('a', 'b') == {97: 98}
assert ''.maketrans({'a': 1}) == {97: 1}

# === str.maketrans with a dict ===
assert str.maketrans({}) == {}
assert str.maketrans({'a': 'b', 98: None, 'c': 1}) == {97: 'b', 98: None, 99: 1}
assert str.maketrans({'é': 'e', 0x1F600: ':)'}) == {233: 'e', 128512: ':)'}
# int keys, including bools and big ints, are kept as they are
assert repr(str.maketrans({True: 1})) == '{True: 1}'
assert str.maketrans({2**70: 3}) == {2**70: 3}
# a str key and an int key for the same code point collapse into one entry
assert str.maketrans({'a': 1, 97: 2}) == {97: 2}
# values are stored as they are, not copied
source = {'a': [1]}
table = str.maketrans(source)
assert table[97] is source['a']
# the result is a new dict
assert table is not source

# === str.maketrans errors ===
assert error(lambda: str.maketrans('a')) == 'TypeError: if you give only one argument to maketrans it must be a dict'
assert error(lambda: str.maketrans(1)) == 'TypeError: if you give only one argument to maketrans it must be a dict'
assert error(lambda: str.maketrans([('a', 'b')])) == (
    'TypeError: if you give only one argument to maketrans it must be a dict'
)
assert error(lambda: str.maketrans(defaultdict(int, a=1))) == (
    'TypeError: if you give only one argument to maketrans it must be a dict'
)
assert error(lambda: str.maketrans(Counter('a'))) == (
    'TypeError: if you give only one argument to maketrans it must be a dict'
)
assert error(lambda: str.maketrans({'ab': 1})) == 'ValueError: string keys in translatetable must be of length 1'
assert error(lambda: str.maketrans({'': 1})) == 'ValueError: string keys in translatetable must be of length 1'
assert error(lambda: str.maketrans({1.5: 1})) == 'TypeError: keys in translate table mustbe strings or integers'
assert error(lambda: str.maketrans({None: 1})) == 'TypeError: keys in translate table mustbe strings or integers'
# keys are checked in order
assert error(lambda: str.maketrans({'a': 1, 2.5: 3})) == 'TypeError: keys in translate table mustbe strings or integers'
assert (
    error(lambda: str.maketrans({'ab': 1, 2.5: 3})) == 'ValueError: string keys in translatetable must be of length 1'
)
assert error(lambda: str.maketrans('ab', 'x')) == 'ValueError: the first two maketrans arguments must have equal length'
assert error(lambda: str.maketrans('é', 'ab')) == 'ValueError: the first two maketrans arguments must have equal length'
assert error(lambda: str.maketrans(1, 'x')) == (
    'TypeError: first maketrans argument must be a string if there is a second argument'
)
assert error(lambda: str.maketrans({}, 'a')) == (
    'TypeError: first maketrans argument must be a string if there is a second argument'
)
assert error(lambda: str.maketrans('a', 1)) == 'TypeError: maketrans() argument 2 must be str, not int'
assert error(lambda: str.maketrans('a', 'b', 1)) == 'TypeError: maketrans() argument 3 must be str, not int'
assert error(lambda: str.maketrans('a', 'b', None)) == 'TypeError: maketrans() argument 3 must be str, not None'
# y and z are type-checked before x
assert error(lambda: str.maketrans(1, 2)) == 'TypeError: maketrans() argument 2 must be str, not int'
assert error(lambda: str.maketrans()) == 'TypeError: maketrans expected at least 1 argument, got 0'
assert error(lambda: str.maketrans('a', 'b', 'c', 'd')) == 'TypeError: maketrans expected at most 3 arguments, got 4'
assert error(lambda: str.maketrans(x={})) == 'TypeError: str.maketrans() takes no keyword arguments'

# === str.translate with dicts ===
assert 'abc'.translate({97: 'X', 98: None, 99: 100}) == 'Xd'
assert 'abc'.translate({}) == 'abc'
assert ''.translate({97: 'X'}) == ''
assert 'hello world'.translate(str.maketrans('lo', 'LO', ' ')) == 'heLLOwOrLd'
assert 'abc'.translate({97: True}) == '\x01bc'
assert 'abc'.translate({97: 0}) == '\x00bc'
# deletion, including every character and runs of deletions
assert 'abc'.translate({97: None, 98: None, 99: None}) == ''
assert 'aabbaacc'.translate({97: None}) == 'bbcc'
assert 'abcabc'.translate(str.maketrans('', '', 'ab')) == 'cc'
# replacements of any length
assert 'abc'.translate({97: 'xyz'}) == 'xyzbc'
assert 'abc'.translate({97: ''}) == 'bc'
assert 'aaa'.translate({97: 'ab'}) == 'ababab'
assert 'a-b'.translate({45: ' - '}) == 'a - b'

# === str.translate with non-ASCII ===
assert 'café'.translate({233: 'e'}) == 'cafe'
assert 'abc'.translate({97: 'é'}) == 'ébc'
assert 'abc'.translate({97: 0x1F600}) == '😀bc'
assert 'abc'.translate({97: 300}) == 'Ĭbc'
assert 'a😀b'.translate({0x1F600: 'smile', 98: 0x1F601}) == 'asmile😁'
assert 'naïve café'.translate(str.maketrans('ïé', 'ie')) == 'naive cafe'
assert 'aébaé'.translate({233: None}) == 'aba'
assert 'aébaé'.translate({97: None, 233: None, 98: None}) == ''
assert 'ñ'.translate({0xF1: 'n\u0303'}) == 'n\u0303'
assert 'ΣΑΣ'.translate(str.maketrans('ΣΑ', 'sa')) == 'sas'

# === str.translate with other tables ===
assert 'abc'.translate(['A'] * 200) == 'AAA'
assert 'abc'.translate(['x'] * 98) == 'xbc'
assert 'abc'.translate('xyz' * 40) == 'yzx'
assert 'abc'.translate(b'xyz' * 40) == 'yzx'
assert 'abc'.translate((1, 2)) == 'abc'
assert 'abc'.translate(range(1000, 2000)) == 'щъы'
assert 'abc'.translate(range(98)) == 'abc'
assert 'abc'.translate(range(200)) == 'abc'
assert 'abc'.translate(Counter()) == '\x00\x00\x00'
assert 'abc'.translate(Counter({98: 66})) == '\x00B\x00'
dd = defaultdict(lambda: '-', {98: 'B'})
assert 'abca'.translate(dd) == '-B--'
assert dd == {97: '-', 98: 'B', 99: '-'}


# === str.translate with a user-defined table ===
class Upper:
    def __getitem__(self, key):
        if key == 97:
            return 'Z'
        raise LookupError


assert 'ab'.translate(Upper()) == 'Zb'


class Missing:
    def __init__(self, exc):
        self.exc = exc

    def __getitem__(self, key):
        raise self.exc


assert 'ab'.translate(Missing(KeyError('k'))) == 'ab'
assert 'ab'.translate(Missing(IndexError('i'))) == 'ab'
assert 'aé'.translate(Missing(LookupError('l'))) == 'aé'
assert error(lambda: 'a'.translate(Missing(ValueError('boom')))) == 'ValueError: boom'
assert error(lambda: 'é'.translate(Missing(TypeError('bad')))) == 'TypeError: bad'
# an empty string never looks anything up
assert ''.translate(Missing(ValueError('boom'))) == ''


# === str.translate lookup order ===
# CPython caches ASCII results that are a single ASCII character or a deletion,
# and looks ahead over runs of deletions; both are visible to `__getitem__`.
class Recorder:
    def __init__(self, result):
        self.calls = []
        self.result = result

    def __getitem__(self, key):
        self.calls.append(key)
        return self.result(key)


def calls(text, result):
    recorder = Recorder(result)
    return text.translate(recorder), recorder.calls


assert calls('aabab', lambda k: k) == ('aabab', [97, 98])
assert calls('aabab', lambda k: None) == ('', [97, 98])
assert calls('aabab', lambda k: chr(k).upper()) == ('AABAB', [97, 98])
assert calls('aabab', lambda k: 'xy') == ('xyxyxyxyxy', [97, 97, 97, 98, 97, 98])
assert calls('aabab', lambda k: 'é') == ('ééééé', [97, 97, 97, 98, 97, 98])
assert calls('aabab', lambda k: 300) == ('ĬĬĬĬĬ', [97, 97, 97, 98, 97, 98])
assert calls('abcab', lambda k: 'é' if k == 99 else k) == ('abéab', [97, 98, 99, 99, 97, 98])
assert calls('aébaé', lambda k: k) == ('aébaé', [97, 233, 98, 97, 233])
assert calls('aébaé', lambda k: None) == ('', [97, 233, 98, 97, 233])
assert calls('aébaé', lambda k: chr(k).upper()) == ('AÉBAÉ', [97, 233, 98, 97, 233])
# after a deleted run, the character that ended it is looked up again
assert calls('éaab', lambda k: None if k == 97 else k) == ('éb', [233, 97, 97, 98, 98])
assert calls('éaabaa', lambda k: None if k == 97 else 'xy') == ('xyxy', [233, 97, 97, 98, 98, 97, 97])

# exact-dict hash collision: LookupError from __eq__ is suppressed like any other lookup,
# while non-LookupError exceptions still propagate.
class CollidingLookupError:
    def __hash__(self):
        return 97

    def __eq__(self, other):
        raise IndexError('miss')


class CollidingRuntimeError:
    def __hash__(self):
        return 97

    def __eq__(self, other):
        raise RuntimeError('boom')


assert 'a'.translate({CollidingLookupError(): 'X'}) == 'a'
assert error(lambda: 'a'.translate({CollidingRuntimeError(): 'X'})) == 'RuntimeError: boom'

# === str.translate errors ===
assert error(lambda: 'a'.translate({97: 1.5})) == 'TypeError: character mapping must return integer, None or str'
assert error(lambda: 'a'.translate({97: b'x'})) == 'TypeError: character mapping must return integer, None or str'
assert error(lambda: 'é'.translate({233: [1]})) == 'TypeError: character mapping must return integer, None or str'
assert error(lambda: 'a'.translate({97: 0x110000})) == 'ValueError: character mapping must be in range(0x110000)'
assert error(lambda: 'a'.translate({97: -1})) == 'ValueError: character mapping must be in range(0x110000)'
assert error(lambda: 'a'.translate({97: 2**70})) == 'ValueError: character mapping must be in range(0x110000)'
assert error(lambda: 'a'.translate({97: -(2**70)})) == 'ValueError: character mapping must be in range(0x110000)'
assert error(lambda: 'é'.translate({233: 0x110000})) == 'ValueError: character mapping must be in range(0x110000)'
# errors are raised even after a deleted character
assert error(lambda: 'ab'.translate({97: None, 98: 1.5})) == (
    'TypeError: character mapping must return integer, None or str'
)
assert error(lambda: 'a'.translate(5)) == "TypeError: 'int' object is not subscriptable"
assert error(lambda: 'a'.translate(None)) == "TypeError: 'NoneType' object is not subscriptable"
assert error(lambda: 'a'.translate({1, 2})) == "TypeError: 'set' object is not subscriptable"
assert error(lambda: 'a'.translate()) == 'TypeError: str.translate() takes exactly one argument (0 given)'
assert error(lambda: 'a'.translate({}, {})) == 'TypeError: str.translate() takes exactly one argument (2 given)'
assert error(lambda: 'a'.translate(table={})) == 'TypeError: str.translate() takes no keyword arguments'
