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

# === Tuple augmented assignment edge cases ===
t = ([1],)
try:
    t[0] += [2]
    assert False, 'tuple item augmented assignment should fail'
except TypeError as e:
    assert e.args == ("'tuple' object does not support item assignment",), 'tuple += error matches CPython'
    assert t == ([1, 2],), 'inner list mutation happens before tuple store fails'

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

# === Tuple comparison (<, >, <=, >=) ===
assert (1, 2) < (1, 3), 'lt second element differs'
assert (1,) < (2,), 'lt single element'
assert () < (1,), 'lt empty vs non-empty'
assert (1, 2) < (1, 2, 3), 'lt shorter tuple'
assert not (1, 2) < (1, 2), 'not lt when equal'
assert not (1, 3) < (1, 2), 'not lt when greater'

assert (1, 3) > (1, 2), 'gt second element'
assert (2,) > (1,), 'gt single element'
assert (1,) > (), 'gt non-empty vs empty'
assert (1, 2, 3) > (1, 2), 'gt longer tuple'
assert not (1, 2) > (1, 2), 'not gt when equal'

assert (1, 2) <= (1, 2), 'le when equal'
assert (1, 2) <= (1, 3), 'le when less'
assert not (1, 3) <= (1, 2), 'not le when greater'

assert (1, 2) >= (1, 2), 'ge when equal'
assert (1, 3) >= (1, 2), 'ge when greater'
assert not (1, 2) >= (1, 3), 'not ge when less'

# === Tuple comparison with sorted() ===
assert sorted([(2, 'b'), (1, 'a')]) == [(1, 'a'), (2, 'b')], 'sorted tuples'
assert sorted([(1, 'b'), (1, 'a')]) == [(1, 'a'), (1, 'b')], 'sorted tuples second element'
assert sorted([(3,), (1,), (2,)]) == [(1,), (2,), (3,)], 'sorted single-element tuples'

# === Nested tuple comparison ===
assert ((1, 2), 3) < ((1, 3), 2), 'nested tuple comparison'
assert (1, (2, 3)) < (1, (2, 4)), 'nested tuple inner comparison'

# === Equal-but-unorderable elements (None, lists, dicts) ===
# CPython checks __eq__ first; equal elements skip ordering comparison
assert not (1, None) < (1, None), 'equal None elements not lt'
assert (1, None) <= (1, None), 'equal None elements le'
assert (1, None) >= (1, None), 'equal None elements ge'
assert not (1, None) > (1, None), 'equal None elements not gt'
assert (1, None) < (2, None), 'first element resolves before None'
assert (1, [1, 2]) <= (1, [1, 2]), 'equal list elements le'

# === Mixed types in tuple comparison ===
assert (1,) < (2.0,), 'int vs float in tuple'
assert (1.0,) < (2,), 'float vs int in tuple'
assert (True,) < (2,), 'bool vs int in tuple'
assert (False,) < (True,), 'False vs True in tuple'
assert (1, 'a') < (1, 'b'), 'string comparison in tuple'
assert ('a', 1) < ('b', 1), 'string first element in tuple'

# === Empty and equal tuples ===
assert not () < (), 'empty tuples not lt'
assert () <= (), 'empty tuples le'
assert () >= (), 'empty tuples ge'
assert not () > (), 'empty tuples not gt'
