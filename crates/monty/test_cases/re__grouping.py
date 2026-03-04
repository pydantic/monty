# Tests for the re (regular expression) module - capture groups and grouping

import re

# === Capture groups ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None, 're.search with groups finds a match'
assert m.group(0) == 'user@host', 'group(0) is the full match'
assert m.group(1) == 'user', 'group(1) is first capture'
assert m.group(2) == 'host', 'group(2) is second capture'
assert m.groups() == ('user', 'host'), 'groups() returns tuple of captures'

# === group start/end/span with capture groups ===
m = re.search(r'(\w+)@(\w+)', 'email: user@host here')
assert m is not None, 'search with groups finds match'
assert m.start(0) == 7, 'start(0) is full match start'
assert m.end(0) == 16, 'end(0) is full match end'
assert m.start(1) == 7, 'start(1) is group 1 start'
assert m.end(1) == 11, 'end(1) is group 1 end'
assert m.span(1) == (7, 11), 'span(1) is group 1 span'
assert m.start(2) == 12, 'start(2) is group 2 start'
assert m.end(2) == 16, 'end(2) is group 2 end'
assert m.span(2) == (12, 16), 'span(2) is group 2 span'

# === re.findall() with one group ===
result = re.findall(r'(\d+)', 'a1 b22 c333')
assert result == ['1', '22', '333'], 'findall with one group returns list of group strings'

# === re.findall() with multiple groups ===
result = re.findall(r'(\w+)=(\w+)', 'a=1 b=2')
assert result == [('a', '1'), ('b', '2')], 'findall with multiple groups returns list of tuples'

# === No groups: groups() returns empty tuple ===
m = re.search(r'\d+', '42')
assert m is not None, 'search with no groups finds match'
assert m.groups() == (), 'groups() with no capture groups returns empty tuple'

# === Backreferences ===
m = re.search(r'(\w+)\s+\1', 'hello hello')
assert m is not None, 'backreference finds repeated word'
assert m.group(0) == 'hello hello', 'backreference full match'
assert m.group(1) == 'hello', 'backreference group'

# === Invalid group index ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
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
assert result == '1=a 2=b', 're.sub with backreferences swaps groups'

# === Negative group index ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.group(-1)
    assert False, 'Negative group index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'negative group raises IndexError with "no such group" message'

# === Out-of-range group index ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.group(999)
    assert False, 'Out-of-range group index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'out-of-range group raises IndexError with "no such group" message'

# === Non-integer group argument: float ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.group(1.5)
    assert False, 'Float group argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'float group arg raises IndexError'

# === Non-integer group argument: string ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.group('1')
    assert False, 'String group argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'string group arg raises IndexError'

# === Non-integer group argument: None ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.group(None)
    assert False, 'None group argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'None group arg raises IndexError'

# === Negative group index for start() ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None, 'search with groups finds match'
try:
    m.start(-1)
    assert False, 'Negative start index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'negative start raises IndexError with "no such group" message'

# === Out-of-range group index for start() ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None, 'search with groups finds match'
try:
    m.start(999)
    assert False, 'Out-of-range start index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'out-of-range start raises IndexError'

# === Non-integer argument for start() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.start(1.5)
    assert False, 'Float start argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'float start arg raises IndexError'

# === Negative group index for end() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.end(-2)
    assert False, 'Negative end index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'negative end raises IndexError with "no such group" message'

# === Out-of-range group index for end() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.end(100)
    assert False, 'Out-of-range end index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'out-of-range end raises IndexError'

# === Non-integer argument for end() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.end('0')
    assert False, 'String end argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'string end arg raises IndexError'

# === Negative group index for span() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.span(-1)
    assert False, 'Negative span index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'negative span raises IndexError with "no such group" message'

# === Out-of-range group index for span() ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None, 'search with groups finds match'
try:
    m.span(5)
    assert False, 'Out-of-range span index should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'out-of-range span raises IndexError'

# === Non-integer argument for span() ===
m = re.search(r'(\w+)', 'hello')
assert m is not None, 'search with group finds match'
try:
    m.span(None)
    assert False, 'None span argument should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'None span arg raises IndexError'

# === Accessing unmatched optional group returns None ===
# Optional groups that don't match return None instead of raising an error
m = re.search(r'(\w+)?@(\w+)', '@host')
assert m is not None, 'search with optional group finds match'
assert m.group(1) is None, 'unmatched optional group returns None'
assert m.start(1) == -1, 'start of unmatched optional group returns -1'
assert m.end(1) == -1, 'end of unmatched optional group returns -1'
assert m.span(1) == (-1, -1), 'span of unmatched optional group returns (-1, -1)'

# === Named group access with m.group('name') ===
m = re.search(r'(?P<first>\w+)\s+(?P<second>\w+)', 'hello world')
assert m is not None, 'named group search finds match'
assert m.group('first') == 'hello', "group('first') returns first named group"
assert m.group('second') == 'world', "group('second') returns second named group"
assert m.group(1) == 'hello', 'named group is also accessible by index'
assert m.group(2) == 'world', 'named group is also accessible by index'
assert m.group(0) == 'hello world', 'group(0) still returns full match'

# Named group with invalid name
try:
    m.group('nonexistent')
    assert False, 'non-existent named group should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'non-existent named group error message'

# === m.group() with multiple arguments ===
m = re.search(r'(\w+)\s+(\w+)\s+(\w+)', 'a b c')
assert m is not None, 'multi-group search finds match'
result = m.group(1, 2)
assert result == ('a', 'b'), 'group(1, 2) returns tuple of two groups'

result = m.group(1, 2, 3)
assert result == ('a', 'b', 'c'), 'group(1, 2, 3) returns tuple of three groups'

result = m.group(0, 1)
assert result == ('a b c', 'a'), 'group(0, 1) includes full match'

# === m.groupdict() ===
m = re.search(r'(?P<first>\w+)\s+(?P<second>\w+)', 'hello world')
assert m is not None, 'named group search for groupdict'
d = m.groupdict()
assert d == {'first': 'hello', 'second': 'world'}, 'groupdict returns correct dict'

# groupdict with no named groups
m = re.search(r'(\w+)\s+(\w+)', 'hello world')
assert m is not None, 'unnamed group search for groupdict'
d = m.groupdict()
assert d == {}, 'groupdict with no named groups returns empty dict'

# groupdict with unmatched optional named group
m = re.search(r'(?P<first>\w+)?@(?P<second>\w+)', '@host')
assert m is not None, 'optional named group search for groupdict'
d = m.groupdict()
assert d == {'first': None, 'second': 'host'}, 'groupdict includes unmatched named groups as None'
