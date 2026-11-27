# Tests for the re (regular expression) module - Match object

import re

# === Match .string attribute ===
subject = 'say ' + 'hello'  # concatenate so it isn't interned
m = re.search('hello', subject)
assert m is not None
assert m.string == 'say hello'
assert m.string is subject

# === Match truthiness ===
m = re.search(r'\d+', '123')
assert m

# === Match repr ===
m = re.search(r'\d+', 'abc 42 def')
assert repr(m) == "<re.Match object; span=(4, 6), match='42'>"

# === Object basic ===
assert bool(re.search(r'\w+', 'hello'))
assert isinstance(re.search(r'\w+', 'hello'), re.Match)
assert str(type(re.search(r'\w+', 'hello'))) == "<class 're.Match'>"

# === Match equality - Match objects are not comparable ===
m1 = re.search(r'\w+', 'hello')
m2 = re.search(r'\w+', 'hello')
assert (m1 == m2) == False
assert m1 != m2

# === Match methods are reusable on same object ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None
assert m.group(0) == 'user@host'
assert m.group(0) == 'user@host'
assert m.group(1) == 'user'
assert m.start(1) == 0
assert m.end(1) == 4
assert m.span(0) == (0, 9)

# === .string attribute is accessible multiple times ===
m = re.search(r'hello', 'say hello world')
assert m is not None
assert m.string == 'say hello world'
assert m.string == 'say hello world'

# === Match object with empty string ===
m = re.search(r'', 'hello')
assert m is not None
assert m.string == 'hello'
assert m.group(0) == ''

# === Match object from match() function ===
m = re.match(r'(\w+)', 'hello world')
assert m is not None
assert m.group(0) == 'hello'
assert m.start(0) == 0
assert m.string == 'hello world'

# === Match object from fullmatch() function ===
m = re.fullmatch(r'\w+', 'hello')
assert m is not None
assert m.group(0) == 'hello'
assert m.start(0) == 0
assert m.end(0) == 5

# === Match repr basic format ===
m = re.search(r'\d+', 'abc 42 def')
assert repr(m) == "<re.Match object; span=(4, 6), match='42'>"

m = re.search(r'\w+', 'hello')
assert repr(m) == "<re.Match object; span=(0, 5), match='hello'>"

m = re.search(r'', 'hello')
assert repr(m) == "<re.Match object; span=(0, 0), match=''>"

# === Match repr with special characters ===
p = re.compile(r'a.b', re.DOTALL)
m = p.search('a\nb')
assert m is not None
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\nb'>"

m = re.search(r'a.b', 'a\tb')
assert m is not None
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\tb'>"

# backslash in matched text
m = re.search(r'a.b', 'a\\b')
assert m is not None
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\\\b'>"

# carriage return in matched text
p = re.compile(r'a.b', re.DOTALL)
m = p.search('a\rb')
assert m is not None
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\rb'>"

# single quote in matched text — repr switches to double quotes
m = re.search(r'a.b', "a'b")
assert m is not None
r = repr(m)
assert r == '<re.Match object; span=(0, 3), match="a\'b">'

# double quote in matched text — repr uses single quotes
m = re.search(r'a.b', 'a"b')
assert m is not None
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\"b'>"

# === Pattern repr ===
p = re.compile('hello')
assert repr(p) == "re.compile('hello')"

p = re.compile(r'\n\t')
assert repr(p) == "re.compile('\\\\n\\\\t')"

# === Bool as group index ===
m = re.search(r'(\w+)\s+(\w+)', 'hello world')
assert m is not None
assert m.group(True) == 'hello'
assert m.group(False) == 'hello world'
assert m.start(True) == 0
assert m.end(True) == 5
assert m.span(True) == (0, 5)
assert m.span(False) == (0, 11)

# === m[N] subscript access ===
m = re.search(r'(\w+)\s+(\w+)', 'hello world')
assert m is not None
assert m[0] == 'hello world'
assert m[1] == 'hello'
assert m[2] == 'world'

# subscript with named groups
m = re.search(r'(?P<first>\w+)\s+(?P<second>\w+)', 'hello world')
assert m is not None
assert m['first'] == 'hello'
assert m['second'] == 'world'
assert m[1] == 'hello'

# subscript with invalid index
try:
    m[99]
    assert False, 'out-of-range subscript should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Non-ASCII subjects: positions are character offsets, not bytes ===
subject = 'héllo wörld date: 2026-07-06 ünd'
m = re.search(r'\d{4}-\d{2}-\d{2}', subject)
assert m is not None
assert m.span() == (18, 28)
assert m.start() == 18
assert m.end() == 28
assert m.span() == (18, 28)
assert subject[m.start() : m.end()] == '2026-07-06'

# group spans and unmatched groups on a non-ASCII subject
m = re.search(r'(\w+)@(\w+)(!)?', 'çontact: müller@pydantic é')
assert m is not None
assert m.span() == (9, 24)
assert m.span(1) == (9, 15)
assert m.span(2) == (16, 24)
assert m.span(3) == (-1, -1)
assert m.start(2) == 16
assert m.end(2) == 24
assert repr(m) == "<re.Match object; span=(9, 24), match='müller@pydantic'>"

# === Bytes patterns and subjects ===
assert re.fullmatch('.', 'é').group() == 'é'
assert re.fullmatch(b'.', b'\xc3\xa9') is None
assert re.fullmatch(b'..', b'\xc3\xa9').group() == b'\xc3\xa9'
assert re.match(b'\xff', b'\xc3\xbf') is None
assert re.match(b'z|\xffz', b'\xffz').group() == b'\xffz'
assert re.fullmatch(b'\xff|\xffz', b'\xffz').group() == b'\xffz'
assert re.fullmatch(rb'[\x80-\xff]+', b'\x80\x90\xff').span() == (0, 3)
assert re.fullmatch(rb'(\377)\1', b'\xff\xff').group(1) == b'\xff'
assert re.search(rb'\b\w+\b', b'\xffABC\xe9').group() == b'ABC'
assert re.fullmatch(rb'\s+', b'\t\v\r ') is not None
assert re.fullmatch(rb'\s+', b'\xa0') is None
assert re.fullmatch(rb'\d', b'\xb2') is None
assert re.fullmatch(b'a\xe9', b'A\xe9', re.IGNORECASE) is not None
assert re.fullmatch(b'a\xe9', b'A\xc9', re.IGNORECASE) is None

p = re.compile(b'(?P<hi>[\x80-\xff])(?P<optional>!)?')
assert p.pattern == b'(?P<hi>[\x80-\xff])(?P<optional>!)?'
assert p.flags == 0
assert re.compile(p) is p
subject = bytes([97, 45, 255]) + b'rest'
m = p.search(subject)
assert m is not None
assert p.match(subject) is None
assert p.fullmatch(b'\xff!').groups() == (b'\xff', b'!')
assert m.string is subject
assert m.group() == b'\xff'
assert m.group('hi', 'optional') == (b'\xff', None)
assert m.groups() == (b'\xff', None)
assert m.groupdict() == {'hi': b'\xff', 'optional': None}
assert m.groupdict(b'other') == {'hi': b'\xff', 'optional': b'other'}
assert m.span() == (2, 3)
assert m.span(1) == (2, 3)
assert m.start(1) == 2
assert m.end(1) == 3
assert m.span(2) == (-1, -1)
assert repr(m) == "<re.Match object; span=(2, 3), match=b'\\xff'>"
assert repr(re.compile(b'\xff')) == "re.compile(b'\\xff')"

for call in (lambda: re.match(b'.', 'x'), lambda: re.compile(b'.').fullmatch('x')):
    try:
        call()
        assert False, 'expected TypeError for a string subject with a bytes pattern'
    except TypeError as e:
        assert str(e) == 'cannot use a bytes pattern on a string-like object'
for call in (lambda: re.match('.', b'x'), lambda: re.compile('.').search(b'x')):
    try:
        call()
        assert False, 'expected TypeError for a bytes subject with a string pattern'
    except TypeError as e:
        assert str(e) == 'cannot use a string pattern on a bytes-like object'
