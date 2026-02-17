# === Empty tuple identity (singleton optimization) ===
# In Python, () is () is always True because empty tuples are interned
assert () is (), 'empty tuple identity'
assert tuple() is (), 'tuple() is empty tuple'
assert tuple() is tuple(), 'tuple() identity'
a = ()
b = ()
assert a is b, 'empty tuple vars are same object'
# Empty tuple from operations
assert (1,)[1:] is (), 'slice to empty is singleton'
assert (1, 2) * 0 is (), 'mult by 0 is empty singleton'

# === Tuple length ===
assert len(()) == 0, 'len empty'
assert len((1,)) == 1, 'len single'
assert len((1, 2, 3)) == 3, 'len basic'

# === Tuple indexing ===
a = (1, 2, 3)
assert a[1] == 2, 'getitem basic'

a = ('a', 'b', 'c')
assert a[0 - 2] == 'b', 'getitem negative'
assert a[-1] == 'c', 'getitem -1'

# === Nested tuples ===
assert ((1, 2), (3, 4)) == ((1, 2), (3, 4)), 'nested tuple'

# === Tuple repr/str ===
assert repr((1, 2)) == '(1, 2)', 'tuple repr'
assert str((1, 2)) == '(1, 2)', 'tuple str'

# === Tuple concatenation (+) ===
assert (1, 2) + (3, 4) == (1, 2, 3, 4), 'tuple add basic'
assert () + (1, 2) == (1, 2), 'empty add tuple'
assert (1, 2) + () == (1, 2), 'tuple add empty'
assert () + () == (), 'empty add empty'
assert ('a', 'b') + ('c',) == ('a', 'b', 'c'), 'tuple add strings'
assert ((1, 2),) + ((3, 4),) == ((1, 2), (3, 4)), 'tuple add nested'

# === Tuple repetition (*) ===
assert (1, 2) * 3 == (1, 2, 1, 2, 1, 2), 'tuple mult int'
assert 3 * (1, 2) == (1, 2, 1, 2, 1, 2), 'int mult tuple'
assert (1,) * 0 == (), 'tuple mult zero'
assert (1,) * -1 == (), 'tuple mult negative'
assert () * 5 == (), 'empty tuple mult'
assert (1, 2) * 1 == (1, 2), 'tuple mult one'

# === tuple() constructor ===
assert tuple() == (), 'tuple() empty'
assert tuple([1, 2, 3]) == (1, 2, 3), 'tuple from list'
assert tuple((1, 2, 3)) == (1, 2, 3), 'tuple from tuple'
assert tuple(range(3)) == (0, 1, 2), 'tuple from range'
assert tuple('abc') == ('a', 'b', 'c'), 'tuple from string'
assert tuple(b'abc') == (97, 98, 99), 'tuple from bytes'
assert tuple({'a': 1, 'b': 2}) == ('a', 'b'), 'tuple from dict yields keys'

# non-ASCII strings (multi-byte UTF-8)
assert tuple('héllo') == ('h', 'é', 'l', 'l', 'o'), 'tuple from string with accented char'
assert tuple('日本') == ('日', '本'), 'tuple from string with CJK chars'
assert tuple('a🎉b') == ('a', '🎉', 'b'), 'tuple from string with emoji'
