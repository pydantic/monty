# === Construction ===
s = set()
assert len(s) == 0, 'empty set len'
assert s == set(), 'empty set equality'

s = set([1, 2, 3])
assert len(s) == 3, 'set from list len'

# === Basic Methods ===
s = set()
s.add(1)
s.add(2)
s.add(1)  # duplicate
assert len(s) == 2, 'add with duplicate'

# === Discard and Remove ===
s = set([1, 2, 3])
s.discard(2)
assert len(s) == 2, 'discard existing'
s.discard(99)  # should not raise
assert len(s) == 2, 'discard non-existing'

# === Pop ===
s = set([1])
v = s.pop()
assert v == 1, 'pop returns element'
assert len(s) == 0, 'pop removes element'

# === Clear ===
s = set([1, 2, 3])
s.clear()
assert len(s) == 0, 'clear'

# === Copy ===
s = set([1, 2, 3])
s2 = s.copy()
assert s == s2, 'copy equality'
s.add(4)
assert s != s2, 'copy is independent'

# === Update ===
s = set([1, 2])
s.update([2, 3, 4])
assert len(s) == 4, 'update with list'

# === Union ===
s1 = set([1, 2])
s2 = set([2, 3])
u = s1.union(s2)
assert len(u) == 3, 'union len'

# === Intersection ===
s1 = set([1, 2, 3])
s2 = set([2, 3, 4])
i = s1.intersection(s2)
assert len(i) == 2, 'intersection len'

# === Difference ===
s1 = set([1, 2, 3])
s2 = set([2, 3, 4])
d = s1.difference(s2)
assert len(d) == 1, 'difference len'

# === Symmetric Difference ===
s1 = set([1, 2, 3])
s2 = set([2, 3, 4])
sd = s1.symmetric_difference(s2)
assert len(sd) == 2, 'symmetric_difference len'

# === Binary operators ===
s = {1, 2}
t = {2, 3}
fs = frozenset([2, 3])

assert s & t == {2}, 'set & set works'
assert s | t == {1, 2, 3}, 'set | set works'
assert s ^ t == {1, 3}, 'set ^ set works'
assert s - t == {1}, 'set - set works'

assert s & fs == {2}, 'set & frozenset works'
assert s | fs == {1, 2, 3}, 'set | frozenset works'
assert s ^ fs == {1, 3}, 'set ^ frozenset works'
assert s - fs == {1}, 'set - frozenset works'

keys = {'a': 1, 'b': 2}.keys()
items = {'a': 1, 'b': 2}.items()
assert {'a'} & keys == {'a'}, 'set & dict_keys works'
assert {'a'} | keys == {'a', 'b'}, 'set | dict_keys works'
assert {('a', 1)} ^ items == {('b', 2)}, 'set ^ dict_items works'
assert {('a', 1), ('b', 2)} - items == set(), 'set - dict_items works'

assert type(s & fs).__name__ == 'set', 'set operators keep the left operand type'

try:
    s & [1, 2]
    assert False, 'set operators reject non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for &: 'set' and 'list'", 'set & rhs error matches CPython'

try:
    s | [1, 2]
    assert False, 'set union operator rejects non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for |: 'set' and 'list'", 'set | rhs error matches CPython'

try:
    s ^ [1, 2]
    assert False, 'set xor operator rejects non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for ^: 'set' and 'list'", 'set ^ rhs error matches CPython'

try:
    s - [1, 2]
    assert False, 'set subtraction operator rejects non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for -: 'set' and 'list'", 'set - rhs error matches CPython'

# === Issubset ===
s1 = set([1, 2])
s2 = set([1, 2, 3])
assert s1.issubset(s2) == True, 'issubset true'
assert s2.issubset(s1) == False, 'issubset false'

# === Issuperset ===
s1 = set([1, 2, 3])
s2 = set([1, 2])
assert s1.issuperset(s2) == True, 'issuperset true'
assert s2.issuperset(s1) == False, 'issuperset false'

# === Isdisjoint ===
s1 = set([1, 2])
s2 = set([3, 4])
s3 = set([2, 3])
assert s1.isdisjoint(s2) == True, 'isdisjoint true'
assert s1.isdisjoint(s3) == False, 'isdisjoint false'

# === Bool ===
assert bool(set()) == False, 'empty set is falsy'
assert bool(set([1])) == True, 'non-empty set is truthy'

# === repr ===
assert repr(set()) == 'set()', 'empty set repr'

# === Set literals ===
s = {1, 2, 3}
assert len(s) == 3, 'set literal len'

s = {1, 1, 2, 2, 3}
assert len(s) == 3, 'set literal deduplication'

# Set literal with expressions
x = 5
s = {x, x + 1, x + 2}
assert len(s) == 3, 'set literal with expressions'
