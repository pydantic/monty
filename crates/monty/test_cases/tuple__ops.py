# === Empty tuple identity (singleton optimization) ===
# In Python, () is () is always True because empty tuples are interned
assert () is ()
assert tuple() is ()
assert tuple() is tuple()
a = ()
b = ()
assert a is b
# Empty tuple from operations
assert (1,)[1:] is ()
assert (1, 2) * 0 is ()

# === Tuple length ===
assert len(()) == 0
assert len((1,)) == 1
assert len((1, 2, 3)) == 3

# === Tuple indexing ===
a = (1, 2, 3)
assert a[1] == 2

a = ('a', 'b', 'c')
assert a[0 - 2] == 'b'
assert a[-1] == 'c'

# === Nested tuples ===
assert ((1, 2), (3, 4)) == ((1, 2), (3, 4))

# === Tuple repr/str ===
assert repr((1, 2)) == '(1, 2)'
assert str((1, 2)) == '(1, 2)'

# === Tuple concatenation (+) ===
assert (1, 2) + (3, 4) == (1, 2, 3, 4)
assert () + (1, 2) == (1, 2)
assert (1, 2) + () == (1, 2)
assert () + () == ()
assert ('a', 'b') + ('c',) == ('a', 'b', 'c')
assert ((1, 2),) + ((3, 4),) == ((1, 2), (3, 4))

# === Tuple repetition (*) ===
assert (1, 2) * 3 == (1, 2, 1, 2, 1, 2)
assert 3 * (1, 2) == (1, 2, 1, 2, 1, 2)
assert (1,) * 0 == ()
assert (1,) * -1 == ()
assert () * 5 == ()
assert (1, 2) * 1 == (1, 2)

# === Tuple augmented assignment edge cases ===
t = ([1],)
try:
    t[0] += [2]
    assert False, 'tuple item augmented assignment should fail'
except TypeError as e:
    assert e.args == ("'tuple' object does not support item assignment",)
    assert t == ([1, 2],)

# === tuple() constructor ===
assert tuple() == ()
assert tuple([1, 2, 3]) == (1, 2, 3)
assert tuple((1, 2, 3)) == (1, 2, 3)
assert tuple(range(3)) == (0, 1, 2)
assert tuple('abc') == ('a', 'b', 'c')
assert tuple(b'abc') == (97, 98, 99)
assert tuple({'a': 1, 'b': 2}) == ('a', 'b')

# non-ASCII strings (multi-byte UTF-8)
assert tuple('héllo') == ('h', 'é', 'l', 'l', 'o')
assert tuple('日本') == ('日', '本')
assert tuple('a🎉b') == ('a', '🎉', 'b')

# === Tuple unpacking (PEP 448) ===
a = (1, 2)
b = (3, 4)
assert (*a,) == (1, 2)
assert (*a, *b) == (1, 2, 3, 4)
assert (0, *a, 5) == (0, 1, 2, 5)
assert (*(),) == ()
assert (*[1, 2],) == (1, 2)

# === Tuple comparison (<, >, <=, >=) ===
assert (1, 2) < (1, 3)
assert (1,) < (2,)
assert () < (1,)
assert (1, 2) < (1, 2, 3)
assert not (1, 2) < (1, 2), 'not lt when equal'
assert not (1, 3) < (1, 2), 'not lt when greater'

assert (1, 3) > (1, 2)
assert (2,) > (1,)
assert (1,) > ()
assert (1, 2, 3) > (1, 2)
assert not (1, 2) > (1, 2), 'not gt when equal'

assert (1, 2) <= (1, 2)
assert (1, 2) <= (1, 3)
assert not (1, 3) <= (1, 2), 'not le when greater'

assert (1, 2) >= (1, 2)
assert (1, 3) >= (1, 2)
assert not (1, 2) >= (1, 3), 'not ge when less'

# === Tuple comparison with sorted() ===
assert sorted([(2, 'b'), (1, 'a')]) == [(1, 'a'), (2, 'b')]
assert sorted([(1, 'b'), (1, 'a')]) == [(1, 'a'), (1, 'b')]
assert sorted([(3,), (1,), (2,)]) == [(1,), (2,), (3,)]

# === Nested tuple comparison ===
assert ((1, 2), 3) < ((1, 3), 2)
assert (1, (2, 3)) < (1, (2, 4))

# === Equal-but-unorderable elements (None, lists, dicts) ===
# CPython checks __eq__ first; equal elements skip ordering comparison
assert not (1, None) < (1, None), 'equal None elements not lt'
assert (1, None) <= (1, None)
assert (1, None) >= (1, None)
assert not (1, None) > (1, None), 'equal None elements not gt'
assert (1, None) < (2, None)
assert (1, [1, 2]) <= (1, [1, 2])

# === Mixed types in tuple comparison ===
assert (1,) < (2.0,)
assert (1.0,) < (2,)
assert (True,) < (2,)
assert (False,) < (True,)
assert (1, 'a') < (1, 'b')
assert ('a', 1) < ('b', 1)

# === Empty and equal tuples ===
assert not () < (), 'empty tuples not lt'
assert () <= ()
assert () >= ()
assert not () > (), 'empty tuples not gt'

# === Tuple reprs ===
assert repr(()) == '()'
assert repr((1,)) == '(1,)'
assert repr((1, 2)) == '(1, 2)'
assert repr((1, (2, 3))) == '(1, (2, 3))'
