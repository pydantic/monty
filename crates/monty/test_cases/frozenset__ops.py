# === Construction ===
fs = frozenset()
assert len(fs) == 0
assert fs == frozenset()

fs = frozenset([1, 2, 3])
assert len(fs) == 3

# === Copy ===
fs = frozenset([1, 2, 3])
fs2 = fs.copy()
assert fs == fs2

# === Union ===
fs1 = frozenset([1, 2])
fs2 = frozenset([2, 3])
u = fs1.union(fs2)
assert len(u) == 3

# === Intersection ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([2, 3, 4])
i = fs1.intersection(fs2)
assert len(i) == 2

# === Difference ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([2, 3, 4])
d = fs1.difference(fs2)
assert len(d) == 1

# === Symmetric Difference ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([2, 3, 4])
sd = fs1.symmetric_difference(fs2)
assert len(sd) == 2

# === Binary operators ===
fs = frozenset([1, 2])
other_fs = frozenset([2, 3])
s = {2, 3}

assert fs & other_fs == frozenset([2])
assert fs | other_fs == frozenset([1, 2, 3])
assert fs ^ other_fs == frozenset([1, 3])
assert fs - other_fs == frozenset([1])

assert fs & s == frozenset([2])
assert fs | s == frozenset([1, 2, 3])
assert fs ^ s == frozenset([1, 3])
assert fs - s == frozenset([1])

keys = {'a': 1, 'b': 2}.keys()
items = {'a': 1, 'b': 2}.items()
assert frozenset({'a'}) & keys == frozenset({'a'})
assert frozenset({'a'}) | keys == frozenset({'a', 'b'})
assert frozenset({('a', 1)}) ^ items == frozenset({('b', 2)})
assert frozenset({('a', 1), ('b', 2)}) - items == frozenset()

assert type(fs | s).__name__ == 'frozenset'

try:
    fs & [1, 2]
    assert False, 'frozenset operators reject non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for &: 'frozenset' and 'list'"

# === Issubset ===
fs1 = frozenset([1, 2])
fs2 = frozenset([1, 2, 3])
assert fs1.issubset(fs2) is True
assert fs2.issubset(fs1) is False

# === Issuperset ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([1, 2])
assert fs1.issuperset(fs2) is True
assert fs2.issuperset(fs1) is False

# === Isdisjoint ===
fs1 = frozenset([1, 2])
fs2 = frozenset([3, 4])
fs3 = frozenset([2, 3])
assert fs1.isdisjoint(fs2) is True
assert fs1.isdisjoint(fs3) is False

# === Bool ===
assert bool(frozenset()) == False
assert bool(frozenset([1])) == True

# === repr ===
assert repr(frozenset()) == 'frozenset()'

# === Hashing ===
fs = frozenset([1, 2, 3])
h = hash(fs)
assert isinstance(h, int)

# Same elements should have same hash
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([3, 2, 1])  # Different order
assert hash(fs1) == hash(fs2)

# === As dict key ===
d = {}
fs = frozenset([1, 2])
d[fs] = 'value'
assert d[fs] == 'value'
assert d[frozenset([2, 1])] == 'value'

# === Construction from various iterables ===
fs = frozenset('abc')
assert len(fs) == 3
assert 'a' in fs and 'b' in fs and 'c' in fs, 'frozenset from string elements'

fs = frozenset((1, 2, 3))
assert fs == frozenset({1, 2, 3})

fs = frozenset(range(5))
assert fs == frozenset({0, 1, 2, 3, 4})

fs = frozenset({1, 2, 3})
assert len(fs) == 3

# === Containment (in / not in) ===
fs = frozenset({1, 2, 3})
assert 1 in fs
assert 4 not in fs
assert 'x' not in frozenset({'a', 'b'})

# === Iteration ===
result = []
for x in frozenset({1, 2, 3}):
    result.append(x)
assert len(result) == 3
assert set(result) == {1, 2, 3}

result = []
for x in frozenset():
    result.append(x)
assert result == []

# === Inequality (!=) ===
assert frozenset({1, 2}) != frozenset({1, 3})
assert not (frozenset({1, 2}) != frozenset({1, 2})), 'frozenset ne same'

# === Methods accepting iterables ===
assert frozenset({1, 2}).union([3, 4]) == frozenset({1, 2, 3, 4})
assert frozenset({1, 2, 3}).intersection([2, 3, 4]) == frozenset({2, 3})
assert frozenset({1, 2, 3}).difference([2]) == frozenset({1, 3})
assert frozenset({1, 2}).symmetric_difference([2, 3]) == frozenset({1, 3})
assert frozenset({1}).union(range(3)) == frozenset({0, 1, 2})
assert frozenset({1}).union((2, 3)) == frozenset({1, 2, 3})

# === issubset/issuperset/isdisjoint with non-set iterables ===
fs = frozenset({1, 2, 3})
assert fs.issubset(range(10))
assert fs.issuperset([1, 2])
assert fs.isdisjoint([4, 5, 6])
assert not fs.isdisjoint([3, 4]), 'not isdisjoint with list'

# === Different hashes for different frozensets ===
fs1 = frozenset({1, 2})
fs2 = frozenset({3, 4})
# Not guaranteed to be different, but very likely
# Instead just verify they're integers and stable
assert hash(fs1) == hash(frozenset({2, 1}))
assert hash(frozenset()) == hash(frozenset())

# === Frozenset as set element ===
s = {frozenset({1, 2}), frozenset({3, 4})}
assert len(s) == 2
assert frozenset({1, 2}) in s
# Duplicate frozenset should dedup
s2 = {frozenset({1}), frozenset({1})}
assert len(s2) == 1

# === set <-> frozenset cross-type equality (compare by members) ===
assert frozenset({1, 2, 3}) == {1, 2, 3}
assert {1, 2, 3} == frozenset({1, 2, 3})
assert frozenset({1, 2}) != {1, 2, 3}
assert {1, 2, 3} != frozenset({1, 2})
assert frozenset() == set()
assert set() == frozenset()
assert {1: 'a', 2: 'b'}.keys() == frozenset({1, 2})
