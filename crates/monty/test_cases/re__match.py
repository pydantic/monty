# Tests for the re (regular expression) module - Match object

import re

# === Match .string attribute ===
subject = 'say ' + 'hello'  # concatenate so it isn't interned
m = re.search('hello', subject)
assert m is not None, 'search finds match for .string test'
assert m.string == 'say hello', '.string returns the input string'
assert m.string is subject, '.string is the original subject object, not a copy'

# === Match truthiness ===
m = re.search(r'\d+', '123')
assert m, 'Match objects are truthy'

# === Match repr ===
m = re.search(r'\d+', 'abc 42 def')
assert repr(m) == "<re.Match object; span=(4, 6), match='42'>", 'Match repr'

# === Object basic ===
assert bool(re.search(r'\w+', 'hello'))
assert isinstance(re.search(r'\w+', 'hello'), re.Match), 're.search returns re.Match instance'
assert str(type(re.search(r'\w+', 'hello'))) == "<class 're.Match'>", 'type of search match is re.Match'

# === Match equality - Match objects are not comparable ===
m1 = re.search(r'\w+', 'hello')
m2 = re.search(r'\w+', 'hello')
assert (m1 == m2) == False, 'different Match objects are not equal'
assert m1 != m2, 'Match objects with same content are not equal'

# === Match methods are reusable on same object ===
m = re.search(r'(\w+)@(\w+)', 'user@host')
assert m is not None, 'search finds match'
assert m.group(0) == 'user@host', 'first call to group(0) works'
assert m.group(0) == 'user@host', 'second call to group(0) works'
assert m.group(1) == 'user', 'call to group(1) works'
assert m.start(1) == 0, 'start(1) works'
assert m.end(1) == 4, 'end(1) works'
assert m.span(0) == (0, 9), 'span(0) works'

# === .string attribute is accessible multiple times ===
m = re.search(r'hello', 'say hello world')
assert m is not None, 'search finds match'
assert m.string == 'say hello world', 'first access to .string works'
assert m.string == 'say hello world', 'second access to .string works'

# === Match object with empty string ===
m = re.search(r'', 'hello')
assert m is not None, 'empty pattern matches'
assert m.string == 'hello', '.string returns input for empty match'
assert m.group(0) == '', 'empty match group(0) is empty string'

# === Match object from match() function ===
m = re.match(r'(\w+)', 'hello world')
assert m is not None, 're.match finds match'
assert m.group(0) == 'hello', 'match() returns correct match'
assert m.start(0) == 0, 'match starts at position 0'
assert m.string == 'hello world', '.string returns full input'

# === Match object from fullmatch() function ===
m = re.fullmatch(r'\w+', 'hello')
assert m is not None, 're.fullmatch finds exact match'
assert m.group(0) == 'hello', 'fullmatch returns correct match'
assert m.start(0) == 0, 'fullmatch starts at position 0'
assert m.end(0) == 5, 'fullmatch ends at correct position'

# === Match repr basic format ===
m = re.search(r'\d+', 'abc 42 def')
assert repr(m) == "<re.Match object; span=(4, 6), match='42'>", 'Match repr basic format'

m = re.search(r'\w+', 'hello')
assert repr(m) == "<re.Match object; span=(0, 5), match='hello'>", 'Match repr at start'

m = re.search(r'', 'hello')
assert repr(m) == "<re.Match object; span=(0, 0), match=''>", 'Match repr empty match'

# === Match repr with special characters ===
p = re.compile(r'a.b', re.DOTALL)
m = p.search('a\nb')
assert m is not None, 'DOTALL match for repr test'
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\nb'>", 'Match repr escapes newline'

m = re.search(r'a.b', 'a\tb')
assert m is not None, 'tab match for repr test'
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\tb'>", 'Match repr escapes tab'

# backslash in matched text
m = re.search(r'a.b', 'a\\b')
assert m is not None, 'backslash match for repr test'
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\\\b'>", 'Match repr escapes backslash'

# carriage return in matched text
p = re.compile(r'a.b', re.DOTALL)
m = p.search('a\rb')
assert m is not None, 'carriage return match for repr test'
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\\rb'>", 'Match repr escapes carriage return'

# single quote in matched text — repr switches to double quotes
m = re.search(r'a.b', "a'b")
assert m is not None, 'single quote match for repr test'
r = repr(m)
assert r == '<re.Match object; span=(0, 3), match="a\'b">', 'Match repr handles single quote'

# double quote in matched text — repr uses single quotes
m = re.search(r'a.b', 'a"b')
assert m is not None, 'double quote match for repr test'
r = repr(m)
assert r == "<re.Match object; span=(0, 3), match='a\"b'>", 'Match repr handles double quote'

# === Pattern repr ===
p = re.compile('hello')
assert repr(p) == "re.compile('hello')", 'Pattern repr simple string'

p = re.compile(r'\n\t')
assert repr(p) == "re.compile('\\\\n\\\\t')", 'Pattern repr with escape sequences in pattern'

# === Bool as group index ===
m = re.search(r'(\w+)\s+(\w+)', 'hello world')
assert m is not None, 'search for bool group test'
assert m.group(True) == 'hello', 'group(True) is group(1)'
assert m.group(False) == 'hello world', 'group(False) is group(0)'
assert m.start(True) == 0, 'start(True) is start(1)'
assert m.end(True) == 5, 'end(True) is end(1)'
assert m.span(True) == (0, 5), 'span(True) is span(1)'
assert m.span(False) == (0, 11), 'span(False) is span(0)'

# === m[N] subscript access ===
m = re.search(r'(\w+)\s+(\w+)', 'hello world')
assert m is not None, 'search for subscript test'
assert m[0] == 'hello world', 'm[0] is full match'
assert m[1] == 'hello', 'm[1] is first group'
assert m[2] == 'world', 'm[2] is second group'

# subscript with named groups
m = re.search(r'(?P<first>\w+)\s+(?P<second>\w+)', 'hello world')
assert m is not None, 'search for named subscript test'
assert m['first'] == 'hello', "m['first'] accesses named group"
assert m['second'] == 'world', "m['second'] accesses named group"
assert m[1] == 'hello', 'm[1] also works with named groups'

# subscript with invalid index
try:
    m[99]
    assert False, 'out-of-range subscript should raise IndexError'
except IndexError as e:
    assert str(e) == 'no such group', 'subscript IndexError message'

# === Non-ASCII subjects: positions are character offsets, not bytes ===
subject = 'héllo wörld date: 2026-07-06 ünd'
m = re.search(r'\d{4}-\d{2}-\d{2}', subject)
assert m is not None, 'search finds date in non-ASCII subject'
assert m.span() == (18, 28), 'non-ASCII subject span is in characters'
assert m.start() == 18, 'non-ASCII subject start is in characters'
assert m.end() == 28, 'non-ASCII subject end is in characters'
assert m.span() == (18, 28), 'repeated span call returns the same result'
assert subject[m.start() : m.end()] == '2026-07-06', 'char offsets slice the subject correctly'

# group spans and unmatched groups on a non-ASCII subject
m = re.search(r'(\w+)@(\w+)(!)?', 'çontact: müller@pydantic é')
assert m is not None, 'search finds match in non-ASCII subject with groups'
assert m.span() == (9, 24), 'full-match span in characters'
assert m.span(1) == (9, 15), 'group 1 span in characters'
assert m.span(2) == (16, 24), 'group 2 span in characters'
assert m.span(3) == (-1, -1), 'unmatched group span is (-1, -1)'
assert m.start(2) == 16, 'group 2 start in characters'
assert m.end(2) == 24, 'group 2 end in characters'
assert repr(m) == "<re.Match object; span=(9, 24), match='müller@pydantic'>", 'repr span in characters'
