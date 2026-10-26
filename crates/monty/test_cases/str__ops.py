# === String concatenation (+) ===
assert 'hello' + ' ' + 'world' == 'hello world'
assert '' + 'test' == 'test'
assert 'test' + '' == 'test'
assert '' + '' == ''
assert 'a' + 'b' + 'c' + 'd' == 'abcd'

# === Interned and computed operands ===
# Joining forces runtime allocation even when the text matches a literal.
for left in ['café', ''.join(['ca', 'fé'])]:
    for right in ['日本', ''.join(['日', '本'])]:
        assert left + right == 'café日本'
    assert left + left == 'cafécafé'
    assert left + '' == 'café'
    assert '' + left == 'café'
    assert left[0] == 'c'
    assert left[-1] == 'é'
    assert left[True] == 'a'
    assert left[1:3] == 'af'
    assert left[::-1] == 'éfac'
    assert left[::2] == 'cf'
    assert left[20:] == ''
    assert len(left) == 4
    assert bool(left) is True
    assert bool(left[20:]) is False
    assert repr(left) == "'café'"
    assert repr([left, left]) == "['café', 'café']"
    assert '' in left
    assert left in left
    assert '日本' not in left
    for probe in ['fé', ''.join(['f', 'é'])]:
        assert probe in left
    for count, expected in [(-2, ''), (0, ''), (False, ''), (1, 'café'), (True, 'café'), (2, 'cafécafé')]:
        assert left * count == expected
        assert count * left == expected
        repeated = left
        repeated *= count
        assert repeated == expected
    try:
        1 in left
        assert False, 'expected a non-string probe to raise'
    except TypeError as exc:
        assert str(exc) == "'in <string>' requires string as left operand, not int"
    try:
        left * 2**100
        assert False, 'expected an oversized repetition count to raise'
    except OverflowError as exc:
        assert str(exc) == "cannot fit 'int' into an index-sized integer"
    try:
        2**100 * left
        assert False, 'expected an oversized reflected repetition count to raise'
    except OverflowError as exc:
        assert str(exc) == "cannot fit 'int' into an index-sized integer"
    try:
        left[4]
        assert False, 'expected string index out of range'
    except IndexError as exc:
        assert str(exc) == 'string index out of range'
    try:
        left[::0]
        assert False, 'expected a zero slice step to raise'
    except ValueError as exc:
        assert str(exc) == 'slice step cannot be zero'

# === Augmented assignment (+=) ===
s = 'hello'
s += ' world'
assert s == 'hello world'

s = 'test'
s += ''
assert s == 'test'

s = 'a'
s += 'b'
s += 'c'
assert s == 'abc'

s = 'ab'
s += s
assert s == 'abab'

# === String length ===
assert len('') == 0
assert len('a') == 1
assert len('hello') == 5
assert len('hello world') == 11
assert len('caf\xe9') == 4

# === String repr/str ===
assert repr('') == "''"
assert str('') == ''

assert repr('hello') == "'hello'"
assert str('hello') == 'hello'

assert repr('hello "world"') == '\'hello "world"\''
assert str('hello "world"') == 'hello "world"'

# === String repetition (*) ===
assert 'ab' * 3 == 'ababab'
assert 3 * 'ab' == 'ababab'
assert 'x' * 0 == ''
assert 'x' * -1 == ''
assert '' * 5 == ''
assert 'a' * 1 == 'a'

# === String repetition augmented assignment (*=) ===
s = 'ab'
s *= 3
assert s == 'ababab'

s = 'x'
s *= 0
assert s == ''

# === String join method ===
# Basic join on literals
assert ','.join(['a', 'b', 'c']) == 'a,b,c'
assert ''.join(['a', 'b', 'c']) == 'abc'
assert '-'.join([]) == ''
assert ','.join(['only']) == 'only'

# Empty results reuse Python's canonical empty string.
empty = ''
assert ''.join([]) is empty, 'empty join result should be canonical'
assert ''.strip() is empty, 'empty strip result should be canonical'
assert ''[:] is empty, 'empty slice result should be canonical'
assert ''.lower() is empty, 'empty transformation result should be canonical'
assert 'x' * 0 is empty, 'empty repetition result should be canonical'
assert '' + '' is empty, 'empty concatenation result should be canonical'

# Join with different iterables
assert ' '.join(('hello', 'world')) == 'hello world'

# Join with string iterable (iterates over characters)
assert ','.join('abc') == 'a,b,c'

# Join with variable separator
sep = '-'
assert sep.join(['a', 'b']) == 'a-b'

# Heap-allocated string separator
s = str('.')
assert s.join(['a', 'b']) == 'a.b'

# Mixed string types in iterable (interned and heap)
mixed = ['hello', str('world')]
assert ' '.join(mixed) == 'hello world'

# === String indexing (getitem) ===
# Basic indexing
assert 'hello'[0] == 'h'
assert 'hello'[1] == 'e'
assert 'hello'[4] == 'o'

# Negative indexing
assert 'hello'[-1] == 'o'
assert 'hello'[-2] == 'l'
assert 'hello'[-5] == 'h'

# Single character strings
assert 'a'[0] == 'a'
assert 'a'[-1] == 'a'

# Unicode strings
s = 'café'
assert s[0] == 'c'
assert s[1] == 'a'
assert s[2] == 'f'
assert s[3] == 'é'
assert s[-1] == 'é'

# Multi-byte unicode (CJK characters)
s = '日本語'
assert s[0] == '日'
assert s[1] == '本'
assert s[2] == '語'
assert s[-1] == '語'

# Emoji (multi-byte UTF-8)
s = 'a🎉b'
assert s[0] == 'a'
assert s[1] == '🎉'
assert s[2] == 'b'

# Heap-allocated strings
s = str('hello')
assert s[0] == 'h'
assert s[-1] == 'o'

# Variable index
s = 'abc'
i = 1
assert s[i] == 'b'

# Bool indices (True=1, False=0)
s = 'abc'
assert s[False] == 'a'
assert s[True] == 'b'

# === Computed heap strings: ASCII and non-ASCII ===
# ASCII heap strings index by byte; non-ASCII ones must still count code points
a = ''.join(['ab', 'cd', 'ef'])
assert len(a) == 6
assert a[0] == 'a'
assert a[5] == 'f'
assert a[-6] == 'a'
assert a[1:4] == 'bcd'
assert a[::2] == 'ace'
assert a[::-1] == 'fedcba'
assert a[4:1:-2] == 'ec'
assert a[-100:100] == 'abcdef'
assert a[5:2] == ''
assert a.find('cd', 1) == 2
assert a.find('cd', 3) == -1
assert a.find('ef', 1, 6) == 4
assert a.rfind('b', -10, 100) == 1
assert a.index('e', 2) == 4
assert a.count('c', 1, 3) == 1

u = ''.join(['a\xe9', '日b', '🎉c'])
assert len(u) == 6
assert u[1] == '\xe9'
assert u[2] == '日'
assert u[4] == '🎉'
assert u[-1] == 'c'
assert u[1:5] == '\xe9日b🎉'
assert u[::-1] == 'c🎉b日\xe9a'
assert u[::2] == 'a日🎉'
assert u.find('b', 1) == 3
assert u.find('c', 2, 6) == 5
assert u.rfind('\xe9', 0, 2) == 1
assert u.index('🎉', 2) == 4

# The same text is ASCII or not depending on the character appended
base = 'x' * 50
assert len(base + 'y') == 51
assert len(base + '\xe9') == 51
assert (base + '\xe9')[50] == '\xe9'
assert (base + '\xe9')[-2] == 'x'

# === Sorting and comparisons ===
assert 'a' < 'b'
assert 'b' > 'a'
assert 'a' <= 'a'
assert 'a' <= 'b'
assert 'b' >= 'b'
assert 'b' >= 'a'
assert not ('b' < 'a'), 'str not < str'
assert not ('a' > 'b'), 'str not > str'

# Different lengths
assert 'a' < 'aa'
assert 'ab' < 'b'
assert '' < 'a'
assert 'abc' > 'ab'

# Non-ASCII comparisons (by Unicode code point)
assert 'café' < 'cafë'
assert 'z' < 'é'
assert '日' < '本'
assert '😀' < '😁'

# Sorting
assert sorted('cba') == ['a', 'b', 'c']
assert sorted(['b', 'c', 'a']) == ['a', 'b', 'c']
assert sorted(['café', 'cafë', 'cafa']) == ['cafa', 'café', 'cafë']
assert sorted(['bb', 'a', 'ba']) == ['a', 'ba', 'bb']

# === str() constructor with keyword argument ===
assert str(object=42) == '42'
assert str(object='hello') == 'hello'
assert str(object=True) == 'True'
assert str(object=[1, 2]) == '[1, 2]'
assert str(object=None) == 'None'

# str() constructor error cases
try:
    str(wrong=42)
    assert False, 'str wrong kwarg should raise'
except TypeError as e:
    assert str(e) == "str() got an unexpected keyword argument 'wrong'", f'wrong: {e}'

try:
    str(42, object=42)
    assert False, 'str pos + kwarg should raise'
except TypeError as e:
    assert str(e) == "argument for str() given by name ('object') and position (1)", f'dup: {e}'

# === str() decoding bytes with encoding/errors ===
assert str(b'x', 'utf-8') == 'x'
assert str(b'x', encoding='utf-8') == 'x'
assert str(object=b'x', encoding='utf-8') == 'x'
assert str(b'\xe2\x82\xac', 'utf-8') == '€'
assert str(b'\xff', 'utf-8', 'replace') == '�'
assert str(b'\xff', errors='replace') == '�'
assert str(b'x', errors='strict') == 'x'
# a missing object wins over the decoding args
assert str(encoding='utf-8') == ''
assert str(errors='strict') == ''
# without decoding args, bytes stringify to their repr
assert str(b'x') == "b'x'"

try:
    str(b'\xff', 'utf-8')
    assert False, 'expected UnicodeDecodeError'
except UnicodeDecodeError as e:
    assert str(e) == "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"

try:
    str('x', 'utf-8')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'decoding str is not supported'

try:
    str('x', errors='strict')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'decoding str is not supported'

try:
    str(1, 'utf-8')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'decoding to str: need a bytes-like object, int found'

try:
    str(None, 'utf-8')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'decoding to str: need a bytes-like object, NoneType found'

# encoding/errors are named with the wrong value's type name (NoneType, not
# the bad-arg 'None' special case used by bytes())
try:
    str(b'x', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "str() argument 'encoding' must be str, not int"

try:
    str(b'x', None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "str() argument 'encoding' must be str, not NoneType"

try:
    str(b'x', 'utf-8', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "str() argument 'errors' must be str, not int"

try:
    str(b'x', 'bogus')
    assert False, 'expected LookupError'
except LookupError as e:
    assert str(e) == 'unknown encoding: bogus'

# no kwargs: unicode_vectorcall's un-parenthesised arity wording
try:
    str(b'x', 'utf-8', 'strict', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'str expected at most 3 arguments, got 4'

# with kwargs: the clinic parser's parenthesised total-count wording
try:
    str(b'x', 'utf-8', 'strict', 'x', bogus=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'str() takes at most 3 arguments (5 given)'

try:
    str(b'x', 'utf-8', encoding='q')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "argument for str() given by name ('encoding') and position (2)"

# === `in` / `not in` ===
assert 'bc' in 'abc'
assert 'abc' in 'abc'
assert '' in 'abc'
assert 'x' not in 'abc'
# Only a str is a valid probe; the error names the offending type.
try:
    1 in 'abc'
    assert False, 'expected TypeError for a non-str probe'
except TypeError as exc:
    assert str(exc) == "'in <string>' requires string as left operand, not int"
