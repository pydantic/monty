# Tests for the re (regular expression) module - basic functionality

import re
import sys

_monty = 'Monty' in sys.version

# === Constant ===
assert re.NOFLAG == 0
assert re.I == re.IGNORECASE == 2, 're.I == re.IGNORECASE == 2'
assert re.M == re.MULTILINE == 8, 're.M == re.MULTILINE == 8'
assert re.S == re.DOTALL == 16, 're.S == re.DOTALL == 16'

# === re.search() basic ===
m = re.search('hello', 'say hello world')
assert m is not None
assert m.group() == 'hello'
assert m.group(0) == 'hello'
assert m.start() == 4
assert m.end() == 9
assert m.span() == (4, 9)

# === re.search() with no match ===
m = re.search('xyz', 'hello world')
assert m is None

# === re.search() with error ===
try:
    re.search('(', 'test')
    assert False, 're.search with invalid pattern should raise error'
except re.PatternError as e:
    # The error message may vary based on the regex engine, but it should not be empty
    assert len(str(e)) > 0

# === re.match() ===
m = re.match('hello', 'hello world')
assert m is not None
assert m.group() == 'hello'

m = re.match('world', 'hello world')
assert m is None

# === re.fullmatch() ===
m = re.fullmatch('hello', 'hello')
assert m is not None
assert m.group() == 'hello'

m = re.fullmatch('hello', 'hello world')
assert m is None

# === re.findall() with no groups ===
result = re.findall(r'\d+', 'a1 b22 c333')
assert result == ['1', '22', '333']

# === re.findall() with no match ===
result = re.findall(r'\d+', 'no numbers')
assert result == []

# === re.sub() ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3')
assert result == 'aX bX cX'

# === re.sub() with count ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', 1)
assert result == 'aX b2 c3'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 2)
assert result == 'aX bX c3'

# === re.compile() ===
pattern = re.compile(r'\d+')
m = pattern.search('abc 123 def')
assert m is not None
assert m.group() == '123'

m = pattern.match('123 abc')
assert m is not None
assert m.group() == '123'

m = pattern.match('abc 123')
assert m is None

# === compiled pattern fullmatch ===
pattern = re.compile(r'\d+')
m = pattern.fullmatch('123')
assert m is not None
assert m.group() == '123'

m = pattern.fullmatch('123abc')
assert m is None

# === compiled pattern findall ===
pattern = re.compile(r'\d+')
result = pattern.findall('a1 b2 c3')
assert result == ['1', '2', '3']

# === compiled pattern sub ===
pattern = re.compile(r'\d+')
result = pattern.sub('X', 'a1 b2 c3')
assert result == 'aX bX cX'

result = pattern.sub('X', 'a1 b2 c3', 1)
assert result == 'aX b2 c3'

# === Flags: IGNORECASE ===
pattern = re.compile(r'hello', re.IGNORECASE)
m = pattern.search('Hello World')
assert m is not None
assert m.group() == 'Hello'

# === Flags: DOTALL ===
pattern = re.compile(r'a.b', re.DOTALL)
m = pattern.search('a\nb')
assert m is not None
assert m.group() == 'a\nb'

# === Flags: MULTILINE ===
pattern = re.compile(r'^\w+', re.MULTILINE)
result = pattern.findall('hello\nworld')
assert result == ['hello', 'world']

# === Pattern attributes ===
pattern = re.compile(r'\d+', re.IGNORECASE)
assert pattern.pattern == r'\d+'
# CPython flags include re.UNICODE (32) by default, so we check flags & 2 instead
assert pattern.flags & re.IGNORECASE

# === Pattern repr ===
p = re.compile(r'\d+')
assert repr(p) == r"re.compile('\\d+')"

p = re.compile(r'\d+', re.IGNORECASE)
assert repr(p) == r"re.compile('\\d+', re.IGNORECASE)"

# === Flag constants ===
assert re.IGNORECASE == 2
assert re.MULTILINE == 8
assert re.DOTALL == 16

# === Combined flags ===
pattern = re.compile(r'^hello', re.IGNORECASE | re.MULTILINE)
result = pattern.findall('Hello\nhello\nHELLO')
assert result == ['Hello', 'hello', 'HELLO']

# === More MULTILINE tests ===
# Without MULTILINE, ^ matches only start of string
pattern = re.compile(r'^\w+')
result = pattern.findall('line1\nline2\nline3')
assert result == ['line1']

# With MULTILINE, ^ matches each line start
pattern = re.compile(r'^\w+', re.MULTILINE)
result = pattern.findall('line1\nline2\nline3')
assert result == ['line1', 'line2', 'line3']

# Without MULTILINE, $ matches only end of string
pattern = re.compile(r'\w+$')
result = pattern.findall('line1\nline2\nline3')
assert result == ['line3']

# With MULTILINE, $ matches each line end
pattern = re.compile(r'\w+$', re.MULTILINE)
result = pattern.findall('line1\nline2\nline3')
assert result == ['line1', 'line2', 'line3']

# === More DOTALL tests ===
# Without DOTALL, . does not match newline
pattern = re.compile(r'a.b')
m = pattern.search('a\nb')
assert m is None

# With DOTALL, . matches newline
pattern = re.compile(r'a.b', re.DOTALL)
m = pattern.search('a\nb')
assert m is not None
assert m.group() == 'a\nb'

# DOTALL with multiple newlines
pattern = re.compile(r'start.*end', re.DOTALL)
m = pattern.search('start\nline1\nline2\nend')
assert m is not None
assert m.group() == 'start\nline1\nline2\nend'

# === Pattern repr with multiple flags (I, M, D order) ===
p = re.compile(r'test', re.IGNORECASE)
assert repr(p) == r"re.compile('test', re.IGNORECASE)"

p = re.compile(r'test', re.MULTILINE)
assert repr(p) == r"re.compile('test', re.MULTILINE)"

p = re.compile(r'test', re.DOTALL)
assert repr(p) == r"re.compile('test', re.DOTALL)"

p = re.compile(r'test', re.IGNORECASE | re.MULTILINE)
assert repr(p) == r"re.compile('test', re.IGNORECASE|re.MULTILINE)"

p = re.compile(r'test', re.IGNORECASE | re.DOTALL)
assert repr(p) == r"re.compile('test', re.IGNORECASE|re.DOTALL)"

p = re.compile(r'test', re.MULTILINE | re.DOTALL)
assert repr(p) == r"re.compile('test', re.MULTILINE|re.DOTALL)"

p = re.compile(r'test', re.IGNORECASE | re.MULTILINE | re.DOTALL)
assert repr(p) == r"re.compile('test', re.IGNORECASE|re.MULTILINE|re.DOTALL)"

# === Combined IGNORECASE and DOTALL ===
pattern = re.compile(r'Hello.*World', re.IGNORECASE | re.DOTALL)
m = pattern.search('HELLO\nmiddle\nWORLD')
assert m is not None
assert m.group() == 'HELLO\nmiddle\nWORLD'

# === Combined MULTILINE and DOTALL ===
pattern = re.compile(r'^a.*b$', re.MULTILINE | re.DOTALL)
result = pattern.findall('a\nb\nc\nb')
assert result == ['a\nb\nc\nb']

# === All three flags combined ===
pattern = re.compile(r'^Hello.*World$', re.IGNORECASE | re.MULTILINE | re.DOTALL)
m = pattern.search('first\nHELLO\nsome\nlines\nWORLD\nlast')
assert m is not None
assert m.group() == 'HELLO\nsome\nlines\nWORLD'

# === Empty pattern ===
m = re.search(r'', 'abc')
assert m is not None
assert m.start() == 0 and m.end() == 0, 'empty pattern matches at start of string'

# === Zero-length matches ===
m = re.search(r'a*', 'bc')
assert m is not None
assert m.group() == ''

# === Object identity of compiled patterns ===
p1 = re.compile(r'\d+')
p2 = re.compile(r'\d+')
assert p1 == p2
match1 = p1.search('123')
match2 = p2.search('123')
assert match1 != match2

# === re.sub() error: missing args are aggregated like a Python def ===
try:
    re.sub()
    assert False, 're.sub() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == "sub() missing 3 required positional arguments: 'pattern', 'repl', and 'string'", (
        f're.sub missing all: {e}'
    )

try:
    re.sub(r'\d+')
    assert False, 're.sub(pattern) should raise TypeError'
except TypeError as e:
    assert str(e) == "sub() missing 2 required positional arguments: 'repl' and 'string'", f're.sub missing two: {e}'

try:
    re.sub(r'\d+', 'X')
    assert False, 're.sub(pattern, repl) should raise TypeError'
except TypeError as e:
    assert str(e) == "sub() missing 1 required positional argument: 'string'", f're.sub missing string: {e}'

# === re.sub() error: too many positional args (range wording) ===
try:
    re.sub(r'\d+', 'X', 'a1', 0, 0, 'extra')
    assert False, 're.sub with 6 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'sub() takes from 3 to 5 positional arguments but 6 were given', f're.sub too many: {e}'

# === re.sub() error: count is not an integer ===
try:
    re.sub(r'\d+', 'X', 'a1b2', 1.5)
    assert False, 're.sub with float count should raise TypeError'
except TypeError as e:
    assert str(e) == "'float' object cannot be interpreted as an integer", f're.sub float count error: {e}'

try:
    re.sub(r'\d+', 'X', 'a1b2', 'one')
    assert False, 're.sub with string count should raise TypeError'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer", f're.sub string count error: {e}'

# === re.sub() count wider than 32 bits still caps (never truncates to 0) ===
assert re.sub(r'\d', 'X', '1a2', 2**32) == 'XaX'

# === re.sub() negative count returns the subject unchanged ===
assert re.sub(r'\d', 'X', '1a2', -1) == '1a2'
# ... but the pattern still compiles and the subject is still type-checked
try:
    re.sub('(unclosed', 'X', '1a2', -1)
    assert False, 're.sub with bad pattern and negative count should raise'
except re.PatternError:
    pass
try:
    re.sub(r'\d', 'X', 123, -1)
    assert False, 're.sub with int subject and negative count should raise TypeError'
except TypeError as e:
    assert str(e) == "expected string or bytes-like object, got 'int'", f're.sub negative count subject: {e}'
# ... and a non-str repl is rejected too — CPython processes the replacement
# template before its match loop, even when zero substitutions will run
# (messages differ: callable replacement is a Monty feature gap, see
# limitations/re.md)
try:
    re.sub(r'\d', 123, '1a2', -1)
    assert False, 're.sub with int repl and negative count should raise TypeError'
except TypeError as e:
    if _monty:
        assert str(e) == 'callable replacement is not yet supported in re.sub()', f're.sub neg-count repl: {e}'
    else:
        assert str(e) == 'decoding to str: need a bytes-like object, int found', f're.sub neg-count repl: {e}'
# ... with the repl error winning over a bad subject
try:
    re.sub(r'\d', None, 456, -1)
    assert False, 're.sub with bad repl and bad subject should raise for the repl'
except TypeError as e:
    if _monty:
        assert str(e) == 'callable replacement is not yet supported in re.sub()', f're.sub repl-first: {e}'
    else:
        assert str(e) == 'decoding to str: need a bytes-like object, NoneType found', f're.sub repl-first: {e}'
# Pattern.sub applies the same order
try:
    re.compile(r'\d').sub(123, '1a2', -1)
    assert False, 'Pattern.sub with int repl and negative count should raise TypeError'
except TypeError as e:
    if _monty:
        assert str(e) == 'callable replacement is not yet supported in re.sub()', f'Pattern.sub neg-count repl: {e}'
    else:
        assert str(e) == 'decoding to str: need a bytes-like object, int found', f'Pattern.sub neg-count repl: {e}'

# === re.sub() non-str repl (positive-count path) ===
try:
    re.sub(r'\d', None, '1a2')
    assert False, 're.sub with None repl should raise TypeError'
except TypeError as e:
    if _monty:
        assert str(e) == 'callable replacement is not yet supported in re.sub()', f're.sub None repl: {e}'
    else:
        assert str(e) == 'decoding to str: need a bytes-like object, NoneType found', f're.sub None repl: {e}'

# === Pattern.sub() error: missing repl ===
pattern = re.compile(r'\d+')
try:
    pattern.sub()
    assert False, 'Pattern.sub() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == "sub() missing required argument 'repl' (pos 1)"

# === Pattern.sub() error: missing string ===
try:
    pattern.sub('X')
    assert False, 'Pattern.sub(repl) should raise TypeError'
except TypeError as e:
    assert str(e) == "sub() missing required argument 'string' (pos 2)"

# === re.sub() with count=0 (replace all) ===
result = re.sub(r'\d', 'X', '1a2b3c', 0)
assert result == 'XaXbXc'

# === re.sub() empty replacement ===
result = re.sub(r'\d+', '', 'a1 b2 c3')
assert result == 'a b c'

# === Pattern.sub() edge case: empty match ===
pattern = re.compile(r'a*')
result = pattern.sub('X', 'bac')
# Note: this might be a zero-width match behavior that's different
assert 'X' in result

# === re.compile() error: invalid pattern ===
try:
    re.compile('(unclosed')
    assert False, 're.compile with invalid pattern should raise PatternError'
except re.PatternError as e:
    assert len(str(e)) > 0

# === re.search() error: pattern is not a string ===
try:
    re.search(123, 'hello')
    assert False, 're.search with int pattern should raise TypeError'
except TypeError as e:
    assert str(e) == 'first argument must be string or compiled pattern', f're.search int pattern: {e}'

# === re.search() error: string is not a string ===
try:
    re.search(r'\d+', 123)
    assert False, 're.search with int string should raise TypeError'
except TypeError as e:
    assert str(e) == "expected string or bytes-like object, got 'int'", f're.search int string: {e}'

# === re.match() error: pattern is not a string ===
try:
    re.match(None, 'hello')
    assert False, 're.match with None pattern should raise TypeError'
except TypeError as e:
    assert str(e) == 'first argument must be string or compiled pattern', f're.match None pattern: {e}'

# === re.fullmatch() error: string is not a string ===
try:
    re.fullmatch(r'\d+', None)
    assert False, 're.fullmatch with None string should raise TypeError'
except TypeError as e:
    assert str(e) == "expected string or bytes-like object, got 'NoneType'", f're.fullmatch None string: {e}'

# === re.search() error: flags is not an integer ===
try:
    re.search(r'\d+', 'a1', 'bad')
    assert False, 're.search with str flags should raise TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for &: 'str' and 'int'", f're.search str flags: {e}'

# === re.escape() error: pattern is not a string ===
try:
    re.escape(123)
    assert False, 're.escape with int should raise TypeError'
except TypeError as e:
    assert str(e) == 'decoding to str: need a bytes-like object, int found', f're.escape int: {e}'

# === re.search() error: arity checked before pattern type ===
# a lone non-string pattern must report the missing 'string' argument,
# not the pattern type error — signature binding never type-checks
try:
    re.search(123)
    assert False, 're.search with a single non-string arg should raise TypeError'
except TypeError as e:
    assert str(e) == "search() missing 1 required positional argument: 'string'", f're.search(123): {e}'

try:
    re.findall(123)
    assert False, 're.findall with a single non-string arg should raise TypeError'
except TypeError as e:
    assert str(e) == "findall() missing 1 required positional argument: 'string'", f're.findall(123): {e}'

# === re.search() error: too many args checked before pattern type ===
try:
    re.search(123, 'x', 0, 'extra')
    assert False, 're.search with 4 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'search() takes from 2 to 3 positional arguments but 4 were given', f're.search 4 args: {e}'

# === re.escape() arity (single fixed positional) ===
try:
    re.escape('a', 'b')
    assert False, 're.escape with 2 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'escape() takes 1 positional argument but 2 were given', f're.escape 2 args: {e}'

# === keyword arguments are accepted like CPython's pure-Python functions ===
assert re.search(pattern='h', string='hello') is not None
assert re.search(string='hello', pattern='h') is not None
assert re.match(pattern='h', string='hi') is not None
assert re.fullmatch(pattern='hi', string='hi') is not None
assert re.findall(string='a b', pattern=r'\w+') == ['a', 'b']
assert len(list(re.finditer(pattern='a', string='aa'))) == 2
assert re.sub(pattern='a', repl='b', string='aaa', count=1, flags=0) == 'baa'
assert re.split(pattern=' ', string='a b c', maxsplit=1) == ['a', 'b c']
assert re.compile(pattern='h', flags=re.I).search('H') is not None
assert re.escape(pattern='a.b') == 'a\\.b'
assert re.search('h', 'hello', flags=re.I) is not None

try:
    re.search('h', 'hello', bogus=1)
    assert False, 're.search with unknown kwarg should raise TypeError'
except TypeError as e:
    assert str(e) == "search() got an unexpected keyword argument 'bogus'", f're.search unknown kwarg: {e}'

try:
    re.search('h', string='hello', pattern='x')
    assert False, 're.search with duplicate pattern should raise TypeError'
except TypeError as e:
    assert str(e) == "search() got multiple values for argument 'pattern'", f're.search duplicate: {e}'

# === compiled patterns are accepted by the module-level functions ===
p = re.compile(r'(\w+)')
assert re.compile(p) is p
assert re.search(p, 'hi world').group(0) == 'hi'
assert re.match(p, 'hi') is not None
assert re.fullmatch(p, 'hi') is not None
assert re.findall(p, 'a b') == ['a', 'b']
assert len(list(re.finditer(p, 'a b'))) == 2
assert re.sub(p, 'X', 'a b') == 'X X'
assert re.split(re.compile(' '), 'a b') == ['a', 'b']

# ... but combining a compiled pattern with flags raises ValueError
try:
    re.compile(p, re.I)
    assert False, 're.compile with compiled pattern and flags should raise ValueError'
except ValueError as e:
    assert str(e) == 'cannot process flags argument with a compiled pattern', f're.compile flags+compiled: {e}'

try:
    re.search(p, 'x', re.I)
    assert False, 're.search with compiled pattern and flags should raise ValueError'
except ValueError as e:
    assert str(e) == 'cannot process flags argument with a compiled pattern', f're.search flags+compiled: {e}'

# === re.split() maxsplit semantics and type errors ===
assert re.split(' ', 'a b c', -1) == ['a b c']
try:
    re.split(' ', 'a b c', 'x')
    assert False, 're.split with str maxsplit should raise TypeError'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer", f're.split str maxsplit: {e}'

# === Object basic ===
assert bool(re.compile(r'\d+'))
assert bool(re.search(r'\w+', 'hello'))
assert isinstance(re.compile(r'\d+'), re.Pattern)
assert isinstance(re.search(r'\w+', 'hello'), re.Match)
assert str(type(re.compile(r'\d+'))) == "<class 're.Pattern'>"
assert str(type(re.search(r'\w+', 'hello'))) == "<class 're.Match'>"

# === fullmatch with alternation ===
# fullmatch must try all alternatives to find a full-string match,
# not just pick the first alternative that matches somewhere
m = re.fullmatch('a|ab', 'ab')
assert m is not None
assert m.group() == 'ab'

m = re.fullmatch('ab|a', 'ab')
assert m is not None
assert m.group() == 'ab'

m = re.fullmatch('cat|category', 'category')
assert m is not None
assert m.group() == 'category'

m = re.fullmatch('x|ab|a', 'ab')
assert m is not None
assert m.group() == 'ab'

# compiled pattern fullmatch with alternation
p = re.compile('a|ab')
m = p.fullmatch('ab')
assert m is not None
assert m.group() == 'ab'

# fullmatch with alternation and groups
m = re.fullmatch('(a)|(ab)', 'ab')
assert m is not None
assert m.group(0) == 'ab'
assert m.group(1) is None
assert m.group(2) == 'ab'

# fullmatch with quantifiers
m = re.fullmatch('a+|b+', 'aaa')
assert m is not None
assert m.group() == 'aaa'

# fullmatch with .* (greedy)
m = re.fullmatch('.*', 'anything')
assert m is not None
assert m.group() == 'anything'

# fullmatch on empty string with empty pattern
m = re.fullmatch('', '')
assert m is not None
assert m.group() == ''

# fullmatch should not match partial strings even with alternation
m = re.fullmatch('a|ab', 'abc')
assert m is None

# fullmatch with MULTILINE should still require full-string match
p = re.compile('hello', re.MULTILINE)
m = p.fullmatch('hello')
assert m is not None
assert m.group() == 'hello'

m = p.fullmatch('hello\nworld')
assert m is None

# fullmatch with alternation and flags combined
p = re.compile('(a+)|(b+)', re.MULTILINE)
m = p.fullmatch('bbb')
assert m is not None
assert m.group(0) == 'bbb'
assert m.group(1) is None
assert m.group(2) == 'bbb'

# === Literal $ in replacement ===
result = re.sub(r'\d+', '$', 'a1b2')
assert result == 'a$b$'

result = re.sub(r'\d+', '$1', 'a1b2')
assert result == 'a$1b$1'

result = re.sub(r'\d+', '$$', 'a1b2')
assert result == 'a$$b$$'

# compiled pattern with $ in replacement
p = re.compile(r'\d+')
result = p.sub('$', 'a1b2')
assert result == 'a$b$'

result = re.sub(r'\d+', '$$$', 'a1b2')
assert result == 'a$$$b$$$'

# plain replacement with no special chars
result = re.sub(r'\d+', 'NUM', 'a1 b2')
assert result == 'aNUM bNUM'

# === Negative count in re.sub ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', -1)
assert result == 'a1 b2 c3'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', -100)
assert result == 'a1 b2 c3'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', -999)
assert result == 'a1 b2 c3'

# compiled pattern with negative count
p = re.compile(r'\d+')
result = p.sub('X', 'a1 b2 c3', -1)
assert result == 'a1 b2 c3'

result = p.sub('X', 'a1 b2 c3', -100)
assert result == 'a1 b2 c3'

# negative count with empty string
result = re.sub(r'\d+', 'X', '', -1)
assert result == ''

# === re.sub with count boundary values ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', 0)
assert result == 'aX bX cX'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 1)
assert result == 'aX b2 c3'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 3)
assert result == 'aX bX cX'

result = re.sub(r'\d+', 'X', 'a1 b2 c3', 100)
assert result == 'aX bX cX'

# === Pattern.sub() error: too many arguments ===
p = re.compile(r'\d+')
try:
    p.sub('X', 'a1b2', 0, 'extra')
    assert False, 'Pattern.sub with 4 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'sub() takes at most 3 arguments (4 given)'

try:
    p.sub('X', 'a1b2', 0, count=1)
    assert False, 'Pattern.sub with positional and keyword count should raise TypeError'
except TypeError as e:
    assert str(e) == 'sub() takes at most 3 arguments (4 given)'

try:
    p.sub('X', 'a1b2', bogus=1)
    assert False, 'Pattern.sub with unknown kwarg should raise TypeError'
except TypeError as e:
    assert str(e) == "sub() got an unexpected keyword argument 'bogus'"

# === Flags on module-level functions ===
# re.search with flags
m = re.search(r'hello', 'HELLO WORLD', re.IGNORECASE)
assert m is not None
assert m.group() == 'HELLO'

m = re.search(r'hello', 'HELLO WORLD')
assert m is None

# re.match with flags
m = re.match(r'hello', 'HELLO WORLD', re.IGNORECASE)
assert m is not None
assert m.group() == 'HELLO'

# re.fullmatch with flags
m = re.fullmatch(r'hello', 'HELLO', re.IGNORECASE)
assert m is not None
assert m.group() == 'HELLO'

# re.findall with flags
result = re.findall(r'hello', 'Hello HELLO hello', re.IGNORECASE)
assert result == ['Hello', 'HELLO', 'hello']

# re.sub with flags (5th positional arg)
result = re.sub(r'hello', 'X', 'Hello HELLO hello', 0, re.IGNORECASE)
assert result == 'X X X'

# re.search with DOTALL flag
m = re.search(r'a.b', 'a\nb', re.DOTALL)
assert m is not None
assert m.group() == 'a\nb'

# re.findall with MULTILINE
result = re.findall(r'^\w+', 'hello\nworld\nfoo', re.MULTILINE)
assert result == ['hello', 'world', 'foo']

# re.search with combined flags
m = re.search(r'hello.*world', 'HELLO\nWORLD', re.IGNORECASE | re.DOTALL)
assert m is not None
assert m.group() == 'HELLO\nWORLD'

# === re.ASCII flag ===
assert re.ASCII == 256
assert re.A == re.ASCII

# re.ASCII flag is accepted (doesn't error)
p = re.compile(r'\w+', re.ASCII)
m = p.search('cafe')
assert m is not None
assert m.group() == 'cafe'

# re.ASCII can be combined with other flags
p = re.compile(r'hello', re.ASCII | re.IGNORECASE)
m = p.search('HELLO')
assert m is not None
assert m.group() == 'HELLO'

# Pattern repr with re.ASCII flag
p = re.compile(r'\w+', re.ASCII)
assert repr(p) == r"re.compile('\\w+', re.ASCII)"

p = re.compile(r'\w+', re.ASCII | re.IGNORECASE)
assert repr(p) == r"re.compile('\\w+', re.IGNORECASE|re.ASCII)"

# re.ASCII on module-level functions
m = re.search(r'\w+', 'cafe', re.ASCII)
assert m is not None
assert m.group() == 'cafe'

m = re.match(r'\w+', 'cafe', re.A)
assert m is not None
assert m.group() == 'cafe'

m = re.fullmatch(r'\w+', 'cafe', re.ASCII)
assert m is not None
assert m.group() == 'cafe'

result = re.findall(r'\w+', 'a b c', re.ASCII)
assert result == ['a', 'b', 'c']

# === match with alternation (anchored) ===
# re.match('b|ab', 'ab') must try alternation at position 0
m = re.match(r'b|ab', 'ab')
assert m is not None
assert m.group() == 'ab'

# re.match with alternation: first alt doesn't start at pos 0
m = re.match(r'world|hello', 'hello world')
assert m is not None
assert m.group() == 'hello'

# compiled pattern match with alternation
p = re.compile(r'b|ab')
m = p.match('ab')
assert m is not None
assert m.group() == 'ab'

# match with alternation where shorter alt matches at pos 0
m = re.match(r'a|ab', 'ab')
assert m is not None
assert m.group() == 'a'

# match with alternation + flags
m = re.match(r'B|AB', 'ab', re.IGNORECASE)
assert m is not None
assert m.group() == 'ab'

# compiled match with alternation + flags
p = re.compile(r'B|AB', re.IGNORECASE)
m = p.match('ab')
assert m is not None
assert m.group() == 'ab'

# === \g<N> numeric backreference in replacement ===
result = re.sub(r'(\w+)\s+(\w+)', r'\g<2> \g<1>', 'hello world')
assert result == 'world hello'

result = re.sub(r'(\w+)\s+(\w+)', r'\g<0>', 'hello world')
assert result == 'hello world'

result = re.sub(r'(\w+)', r'\g<1>!', 'hello world')
assert result == 'hello! world!'

# \g<N> with multiple replacements in one string
result = re.sub(r'(\w+)\s+(\w+)\s+(\w+)', r'\g<3>-\g<2>-\g<1>', 'a b c')
assert result == 'c-b-a'

# \g<N> mixed with \1 style backrefs
result = re.sub(r'(\w+)\s+(\w+)', r'\1-\g<2>', 'hello world')
assert result == 'hello-world'

# \g<N> mixed with literal $
result = re.sub(r'(\w+)', r'$\g<1>$', 'hi')
assert result == '$hi$'

# === \g<name> named backreference in replacement ===
result = re.sub(r'(?P<first>\w+)\s+(?P<second>\w+)', r'\g<second> \g<first>', 'hello world')
assert result == 'world hello'

# \g<name> on compiled pattern
p = re.compile(r'(?P<word>\w+)')
result = p.sub(r'[\g<word>]', 'hello world')
assert result == '[hello] [world]'

# \g<name> mixed with \g<N>
result = re.sub(r'(?P<a>\w+)\s+(\w+)', r'\g<a>-\g<2>', 'hello world')
assert result == 'hello-world'

# === \g combined with other replacement features ===
result = re.sub(r'(\w+)', r'[\g<1>]', 'hi')
assert result == '[hi]'

# compiled pattern with \g
p = re.compile(r'(\w+)\s+(\w+)')
result = p.sub(r'\g<2>-\g<1>', 'hello world')
assert result == 'world-hello'

# === Bool as int in re functions ===
# bool as flags (True=1, False=0)
m = re.search(r'hello', 'HELLO', False)
assert m is None

m = re.match(r'hello', 'HELLO', False)
assert m is None

m = re.fullmatch(r'hello', 'HELLO', False)
assert m is None

result = re.findall(r'hello', 'HELLO hello', False)
assert result == ['hello']

p = re.compile(r'hello', False)
assert p.flags & re.IGNORECASE == 0

p = re.compile(r'hello', True)
assert p.flags & 1 != 0

# bool as count in re.sub (True=1 replacement, False=0=all)
result = re.sub(r'\d', 'X', '123', True)
assert result == 'X23'

result = re.sub(r'\d', 'X', '123', False)
assert result == 'XXX'

# bool as count in Pattern.sub
p = re.compile(r'\d')
result = p.sub('X', '123', True)
assert result == 'X23'

result = p.sub('X', '123', False)
assert result == 'XXX'

# === re.error alias (same as re.PatternError) ===
assert re.error is re.PatternError
try:
    re.compile('(unclosed')
    assert False, 'should raise'
except re.error as e:
    assert len(str(e)) > 0

# === re.escape() ===
assert re.escape('hello') == 'hello'
assert re.escape('hello world!') == 'hello\\ world!'
assert re.escape('a.b+c*d?e') == 'a\\.b\\+c\\*d\\?e'
assert re.escape('') == ''
assert re.escape('[test]') == '\\[test\\]'
assert re.escape('price: $10') == 'price:\\ \\$10'
assert re.escape('a_b') == 'a_b'

# re.escape result works as a literal pattern
text = 'price is $10.00 (USD)'
escaped = re.escape('$10.00')
m = re.search(escaped, text)
assert m is not None
assert m.group() == '$10.00'

# === re.sub() with keyword arguments ===
result = re.sub(r'\d+', 'X', 'a1 b2 c3', count=1)
assert result == 'aX b2 c3'

result = re.sub(r'hello', 'X', 'Hello HELLO hello', count=0, flags=re.IGNORECASE)
assert result == 'X X X'

# Pattern.sub with count kwarg
p = re.compile(r'\d+')
result = p.sub('X', 'a1 b2 c3', count=1)
assert result == 'aX b2 c3'

# === re.split() ===
result = re.split(r'\s+', 'hello world foo')
assert result == ['hello', 'world', 'foo']

result = re.split(r'[,;]', 'a,b;c')
assert result == ['a', 'b', 'c']

result = re.split(r'\s+', 'hello world foo', maxsplit=1)
assert result == ['hello', 'world foo']

result = re.split(r'\s+', 'hello')
assert result == ['hello']

result = re.split(r'\s+', '')
assert result == ['']

# Pattern.split
p = re.compile(r'[,;]')
result = p.split('a,b;c')
assert result == ['a', 'b', 'c']

result = p.split('a,b;c', maxsplit=1)
assert result == ['a', 'b;c']

# === re.finditer() ===
matches = list(re.finditer(r'\d+', 'a1 b22 c333'))
assert len(matches) == 3
assert matches[0].group() == '1'
assert matches[1].group() == '22'
assert matches[2].group() == '333'

# finditer with no matches
matches = list(re.finditer(r'\d+', 'no numbers'))
assert len(matches) == 0

# finditer iteration
groups = [m.group() for m in re.finditer(r'\w+', 'hello world')]
assert groups == ['hello', 'world']

# Pattern.finditer
p = re.compile(r'\d+')
matches = list(p.finditer('a1 b22'))
assert len(matches) == 2
assert matches[0].group() == '1'
assert matches[1].group() == '22'

# finditer with capture groups
matches = list(re.finditer(r'(\w+)=(\w+)', 'a=1 b=2'))
assert len(matches) == 2
assert matches[0].group(1) == 'a'
assert matches[0].group(2) == '1'
assert matches[1].group(1) == 'b'
