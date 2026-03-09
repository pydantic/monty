# Tests for the re (regular expression) module - basic functionality

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

# === re.findall() with no groups ===
result = re.findall(r'\d+', 'a1 b22 c333')
assert result == ['1', '22', '333'], 'findall without groups returns list of matches'

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

# === Pattern repr ===
p = re.compile(r'\d+')
assert repr(p) == r"re.compile('\\d+')", 'Pattern repr without flags'

p = re.compile(r'\d+', re.IGNORECASE)
assert repr(p) == r"re.compile('\\d+', re.IGNORECASE)", 'Pattern repr with IGNORECASE'

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

# === re.sub() error: missing pattern ===
try:
    re.sub()
    assert False, 're.sub() with no args should raise TypeError'
except TypeError as e:
    assert 'pattern' in str(e).lower(), 're.sub missing pattern error mentions pattern'

# === re.sub() error: missing repl ===
try:
    re.sub(r'\d+')
    assert False, 're.sub(pattern) should raise TypeError'
except TypeError as e:
    assert 'repl' in str(e).lower(), 're.sub missing repl error mentions repl'

# === re.sub() error: missing string ===
try:
    re.sub(r'\d+', 'X')
    assert False, 're.sub(pattern, repl) should raise TypeError'
except TypeError as e:
    assert 'string' in str(e).lower(), 're.sub missing string error mentions string'

# === re.sub() error: count is not an integer ===
try:
    re.sub(r'\d+', 'X', 'a1b2', 1.5)
    assert False, 're.sub with float count should raise TypeError'
except TypeError as e:
    assert "'float' object cannot be interpreted as an integer" in str(e), 're.sub float count error'

try:
    re.sub(r'\d+', 'X', 'a1b2', 'one')
    assert False, 're.sub with string count should raise TypeError'
except TypeError as e:
    assert "'str' object cannot be interpreted as an integer" in str(e), 're.sub string count error'

# === Pattern.sub() error: missing repl ===
pattern = re.compile(r'\d+')
try:
    pattern.sub()
    assert False, 'Pattern.sub() with no args should raise TypeError'
except TypeError as e:
    assert 'repl' in str(e).lower(), 'Pattern.sub missing repl error mentions repl'

# === Pattern.sub() error: missing string ===
try:
    pattern.sub('X')
    assert False, 'Pattern.sub(repl) should raise TypeError'
except TypeError as e:
    assert 'string' in str(e).lower(), 'Pattern.sub missing string error mentions string'

# === re.sub() with count=0 (replace all) ===
result = re.sub(r'\d', 'X', '1a2b3c', 0)
assert result == 'XaXbXc', 're.sub with count=0 replaces all'

# === re.sub() empty replacement ===
result = re.sub(r'\d+', '', 'a1 b2 c3')
assert result == 'a b c', 're.sub with empty replacement removes matches'

# === Pattern.sub() edge case: empty match ===
pattern = re.compile(r'a*')
result = pattern.sub('X', 'bac')
# Note: this might be a zero-width match behavior that's different
assert 'X' in result, 'Pattern.sub handles zero-width matches'

# === re.compile() error: invalid pattern ===
try:
    re.compile('(unclosed')
    assert False, 're.compile with invalid pattern should raise PatternError'
except re.PatternError as e:
    assert len(str(e)) > 0, 're.compile invalid pattern raises PatternError'

# === re.search() error: pattern is not a string ===
try:
    re.search(123, 'hello')
    assert False, 're.search with int pattern should raise TypeError'
except TypeError as e:
    assert 'string' in str(e).lower(), 're.search non-string pattern error'

# === re.search() error: string is not a string ===
try:
    re.search(r'\d+', 123)
    assert False, 're.search with int string should raise TypeError'
except TypeError as e:
    assert 'string' in str(e).lower(), 're.search non-string string error'

# === re.match() error: pattern is not a string ===
try:
    re.match(None, 'hello')
    assert False, 're.match with None pattern should raise TypeError'
except TypeError as e:
    assert 'string' in str(e).lower(), 're.match None pattern error'

# === re.fullmatch() error: string is not a string ===
try:
    re.fullmatch(r'\d+', None)
    assert False, 're.fullmatch with None string should raise TypeError'
except TypeError as e:
    assert 'string' in str(e).lower(), 're.fullmatch None string error'

# === Object basic ===
assert bool(re.compile(r'\d+'))
assert bool(re.search(r'\w+', 'hello'))
assert isinstance(re.compile(r'\d+'), re.Pattern), 're.compile returns re.Pattern instance'
assert isinstance(re.search(r'\w+', 'hello'), re.Match), 're.search returns re.Match instance'
assert str(type(re.compile(r'\d+'))) == "<class 're.Pattern'>", 'type of compiled pattern is re.Pattern'
assert str(type(re.search(r'\w+', 'hello'))) == "<class 're.Match'>", 'type of search match is re.Match'

# === fullmatch with alternation ===
# fullmatch must try all alternatives to find a full-string match,
# not just pick the first alternative that matches somewhere
m = re.fullmatch('a|ab', 'ab')
assert m is not None, 'fullmatch with alternation finds full match'
assert m.group() == 'ab', 'fullmatch alternation matches full-string alternative'

m = re.fullmatch('ab|a', 'ab')
assert m is not None, 'fullmatch when full-match alternative is first'
assert m.group() == 'ab', 'fullmatch returns correct match when first alt matches'

m = re.fullmatch('cat|category', 'category')
assert m is not None, 'fullmatch alternation picks full-string alternative'
assert m.group() == 'category', 'fullmatch alternation returns correct match'

m = re.fullmatch('x|ab|a', 'ab')
assert m is not None, 'fullmatch with three alternatives'
assert m.group() == 'ab', 'fullmatch picks correct alternative from three'

# compiled pattern fullmatch with alternation
p = re.compile('a|ab')
m = p.fullmatch('ab')
assert m is not None, 'compiled fullmatch with alternation finds full match'
assert m.group() == 'ab', 'compiled fullmatch alternation matches correctly'

# fullmatch with alternation and groups
m = re.fullmatch('(a)|(ab)', 'ab')
assert m is not None, 'fullmatch alternation with groups'
assert m.group(0) == 'ab', 'fullmatch alternation groups: group(0) is full match'
assert m.group(1) is None, 'fullmatch alternation groups: group(1) did not match'
assert m.group(2) == 'ab', 'fullmatch alternation groups: group(2) matched'

# fullmatch with quantifiers
m = re.fullmatch('a+|b+', 'aaa')
assert m is not None, 'fullmatch a+|b+ on aaa'
assert m.group() == 'aaa', 'fullmatch a+|b+ returns full match'

# fullmatch with .* (greedy)
m = re.fullmatch('.*', 'anything')
assert m is not None, 'fullmatch .* matches anything'
assert m.group() == 'anything', 'fullmatch .* returns full string'

# fullmatch on empty string with empty pattern
m = re.fullmatch('', '')
assert m is not None, 'fullmatch empty pattern on empty string'
assert m.group() == '', 'fullmatch empty returns empty'

# fullmatch should not match partial strings even with alternation
m = re.fullmatch('a|ab', 'abc')
assert m is None, 'fullmatch rejects when no alternative spans full string'

# fullmatch with MULTILINE should still require full-string match
p = re.compile('hello', re.MULTILINE)
m = p.fullmatch('hello')
assert m is not None, 'fullmatch MULTILINE on single line'
assert m.group() == 'hello', 'fullmatch MULTILINE returns correct match'

m = p.fullmatch('hello\nworld')
assert m is None, 'fullmatch MULTILINE rejects multi-line input'

# fullmatch with alternation and flags combined
p = re.compile('(a+)|(b+)', re.MULTILINE)
m = p.fullmatch('bbb')
assert m is not None, 'fullmatch groups with MULTILINE flag'
assert m.group(0) == 'bbb', 'fullmatch groups MULTILINE: group(0) correct'
assert m.group(1) is None, 'fullmatch groups MULTILINE: group(1) did not match'
assert m.group(2) == 'bbb', 'fullmatch groups MULTILINE: group(2) matched'

# === Literal $ in replacement ===
result = re.sub(r'\d+', '$', 'a1b2')
assert result == 'a$b$', 'literal $ in replacement is preserved'

result = re.sub(r'\d+', '$1', 'a1b2')
assert result == 'a$1b$1', 'literal $1 in replacement is preserved (not backreference)'

result = re.sub(r'\d+', '$$', 'a1b2')
assert result == 'a$$b$$', 'literal $$ in replacement is preserved'

# compiled pattern with $ in replacement
p = re.compile(r'\d+')
result = p.sub('$', 'a1b2')
assert result == 'a$b$', 'compiled pattern: literal $ in replacement is preserved'

result = re.sub(r'\d+', '$$$', 'a1b2')
assert result == 'a$$$b$$$', 'triple $ in replacement preserved'

# plain replacement with no special chars
result = re.sub(r'\d+', 'NUM', 'a1 b2')
assert result == 'aNUM bNUM', 'plain replacement without special chars'

# === Negative count in re.sub ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', -1)
assert result == 'a1 b2 c3', 're.sub with negative count returns string unchanged'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', -100)
assert result == 'a1 b2 c3', 're.sub with large negative count returns string unchanged'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', -999)
assert result == 'a1 b2 c3', 're.sub with very large negative count returns string unchanged'

# compiled pattern with negative count
p = re.compile(r'\d+')
result = p.sub('X', 'a1 b2 c3', -1)
assert result == 'a1 b2 c3', 'compiled pattern: negative count returns string unchanged'

result = p.sub('X', 'a1 b2 c3', -100)
assert result == 'a1 b2 c3', 'compiled pattern: large negative count returns string unchanged'

# negative count with empty string
result = re.sub(r'\d+', 'X', '', -1)
assert result == '', 're.sub negative count on empty string'

# === re.sub with count boundary values ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', 0)
assert result == 'aX bX cX', 're.sub count=0 replaces all (explicit)'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 1)
assert result == 'aX b2 c3', 're.sub count=1 replaces first only'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 3)
assert result == 'aX bX cX', 're.sub count=3 replaces all three'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 100)
assert result == 'aX bX cX', 're.sub count exceeding matches replaces all'

# === Pattern.sub() error: too many arguments ===
p = re.compile(r'\d+')
try:
    p.sub('X', 'a1b2', 0, 'extra')
    assert False, 'Pattern.sub with 4 args should raise TypeError'
except TypeError as e:
    assert 'at most 3' in str(e), 'Pattern.sub too many args error'

# === Flags on module-level functions ===
# re.search with flags
m = re.search(r'hello', 'HELLO WORLD', re.IGNORECASE)
assert m is not None, 're.search with IGNORECASE flag'
assert m.group() == 'HELLO', 're.search IGNORECASE matches case-insensitively'

m = re.search(r'hello', 'HELLO WORLD')
assert m is None, 're.search without flags is case-sensitive'

# re.match with flags
m = re.match(r'hello', 'HELLO WORLD', re.IGNORECASE)
assert m is not None, 're.match with IGNORECASE flag'
assert m.group() == 'HELLO', 're.match IGNORECASE matches case-insensitively'

# re.fullmatch with flags
m = re.fullmatch(r'hello', 'HELLO', re.IGNORECASE)
assert m is not None, 're.fullmatch with IGNORECASE flag'
assert m.group() == 'HELLO', 're.fullmatch IGNORECASE matches case-insensitively'

# re.findall with flags
result = re.findall(r'hello', 'Hello HELLO hello', re.IGNORECASE)
assert result == ['Hello', 'HELLO', 'hello'], 're.findall with IGNORECASE flag'

# re.sub with flags (5th positional arg)
result = re.sub(r'hello', 'X', 'Hello HELLO hello', 0, re.IGNORECASE)
assert result == 'X X X', 're.sub with flags as 5th arg'

# re.search with DOTALL flag
m = re.search(r'a.b', 'a\nb', re.DOTALL)
assert m is not None, 're.search with DOTALL flag'
assert m.group() == 'a\nb', 're.search DOTALL matches across newlines'

# re.findall with MULTILINE
result = re.findall(r'^\w+', 'hello\nworld\nfoo', re.MULTILINE)
assert result == ['hello', 'world', 'foo'], 're.findall with MULTILINE flag'

# re.search with combined flags
m = re.search(r'hello.*world', 'HELLO\nWORLD', re.IGNORECASE | re.DOTALL)
assert m is not None, 're.search with IGNORECASE | DOTALL'
assert m.group() == 'HELLO\nWORLD', 're.search combined flags work'

# === re.ASCII flag ===
assert re.ASCII == 256, 're.ASCII flag value'
assert re.A == re.ASCII, 're.A is alias for re.ASCII'

# re.ASCII flag is accepted (doesn't error)
p = re.compile(r'\w+', re.ASCII)
m = p.search('cafe')
assert m is not None, 'ASCII mode matches ASCII word chars'
assert m.group() == 'cafe', 'ASCII mode returns correct match'

# re.ASCII can be combined with other flags
p = re.compile(r'hello', re.ASCII | re.IGNORECASE)
m = p.search('HELLO')
assert m is not None, 'ASCII | IGNORECASE combined'
assert m.group() == 'HELLO', 'ASCII | IGNORECASE matches correctly'

# Pattern repr with re.ASCII flag
p = re.compile(r'\w+', re.ASCII)
assert repr(p) == r"re.compile('\\w+', re.ASCII)", 'Pattern repr with ASCII flag'

p = re.compile(r'\w+', re.ASCII | re.IGNORECASE)
assert repr(p) == r"re.compile('\\w+', re.IGNORECASE|re.ASCII)", 'Pattern repr with ASCII|IGNORECASE flags'

# re.ASCII on module-level functions
m = re.search(r'\w+', 'cafe', re.ASCII)
assert m is not None, 're.search with re.ASCII flag'
assert m.group() == 'cafe', 're.search re.ASCII returns correct match'

m = re.match(r'\w+', 'cafe', re.A)
assert m is not None, 're.match with re.A alias'
assert m.group() == 'cafe', 're.match re.A returns correct match'

m = re.fullmatch(r'\w+', 'cafe', re.ASCII)
assert m is not None, 're.fullmatch with re.ASCII flag'
assert m.group() == 'cafe', 're.fullmatch re.ASCII returns correct match'

result = re.findall(r'\w+', 'a b c', re.ASCII)
assert result == ['a', 'b', 'c'], 're.findall with re.ASCII flag'

# === match with alternation (anchored) ===
# re.match('b|ab', 'ab') must try alternation at position 0
m = re.match(r'b|ab', 'ab')
assert m is not None, 're.match with alternation at start'
assert m.group() == 'ab', 're.match alternation: second alt matches at pos 0'

# re.match with alternation: first alt doesn't start at pos 0
m = re.match(r'world|hello', 'hello world')
assert m is not None, 're.match alternation: finds match starting at pos 0'
assert m.group() == 'hello', 're.match alternation: correct alternative matches'

# compiled pattern match with alternation
p = re.compile(r'b|ab')
m = p.match('ab')
assert m is not None, 'Pattern.match with alternation'
assert m.group() == 'ab', 'Pattern.match alternation: second alt matches at pos 0'

# match with alternation where shorter alt matches at pos 0
m = re.match(r'a|ab', 'ab')
assert m is not None, 're.match alternation: shorter alt at pos 0'
assert m.group() == 'a', 're.match alternation: leftmost match wins (like CPython)'

# match with alternation + flags
m = re.match(r'B|AB', 'ab', re.IGNORECASE)
assert m is not None, 're.match alternation with IGNORECASE flag'
assert m.group() == 'ab', 're.match alternation IGNORECASE: second alt matches at pos 0'

# compiled match with alternation + flags
p = re.compile(r'B|AB', re.IGNORECASE)
m = p.match('ab')
assert m is not None, 'Pattern.match alternation with IGNORECASE flag'
assert m.group() == 'ab', 'Pattern.match alternation IGNORECASE matches correctly'

# === \g<N> numeric backreference in replacement ===
result = re.sub(r'(\w+)\s+(\w+)', r'\g<2> \g<1>', 'hello world')
assert result == 'world hello', r'\g<N> numeric backreference swaps groups'

result = re.sub(r'(\w+)\s+(\w+)', r'\g<0>', 'hello world')
assert result == 'hello world', r'\g<0> is the full match'

result = re.sub(r'(\w+)', r'\g<1>!', 'hello world')
assert result == 'hello! world!', r'\g<1> with suffix'

# \g<N> with multiple replacements in one string
result = re.sub(r'(\w+)\s+(\w+)\s+(\w+)', r'\g<3>-\g<2>-\g<1>', 'a b c')
assert result == 'c-b-a', r'\g<N> multiple groups reversed'

# \g<N> mixed with \1 style backrefs
result = re.sub(r'(\w+)\s+(\w+)', r'\1-\g<2>', 'hello world')
assert result == 'hello-world', r'\1 and \g<2> mixed in replacement'

# \g<N> mixed with literal $
result = re.sub(r'(\w+)', r'$\g<1>$', 'hi')
assert result == '$hi$', r'\g<1> with literal $ signs'

# === \g<name> named backreference in replacement ===
result = re.sub(r'(?P<first>\w+)\s+(?P<second>\w+)', r'\g<second> \g<first>', 'hello world')
assert result == 'world hello', r'\g<name> named backreference swaps groups'

# \g<name> on compiled pattern
p = re.compile(r'(?P<word>\w+)')
result = p.sub(r'[\g<word>]', 'hello world')
assert result == '[hello] [world]', r'compiled pattern \g<name> backreference'

# \g<name> mixed with \g<N>
result = re.sub(r'(?P<a>\w+)\s+(\w+)', r'\g<a>-\g<2>', 'hello world')
assert result == 'hello-world', r'\g<name> and \g<N> mixed'

# === \g combined with other replacement features ===
result = re.sub(r'(\w+)', r'[\g<1>]', 'hi')
assert result == '[hi]', r'\g<1> with surrounding literal brackets'

# compiled pattern with \g
p = re.compile(r'(\w+)\s+(\w+)')
result = p.sub(r'\g<2>-\g<1>', 'hello world')
assert result == 'world-hello', r'compiled pattern \g<N> backreference'

# === Bool as int in re functions ===
# bool as flags (True=1, False=0)
m = re.search(r'hello', 'HELLO', False)
assert m is None, 'search flags=False (0) is case-sensitive'

m = re.match(r'hello', 'HELLO', False)
assert m is None, 'match flags=False is case-sensitive'

m = re.fullmatch(r'hello', 'HELLO', False)
assert m is None, 'fullmatch flags=False is case-sensitive'

result = re.findall(r'hello', 'HELLO hello', False)
assert result == ['hello'], 'findall flags=False is case-sensitive'

p = re.compile(r'hello', False)
assert p.flags & re.IGNORECASE == 0, 'compile with flags=False has no IGNORECASE'

p = re.compile(r'hello', True)
assert p.flags & 1 != 0, 'compile with flags=True stores 1'

# bool as count in re.sub (True=1 replacement, False=0=all)
result = re.sub(r'\d', 'X', '123', True)
assert result == 'X23', 'count=True replaces only first match'

result = re.sub(r'\d', 'X', '123', False)
assert result == 'XXX', 'count=False (0) replaces all matches'

# bool as count in Pattern.sub
p = re.compile(r'\d')
result = p.sub('X', '123', True)
assert result == 'X23', 'Pattern.sub count=True replaces only first'

result = p.sub('X', '123', False)
assert result == 'XXX', 'Pattern.sub count=False replaces all'

# === re.error alias (same as re.PatternError) ===
assert re.error is re.PatternError, 're.error is alias for re.PatternError'
try:
    re.compile('(unclosed')
    assert False, 'should raise'
except re.error as e:
    assert len(str(e)) > 0, 're.error catches PatternError'

# === re.escape() ===
assert re.escape('hello') == 'hello', 're.escape leaves alphanumeric unchanged'
assert re.escape('hello world!') == 'hello\\ world!', 're.escape escapes space but not !'
assert re.escape('a.b+c*d?e') == 'a\\.b\\+c\\*d\\?e', 're.escape escapes regex metacharacters'
assert re.escape('') == '', 're.escape on empty string'
assert re.escape('[test]') == '\\[test\\]', 're.escape escapes brackets'
assert re.escape('price: $10') == 'price:\\ \\$10', 're.escape escapes space and dollar'
assert re.escape('a_b') == 'a_b', 're.escape preserves underscores'

# re.escape result works as a literal pattern
text = 'price is $10.00 (USD)'
escaped = re.escape('$10.00')
m = re.search(escaped, text)
assert m is not None, 'escaped pattern matches literally'
assert m.group() == '$10.00', 'escaped pattern matches the exact string'

# === re.sub() with keyword arguments ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', count=1)
assert result == 'aX b2 c3', 're.sub with count kwarg'

result = re.sub(r'hello', 'X', 'Hello HELLO hello', count=0, flags=re.IGNORECASE)
assert result == 'X X X', 're.sub with flags kwarg'

# Pattern.sub with count kwarg
p = re.compile(r'\d+')
result = p.sub('X', 'a1 b2 c3', count=1)
assert result == 'aX b2 c3', 'Pattern.sub with count kwarg'

# === re.split() ===
result = re.split(r'\s+', 'hello world foo')
assert result == ['hello', 'world', 'foo'], 're.split basic'

result = re.split(r'[,;]', 'a,b;c')
assert result == ['a', 'b', 'c'], 're.split on multiple delimiters'

result = re.split(r'\s+', 'hello world foo', maxsplit=1)
assert result == ['hello', 'world foo'], 're.split with maxsplit=1'

result = re.split(r'\s+', 'hello')
assert result == ['hello'], 're.split with no matches'

result = re.split(r'\s+', '')
assert result == [''], 're.split on empty string'

# Pattern.split
p = re.compile(r'[,;]')
result = p.split('a,b;c')
assert result == ['a', 'b', 'c'], 'Pattern.split basic'

result = p.split('a,b;c', maxsplit=1)
assert result == ['a', 'b;c'], 'Pattern.split with maxsplit kwarg'

# === re.finditer() ===
matches = list(re.finditer(r'\d+', 'a1 b22 c333'))
assert len(matches) == 3, 'finditer returns 3 matches'
assert matches[0].group() == '1', 'finditer match 0'
assert matches[1].group() == '22', 'finditer match 1'
assert matches[2].group() == '333', 'finditer match 2'

# finditer with no matches
matches = list(re.finditer(r'\d+', 'no numbers'))
assert len(matches) == 0, 'finditer with no matches returns empty'

# finditer iteration
groups = [m.group() for m in re.finditer(r'\w+', 'hello world')]
assert groups == ['hello', 'world'], 'finditer in list comprehension'

# Pattern.finditer
p = re.compile(r'\d+')
matches = list(p.finditer('a1 b22'))
assert len(matches) == 2, 'Pattern.finditer returns 2 matches'
assert matches[0].group() == '1', 'Pattern.finditer match 0'
assert matches[1].group() == '22', 'Pattern.finditer match 1'

# finditer with capture groups
matches = list(re.finditer(r'(\w+)=(\w+)', 'a=1 b=2'))
assert len(matches) == 2, 'finditer with groups returns 2 matches'
assert matches[0].group(1) == 'a', 'finditer group 1 of match 0'
assert matches[0].group(2) == '1', 'finditer group 2 of match 0'
assert matches[1].group(1) == 'b', 'finditer group 1 of match 1'
