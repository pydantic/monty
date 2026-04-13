# === Construction ===
fs = frozenset()
assert len(fs) == 0, 'empty frozenset len'
assert fs == frozenset(), 'empty frozenset equality'

fs = frozenset([1, 2, 3])
assert len(fs) == 3, 'frozenset from list len'

# === Copy ===
fs = frozenset([1, 2, 3])
fs2 = fs.copy()
assert fs == fs2, 'copy equality'

# === Union ===
fs1 = frozenset([1, 2])
fs2 = frozenset([2, 3])
u = fs1.union(fs2)
assert len(u) == 3, 'union len'

# === Intersection ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([2, 3, 4])
i = fs1.intersection(fs2)
assert len(i) == 2, 'intersection len'

# === Difference ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([2, 3, 4])
d = fs1.difference(fs2)
assert len(d) == 1, 'difference len'

# === Symmetric Difference ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([2, 3, 4])
sd = fs1.symmetric_difference(fs2)
assert len(sd) == 2, 'symmetric_difference len'

# === Binary operators ===
fs = frozenset([1, 2])
other_fs = frozenset([2, 3])
s = {2, 3}

assert fs & other_fs == frozenset([2]), 'frozenset & frozenset works'
assert fs | other_fs == frozenset([1, 2, 3]), 'frozenset | frozenset works'
assert fs ^ other_fs == frozenset([1, 3]), 'frozenset ^ frozenset works'
assert fs - other_fs == frozenset([1]), 'frozenset - frozenset works'

assert fs & s == frozenset([2]), 'frozenset & set works'
assert fs | s == frozenset([1, 2, 3]), 'frozenset | set works'
assert fs ^ s == frozenset([1, 3]), 'frozenset ^ set works'
assert fs - s == frozenset([1]), 'frozenset - set works'

keys = {'a': 1, 'b': 2}.keys()
items = {'a': 1, 'b': 2}.items()
assert frozenset({'a'}) & keys == frozenset({'a'}), 'frozenset & dict_keys works'
assert frozenset({'a'}) | keys == frozenset({'a', 'b'}), 'frozenset | dict_keys works'
assert frozenset({('a', 1)}) ^ items == frozenset({('b', 2)}), 'frozenset ^ dict_items works'
assert frozenset({('a', 1), ('b', 2)}) - items == frozenset(), 'frozenset - dict_items works'

assert type(fs | s).__name__ == 'frozenset', 'frozenset operators keep the left operand type'

try:
    fs & [1, 2]
    assert False, 'frozenset operators reject non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for &: 'frozenset' and 'list'", (
        'frozenset & rhs error matches CPython'
    )

# === Issubset ===
fs1 = frozenset([1, 2])
fs2 = frozenset([1, 2, 3])
assert fs1.issubset(fs2) == True, 'issubset true'
assert fs2.issubset(fs1) == False, 'issubset false'

# === Issuperset ===
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([1, 2])
assert fs1.issuperset(fs2) == True, 'issuperset true'
assert fs2.issuperset(fs1) == False, 'issuperset false'

# === Isdisjoint ===
fs1 = frozenset([1, 2])
fs2 = frozenset([3, 4])
fs3 = frozenset([2, 3])
assert fs1.isdisjoint(fs2) == True, 'isdisjoint true'
assert fs1.isdisjoint(fs3) == False, 'isdisjoint false'

# === Bool ===
assert bool(frozenset()) == False, 'empty frozenset is falsy'
assert bool(frozenset([1])) == True, 'non-empty frozenset is truthy'

# === repr ===
assert repr(frozenset()) == 'frozenset()', 'empty frozenset repr'

# === Hashing ===
fs = frozenset([1, 2, 3])
h = hash(fs)
assert isinstance(h, int), 'frozenset hash is int'

# Same elements should have same hash
fs1 = frozenset([1, 2, 3])
fs2 = frozenset([3, 2, 1])  # Different order
assert hash(fs1) == hash(fs2), 'frozenset hash is order-independent'

# === As dict key ===
d = {}
fs = frozenset([1, 2])
d[fs] = 'value'
assert d[fs] == 'value', 'frozenset as dict key'
assert d[frozenset([2, 1])] == 'value', 'frozenset key lookup order-independent'

# === Construction from various iterables ===
fs = frozenset('abc')
assert len(fs) == 3, 'frozenset from string len'
assert 'a' in fs and 'b' in fs and 'c' in fs, 'frozenset from string elements'

fs = frozenset((1, 2, 3))
assert fs == frozenset({1, 2, 3}), 'frozenset from tuple'

fs = frozenset(range(5))
assert fs == frozenset({0, 1, 2, 3, 4}), 'frozenset from range'

fs = frozenset({1, 2, 3})
assert len(fs) == 3, 'frozenset from set'

# === Containment (in / not in) ===
fs = frozenset({1, 2, 3})
assert 1 in fs, 'in frozenset positive'
assert 4 not in fs, 'not in frozenset'
assert 'x' not in frozenset({'a', 'b'}), 'not in frozenset strings'

# === Iteration ===
result = []
for x in frozenset({1, 2, 3}):
    result.append(x)
assert len(result) == 3, 'frozenset iteration length'
assert set(result) == {1, 2, 3}, 'frozenset iteration elements'

result = []
for x in frozenset():
    result.append(x)
assert result == [], 'empty frozenset iteration'

# === Inequality (!=) ===
assert frozenset({1, 2}) != frozenset({1, 3}), 'frozenset ne different'
assert not (frozenset({1, 2}) != frozenset({1, 2})), 'frozenset ne same'

# === Methods accepting iterables ===
assert frozenset({1, 2}).union([3, 4]) == frozenset({1, 2, 3, 4}), 'union with list arg'
assert frozenset({1, 2, 3}).intersection([2, 3, 4]) == frozenset({2, 3}), 'intersection with list arg'
assert frozenset({1, 2, 3}).difference([2]) == frozenset({1, 3}), 'difference with list arg'
assert frozenset({1, 2}).symmetric_difference([2, 3]) == frozenset({1, 3}), 'symmetric_difference with list arg'
assert frozenset({1}).union(range(3)) == frozenset({0, 1, 2}), 'union with range arg'
assert frozenset({1}).union((2, 3)) == frozenset({1, 2, 3}), 'union with tuple arg'

# === issubset/issuperset/isdisjoint with non-set iterables ===
fs = frozenset({1, 2, 3})
assert fs.issubset(range(10)), 'issubset with range'
assert fs.issuperset([1, 2]), 'issuperset with list'
assert fs.isdisjoint([4, 5, 6]), 'isdisjoint with list'
assert not fs.isdisjoint([3, 4]), 'not isdisjoint with list'

# === Different hashes for different frozensets ===
fs1 = frozenset({1, 2})
fs2 = frozenset({3, 4})
# Not guaranteed to be different, but very likely
# Instead just verify they're integers and stable
assert hash(fs1) == hash(frozenset({2, 1})), 'hash stable across order'
assert hash(frozenset()) == hash(frozenset()), 'empty frozenset hash stable'

# === Frozenset as set element ===
s = {frozenset({1, 2}), frozenset({3, 4})}
assert len(s) == 2, 'set of frozensets'
assert frozenset({1, 2}) in s, 'frozenset element lookup'
# Duplicate frozenset should dedup
s2 = {frozenset({1}), frozenset({1})}
assert len(s2) == 1, 'duplicate frozensets dedup in set'
