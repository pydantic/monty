# Tests for the re (regular expression) module - Match object

import re

# === Match .string attribute ===
m = re.search('hello', 'say hello')
assert m is not None, 'search finds match for .string test'
assert m.string == 'say hello', '.string returns the input string'

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
