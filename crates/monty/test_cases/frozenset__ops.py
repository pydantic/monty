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
