# === String concatenation (+) ===
assert 'hello' + ' ' + 'world' == 'hello world', 'basic concat'
assert '' + 'test' == 'test', 'empty left concat'
assert 'test' + '' == 'test', 'empty right concat'
assert '' + '' == '', 'empty both concat'
assert 'a' + 'b' + 'c' + 'd' == 'abcd', 'multiple concat'

# === Augmented assignment (+=) ===
s = 'hello'
s += ' world'
assert s == 'hello world', 'basic iadd'

s = 'test'
s += ''
assert s == 'test', 'iadd empty'

s = 'a'
s += 'b'
s += 'c'
assert s == 'abc', 'multiple iadd'

s = 'ab'
s += s
assert s == 'abab', 'iadd self'

# === String length ===
assert len('') == 0, 'len empty'
assert len('a') == 1, 'len single'
assert len('hello') == 5, 'len basic'
assert len('hello world') == 11, 'len with space'
assert len('caf\xe9') == 4, 'len unicode'

# === String repr/str ===
assert repr('') == "''", 'empty string repr'
assert str('') == '', 'empty string str'

assert repr('hello') == "'hello'", 'string repr'
assert str('hello') == 'hello', 'string str'

assert repr('hello "world"') == '\'hello "world"\'', 'string with quotes repr'
assert str('hello "world"') == 'hello "world"', 'string with quotes str'

# === String repetition (*) ===
assert 'ab' * 3 == 'ababab', 'str mult int'
assert 3 * 'ab' == 'ababab', 'int mult str'
assert 'x' * 0 == '', 'str mult zero'
assert 'x' * -1 == '', 'str mult negative'
assert '' * 5 == '', 'empty str mult'
assert 'a' * 1 == 'a', 'str mult one'

# === String repetition augmented assignment (*=) ===
s = 'ab'
s *= 3
assert s == 'ababab', 'str imult'

s = 'x'
s *= 0
assert s == '', 'str imult zero'

# === String join method ===
# Basic join on literals
assert ','.join(['a', 'b', 'c']) == 'a,b,c', 'join list with comma'
assert ''.join(['a', 'b', 'c']) == 'abc', 'join with empty separator'
assert '-'.join([]) == '', 'join empty list'
assert ','.join(['only']) == 'only', 'join single element'

# Join with different iterables
assert ' '.join(('hello', 'world')) == 'hello world', 'join tuple'

# Join with string iterable (iterates over characters)
assert ','.join('abc') == 'a,b,c', 'join string iterable'

# Join with variable separator
sep = '-'
assert sep.join(['a', 'b']) == 'a-b', 'join with variable separator'

# Heap-allocated string separator
s = str('.')
assert s.join(['a', 'b']) == 'a.b', 'join with heap string'

# Mixed string types in iterable (interned and heap)
mixed = ['hello', str('world')]
assert ' '.join(mixed) == 'hello world', 'join with mixed string types'

# === String indexing (getitem) ===
# Basic indexing
assert 'hello'[0] == 'h', 'getitem index 0'
assert 'hello'[1] == 'e', 'getitem index 1'
assert 'hello'[4] == 'o', 'getitem last index'

# Negative indexing
assert 'hello'[-1] == 'o', 'getitem -1'
assert 'hello'[-2] == 'l', 'getitem -2'
assert 'hello'[-5] == 'h', 'getitem -5'

# Single character strings
assert 'a'[0] == 'a', 'getitem single char at 0'
assert 'a'[-1] == 'a', 'getitem single char at -1'

# Unicode strings
s = 'café'
assert s[0] == 'c', 'unicode getitem 0'
assert s[1] == 'a', 'unicode getitem 1'
assert s[2] == 'f', 'unicode getitem 2'
assert s[3] == 'é', 'unicode getitem 3 (accented)'
assert s[-1] == 'é', 'unicode getitem -1'

# Multi-byte unicode (CJK characters)
s = '日本語'
assert s[0] == '日', 'cjk getitem 0'
assert s[1] == '本', 'cjk getitem 1'
assert s[2] == '語', 'cjk getitem 2'
assert s[-1] == '語', 'cjk getitem -1'

# Emoji (multi-byte UTF-8)
s = 'a🎉b'
assert s[0] == 'a', 'emoji string getitem 0'
assert s[1] == '🎉', 'emoji string getitem 1 (emoji)'
assert s[2] == 'b', 'emoji string getitem 2'

# Heap-allocated strings
s = str('hello')
assert s[0] == 'h', 'heap string getitem'
assert s[-1] == 'o', 'heap string negative getitem'

# Variable index
s = 'abc'
i = 1
assert s[i] == 'b', 'getitem with variable index'

# Bool indices (True=1, False=0)
s = 'abc'
assert s[False] == 'a', 'str getitem with False'
assert s[True] == 'b', 'str getitem with True'
