# Slicing creates runtime strings rather than interned literals.
import hashlib

s = 'abc!'[:-1]
u = 'é!'[:-1]
assert str(s) == 'abc'
assert repr(s) == "'abc'"
assert len(s) == 3 and bool(s)
assert ord(u) == 233
for invalid in [s, u + u]:
    try:
        ord(invalid)
        assert False
    except TypeError:
        pass
assert s == 'abc' and 'abc' == s
assert s < 'abd' and 'abb' < s
assert s < 'abcdefghijklmnop'
assert 'abcdefghijklmnop' > s
assert s + 'd' == 'abcd'
assert 'd' + s == 'dabc'
assert s + s == 'abcabc'
assert s * 2 == 'abcabc' and 2 * s == 'abcabc'
assert s * 0 == '' and -1 * s == ''
assert s * 4 == 'abcabcabcabc'
assert s[1] == 'b' and s[-1] == 'c'
assert s[1:] == 'bc' and s[::-1] == 'cba'
assert list(s) == ['a', 'b', 'c']
assert list(reversed(s)) == ['c', 'b', 'a']
assert list(u + u) == ['é', 'é']
assert s in 'xabcx' and 'bc!'[:-1] in s
assert s.startswith('ab!'[:-1])
assert s.endswith(('zz!'[:-1], 'bc!'[:-1]))
assert s.removeprefix('ab!'[:-1]) == 'c'
assert s.removesuffix('bc!'[:-1]) == 'a'
assert '--!'[:-1].join([s, u]) == 'abc--é'
assert hash(s) == hash('abc')
assert {s: 1}['abc'] == 1 and s in {'abc'}
assert s.encode('utf-8!'[:-1]) == b'abc'
assert b'abc'.decode('utf-8!'[:-1]) == s
assert bytes.fromhex('6162!'[:-1]) == b'ab'
assert b'ab'.hex('::!'[:1]) == '61:62'
for operation in [lambda: b'abc'.find(s), lambda: b'abc'.startswith((s,)), lambda: str(s, 'utf-8')]:
    try:
        operation()
        assert False
    except TypeError:
        pass
assert eval('1+2!'[:-1]) == 3
exec('x=42!'[:-1])
assert x == 42

heap_text = 'abcdefghijklmnop!'[:-1]
assert s < heap_text and heap_text > s
assert s + heap_text == 'abcabcdefghijklmnop'
assert heap_text + s == 'abcdefghijklmnopabc'
assert s.startswith((s,)) and not s.endswith(('xy!'[:-1],))
for operation in [
    lambda: s.startswith((42,)),
    lambda: s.endswith(42),
    lambda: s.join([s, 1]),
    lambda: next(s),
    lambda: s + 1,
    lambda: ord(1),
]:
    try:
        operation()
        assert False
    except TypeError:
        pass
assert s < 'abd!'[:-1]
assert '%s!'[:-1] % s == 'abc'
assert int('123!'[:-1]) == 123
assert int('ff!'[:-1], 16) == 255
assert float('1.25!'[:-1]) == 1.25
assert isinstance(id(s), int)
assert str(ValueError(s)) == 'abc'
try:
    hashlib.sha256(s)
    assert False
except TypeError as exc:
    assert 'Strings must be encoded before hashing' in str(exc)
try:
    str(s, 'utf-8')
    assert False
except TypeError as exc:
    assert 'decoding str is not supported' in str(exc)
try:
    b'ab'.hex('é!'[:-1])
    assert False
except ValueError:
    pass
for operation in [lambda: b'abc'.startswith(s), lambda: s * 1.5, lambda: 1.5 * s]:
    try:
        operation()
        assert False
    except TypeError:
        pass

# Exercise the full inline payload and the transition to heap storage.
for text in ['abcdefghijklmno', 'ééééééabc']:
    inline = (text + '!')[:-1]
    assert len(inline.encode()) == 15
    assert inline == text and hash(inline) == hash(text)
    assert {inline: 1}[text] == 1
    assert list(inline) == list(text)
    assert inline[::-1] == text[::-1]
    assert inline.upper().strip() == text.upper()
    longer = inline + 'x'
    assert len(longer.encode()) == 16
    assert longer[:-1] == inline

# Attribute errors and formatting must also handle computed inline text.
raises_attribute_error = False
try:
    s.method_that_does_not_exist()
except AttributeError:
    raises_attribute_error = True
assert raises_attribute_error
assert format(s, '>5!'[:-1]) == '  abc'
assert f'{s:>{2 + 3}}' == '  abc'
assert f'{s!s:>5}/{s!r}/{s!a}' == "  abc/'abc'/'abc'"
assert f'{u:>3}' == '  é'


# An index conversion can re-enter Python while the inline receiver is retained.
class Index:
    def __index__(self):
        assert s.upper() == 'ABC'
        return 1


assert s[Index()] == 'b'
assert s[Index() :] == 'bc'

for text in ['abc', 'éabc', 'abcdefghijklmnop', 'ééééééabcdefgh']:
    iterator = iter((text + '!')[:-1])
    assert next(iterator) == text[0]
    assert list(iterator) == list(text[1:])
