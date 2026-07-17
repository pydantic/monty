# Tests for the re (regular expression) module - capture groups and grouping

import re

# === Capture groups ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None
assert m.group(0) == 'user@host'
assert m.group(1) == 'user'
assert m.group(2) == 'host'
assert m.groups() == ('user', 'host')

# === group start/end/span with capture groups ===
m = re.search(r'(\w+)@(\w+)', 'email: user@host here')
assert m is not None
assert m.start(0) == 7
assert m.end(0) == 16
assert m.start(1) == 7
assert m.end(1) == 11
assert m.span(1) == (7, 11)
assert m.start(2) == 12
assert m.end(2) == 16
assert m.span(2) == (12, 16)

# === re.findall() with one group ===
result = re.findall(r'(\d+)', 'a1 b22 c333')
assert result == ['1', '22', '333']

# === re.findall() with multiple groups ===
result = re.findall(r'(\w+)=(\w+)', 'a=1 b=2')
assert result == [('a', '1'), ('b', '2')]

# === No groups: groups() returns empty tuple ===
m = re.search(r'\d+', '42')
assert m is not None
assert m.groups() == ()

# === Backreferences ===
m = re.search(r'(\w+)\s+\1', 'hello hello')
assert m is not None
assert m.group(0) == 'hello hello'
assert m.group(1) == 'hello'

# === Invalid group index ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.group(2)
    assert False, 'Accessing invalid group index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'
try:
    m.group('foo')
    assert False, 'Accessing group with non-integer index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === re.sub() replacement with backreferences ===
result = re.sub(r'(\w+)=(\w+)', r'\2=\1', 'a=1 b=2')
assert result == '1=a 2=b'

# === Negative group index ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.group(-1)
    assert False, 'Negative group index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Out-of-range group index ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.group(999)
    assert False, 'Out-of-range group index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Non-integer group argument: float ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.group(1.5)
    assert False, 'Float group argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Non-integer group argument: string ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.group('1')
    assert False, 'String group argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Non-integer group argument: None ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.group(None)
    assert False, 'None group argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Negative group index for start() ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None
try:
    m.start(-1)
    assert False, 'Negative start index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Out-of-range group index for start() ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None
try:
    m.start(999)
    assert False, 'Out-of-range start index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Non-integer argument for start() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.start(1.5)
    assert False, 'Float start argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Negative group index for end() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.end(-2)
    assert False, 'Negative end index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Out-of-range group index for end() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.end(100)
    assert False, 'Out-of-range end index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Non-integer argument for end() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.end('0')
    assert False, 'String end argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Negative group index for span() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.span(-1)
    assert False, 'Negative span index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Out-of-range group index for span() ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None
try:
    m.span(5)
    assert False, 'Out-of-range span index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Non-integer argument for span() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None
try:
    m.span(None)
    assert False, 'None span argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === Accessing unmatched optional group returns None ===
# Optional groups that don't match return None instead of raising an error
m = re.search(r'(\w+)?@(\w+)', '@host')
assert m is not None
assert m.group(1) is None
assert m.start(1) == -1
assert m.end(1) == -1
assert m.span(1) == (-1, -1)

# === Named group access with m.group('name') ===
m = re.search(r'(?P<first>\w+)\s+(?P<second>\w+)', 'hello world')
assert m is not None
assert m.group('first') == 'hello'
assert m.group('second') == 'world'
assert m.group(1) == 'hello'
assert m.group(2) == 'world'
assert m.group(0) == 'hello world'

# Named group with invalid name
try:
    m.group('nonexistent')
    assert False, 'non-existent named group should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group'

# === m.group() with multiple arguments ===
m = re.search(r'(\w+)\s+(\w+)\s+(\w+)', 'a b c')
assert m is not None
result = m.group(1, 2)
assert result == ('a', 'b')

result = m.group(1, 2, 3)
assert result == ('a', 'b', 'c')

result = m.group(0, 1)
assert result == ('a b c', 'a')

# === m.groupdict() ===
m = re.search(r'(?P<first>\w+)\s+(?P<second>\w+)', 'hello world')
assert m is not None
d = m.groupdict()
assert d == {'first': 'hello', 'second': 'world'}

# groupdict with no named groups
m = re.search(r'(\w+)\s+(\w+)', 'hello world')
assert m is not None
d = m.groupdict()
assert d == {}

# groupdict with unmatched optional named group
m = re.search(r'(?P<first>\w+)?@(?P<second>\w+)', '@host')
assert m is not None
d = m.groupdict()
assert d == {'first': None, 'second': 'host'}

# groupdict with default keyword argument
m = re.search(r'(?P<first>\w+)?@(?P<second>\w+)', '@host')
assert m is not None
d = m.groupdict(default='N/A')
assert d == {'first': 'N/A', 'second': 'host'}

# groupdict error cases
try:
    m.groupdict(wrong='N/A')
    assert False, 'groupdict wrong kwarg should raise'
except TypeError as e:
    assert str(e) == "groupdict() got an unexpected keyword argument 'wrong'", f'wrong: {e}'

try:
    m.groupdict('N/A', default='N/A')
    assert False, 'groupdict pos + kwarg should raise'
except TypeError as e:
    assert str(e) == 'groupdict() takes at most 1 argument (2 given)', f'dup: {e}'
