# Tests for the re (regular expression) module

import re

# === Constant ===
assert re.NOFLAG == 0, 're.NOFLAG == 0'
assert re.I == re.IGNORECASE == 2, 're.I == re.IGNORECASE == 2'
assert re.M == re.MULTILINE == 8, 're.M == re.MULTILINE == 8'
assert re.S == re.DOTALL == 16, 're.S == re.DOTALL == 16'

# === re.search() basic ===
m = re.search('hello', 'say hello world')
assert m is not None, 're.search finds a match'
assert m.group() == 'hello', 're.search group(0) returns matched text'
assert m.group(0) == 'hello', 're.search group(0) explicit returns matched text'
assert m.start() == 4, 're.search start() returns start position'
assert m.end() == 9, 're.search end() returns end position'
assert m.span() == (4, 9), 're.search span() returns (start, end) tuple'

# === re.search() with no match ===
m = re.search('xyz', 'hello world')
assert m is None, 're.search returns None when no match'

# === re.search() with error ===
try:
    re.search('(', 'test')
    assert False, 're.search with invalid pattern should raise error'
except re.PatternError as e:
    # The error message may vary based on the regex engine, but it should not be empty
    assert len(str(e)) > 0, 're.search with invalid pattern raises PatternError with message'

# === re.match() ===
m = re.match('hello', 'hello world')
assert m is not None, 're.match matches at start'
assert m.group() == 'hello', 're.match group returns matched text'

m = re.match('world', 'hello world')
assert m is None, 're.match does not match in the middle'

# === re.fullmatch() ===
m = re.fullmatch('hello', 'hello')
assert m is not None, 're.fullmatch matches exact string'
assert m.group() == 'hello', 're.fullmatch group returns full match'

m = re.fullmatch('hello', 'hello world')
assert m is None, 're.fullmatch does not match partial string'

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

# === Match .string attribute ===
m = re.search('hello', 'say hello')
assert m is not None, 'search finds match for .string test'
assert m.string == 'say hello', '.string returns the input string'

# === re.findall() with no groups ===
result = re.findall(r'\d+', 'a1 b22 c333')
assert result == ['1', '22', '333'], 'findall without groups returns list of matches'

# === re.findall() with one group ===
result = re.findall(r'(\d+)', 'a1 b22 c333')
assert result == ['1', '22', '333'], 'findall with one group returns list of group strings'

# === re.findall() with multiple groups ===
result = re.findall(r'(\w+)=(\w+)', 'a=1 b=2')
assert result == [('a', '1'), ('b', '2')], 'findall with multiple groups returns list of tuples'

# === re.findall() with no match ===
result = re.findall(r'\d+', 'no numbers')
assert result == [], 'findall with no match returns empty list'

# === re.sub() ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3')
assert result == 'aX bX cX', 're.sub replaces all matches'

# === re.sub() with count ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', 1)
assert result == 'aX b2 c3', 're.sub with count=1 replaces only first'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 2)
assert result == 'aX bX c3', 're.sub with count=2 replaces first two'

# === re.compile() ===
pattern = re.compile(r'\d+')
m = pattern.search('abc 123 def')
assert m is not None, 'compiled pattern search finds match'
assert m.group() == '123', 'compiled pattern match returns correct group'

m = pattern.match('123 abc')
assert m is not None, 'compiled pattern match at start'
assert m.group() == '123', 'compiled pattern match group'

m = pattern.match('abc 123')
assert m is None, 'compiled pattern match does not match in middle'

# === compiled pattern fullmatch ===
pattern = re.compile(r'\d+')
m = pattern.fullmatch('123')
assert m is not None, 'compiled pattern fullmatch on exact string'
assert m.group() == '123', 'compiled pattern fullmatch group'

m = pattern.fullmatch('123abc')
assert m is None, 'compiled pattern fullmatch rejects partial match'

# === compiled pattern findall ===
pattern = re.compile(r'\d+')
result = pattern.findall('a1 b2 c3')
assert result == ['1', '2', '3'], 'compiled pattern findall'

# === compiled pattern sub ===
pattern = re.compile(r'\d+')
result = pattern.sub('X', 'a1 b2 c3')
assert result == 'aX bX cX', 'compiled pattern sub'

result = pattern.sub('X', 'a1 b2 c3', 1)
assert result == 'aX b2 c3', 'compiled pattern sub with count'

# === Flags: IGNORECASE ===
pattern = re.compile(r'hello', re.IGNORECASE)
m = pattern.search('Hello World')
assert m is not None, 'IGNORECASE flag works'
assert m.group() == 'Hello', 'IGNORECASE matches case-insensitively'

# === Flags: DOTALL ===
pattern = re.compile(r'a.b', re.DOTALL)
m = pattern.search('a\nb')
assert m is not None, 'DOTALL flag allows dot to match newline'
assert m.group() == 'a\nb', 'DOTALL matches newline with dot'

# === Flags: MULTILINE ===
pattern = re.compile(r'^\w+', re.MULTILINE)
result = pattern.findall('hello\nworld')
assert result == ['hello', 'world'], 'MULTILINE allows ^ to match at line boundaries'

# === Pattern attributes ===
pattern = re.compile(r'\d+', re.IGNORECASE)
assert pattern.pattern == r'\d+', '.pattern returns the pattern string'
# CPython flags include re.UNICODE (32) by default, so we check flags & 2 instead
assert pattern.flags & re.IGNORECASE, '.flags includes IGNORECASE'

# === Match truthiness ===
m = re.search(r'\d+', '123')
assert m, 'Match objects are truthy'

# === Pattern repr ===
p = re.compile(r'\d+')
assert repr(p) == r"re.compile('\\d+')", 'Pattern repr without flags'

p = re.compile(r'\d+', re.IGNORECASE)
assert repr(p) == r"re.compile('\\d+', re.IGNORECASE)", 'Pattern repr with IGNORECASE'

# === Match repr ===
m = re.search(r'\d+', 'abc 42 def')
assert repr(m) == "<re.Match object; span=(4, 6), match='42'>", 'Match repr'

# === Flag constants ===
assert re.IGNORECASE == 2, 'IGNORECASE flag value'
assert re.MULTILINE == 8, 'MULTILINE flag value'
assert re.DOTALL == 16, 'DOTALL flag value'

# === Combined flags ===
pattern = re.compile(r'^hello', re.IGNORECASE | re.MULTILINE)
result = pattern.findall('Hello\nhello\nHELLO')
assert result == ['Hello', 'hello', 'HELLO'], 'Combined IGNORECASE | MULTILINE flags'

# === More MULTILINE tests ===
# Without MULTILINE, ^ matches only start of string
pattern = re.compile(r'^\w+')
result = pattern.findall('line1\nline2\nline3')
assert result == ['line1'], 'Without MULTILINE, ^ matches only start of string'

# With MULTILINE, ^ matches each line start
pattern = re.compile(r'^\w+', re.MULTILINE)
result = pattern.findall('line1\nline2\nline3')
assert result == ['line1', 'line2', 'line3'], 'With MULTILINE, ^ matches each line start'

# Without MULTILINE, $ matches only end of string
pattern = re.compile(r'\w+$')
result = pattern.findall('line1\nline2\nline3')
assert result == ['line3'], 'Without MULTILINE, $ matches only end of string'

# With MULTILINE, $ matches each line end
pattern = re.compile(r'\w+$', re.MULTILINE)
result = pattern.findall('line1\nline2\nline3')
assert result == ['line1', 'line2', 'line3'], 'With MULTILINE, $ matches each line end'

# === More DOTALL tests ===
# Without DOTALL, . does not match newline
pattern = re.compile(r'a.b')
m = pattern.search('a\nb')
assert m is None, 'Without DOTALL, . does not match newline'

# With DOTALL, . matches newline
pattern = re.compile(r'a.b', re.DOTALL)
m = pattern.search('a\nb')
assert m is not None, 'With DOTALL, . matches newline'
assert m.group() == 'a\nb', 'DOTALL allows . to match newline'

# DOTALL with multiple newlines
pattern = re.compile(r'start.*end', re.DOTALL)
m = pattern.search('start\nline1\nline2\nend')
assert m is not None, 'DOTALL .* matches multiple newlines'
assert m.group() == 'start\nline1\nline2\nend', 'DOTALL .* captures everything including newlines'

# === Pattern repr with multiple flags (I, M, D order) ===
p = re.compile(r'test', re.IGNORECASE)
assert repr(p) == r"re.compile('test', re.IGNORECASE)", 'Pattern repr with I flag'

p = re.compile(r'test', re.MULTILINE)
assert repr(p) == r"re.compile('test', re.MULTILINE)", 'Pattern repr with M flag'

p = re.compile(r'test', re.DOTALL)
assert repr(p) == r"re.compile('test', re.DOTALL)", 'Pattern repr with D flag'

p = re.compile(r'test', re.IGNORECASE | re.MULTILINE)
assert repr(p) == r"re.compile('test', re.IGNORECASE|re.MULTILINE)", 'Pattern repr with I|M flags'

p = re.compile(r'test', re.IGNORECASE | re.DOTALL)
assert repr(p) == r"re.compile('test', re.IGNORECASE|re.DOTALL)", 'Pattern repr with I|D flags'

p = re.compile(r'test', re.MULTILINE | re.DOTALL)
assert repr(p) == r"re.compile('test', re.MULTILINE|re.DOTALL)", 'Pattern repr with M|D flags'

p = re.compile(r'test', re.IGNORECASE | re.MULTILINE | re.DOTALL)
assert repr(p) == r"re.compile('test', re.IGNORECASE|re.MULTILINE|re.DOTALL)", 'Pattern repr with I|M|D flags'

# === Combined IGNORECASE and DOTALL ===
pattern = re.compile(r'Hello.*World', re.IGNORECASE | re.DOTALL)
m = pattern.search('HELLO\nmiddle\nWORLD')
assert m is not None, 'Combined IGNORECASE|DOTALL finds match'
assert m.group() == 'HELLO\nmiddle\nWORLD', 'IGNORECASE|DOTALL matches case-insensitively across newlines'

# === Combined MULTILINE and DOTALL ===
pattern = re.compile(r'^a.*b$', re.MULTILINE | re.DOTALL)
result = pattern.findall('a\nb\nc\nb')
assert result == ['a\nb\nc\nb'], 'Combined MULTILINE|DOTALL with ^ and $ and .'

# === All three flags combined ===
pattern = re.compile(r'^Hello.*World$', re.IGNORECASE | re.MULTILINE | re.DOTALL)
m = pattern.search('first\nHELLO\nsome\nlines\nWORLD\nlast')
assert m is not None, 'All three flags combined finds match'
assert m.group() == 'HELLO\nsome\nlines\nWORLD', 'I|M|D flags work together'

# === No groups: groups() returns empty tuple ===
m = re.search(r'\d+', '42')
assert m is not None, 'search with no groups finds match'
assert m.groups() == (), 'groups() with no capture groups returns empty tuple'

# === Empty pattern ===
m = re.search(r'', 'abc')
assert m is not None, 'search with empty pattern finds match'
assert m.start() == 0 and m.end() == 0, 'empty pattern matches at start of string'

# === Zero-length matches ===
m = re.search(r'a*', 'bc')
assert m is not None, 'search with zero-length match finds match'
assert m.group() == '', 'zero-length match returns empty string'

# === Object identity of compiled patterns ===
p1 = re.compile(r'\d+')
p2 = re.compile(r'\d+')
assert p1 == p2, 'separately compiled patterns with same pattern are equal'
match1 = p1.search('123')
match2 = p2.search('123')
assert match1 != match2, 'matches from different pattern objects are distinct'

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

# === Object basic ===
assert bool(re.compile(r'\d+'))
assert bool(re.search(r'\w+', 'hello'))
assert isinstance(re.compile(r'\d+'), re.Pattern), 're.compile returns re.Pattern instance'
assert isinstance(re.search(r'\w+', 'hello'), re.Match), 're.search returns re.Match instance'
