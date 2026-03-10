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

# === Set unpacking (PEP 448) ===
a = [1, 2]
b = [3, 4]
assert {*a} == {1, 2}, 'single set unpack from list'
assert {*a, *b} == {1, 2, 3, 4}, 'double set unpack'
assert {0, *a, 5} == {0, 1, 2, 5}, 'mixed set unpack'
assert {*[]} == set(), 'unpack empty into set'
assert {*(1, 2)} == {1, 2}, 'unpack tuple into set'
assert {*{'a': 1, 'b': 2}} == {'a', 'b'}, 'unpack dict keys into set'
assert {*'aab'} == {'a', 'b'}, 'unpack string into set'
# Heap-allocated set: covers the HeapData::Set arm in set_extend
inner_set = {1, 2, 3}
assert {*inner_set} == {1, 2, 3}, 'unpack set into set'
# Heap-allocated Str (result of concat, not interned): covers HeapData::Str in set_extend
hs = 'hel' + 'lo'
assert {*hs} == {'h', 'e', 'l', 'o'}, 'unpack heap string into set'


# Non-iterable heap-allocated Ref (closure) hits the inner `_` arm in set_extend.
# A plain top-level function is Value::DefFunction (not a Ref), so a closure is
# required to reach the Value::Ref(_) branch (HeapData that is not List/Tuple/Set/Dict/Str).
def _make_set_unpack_closure():
    _sentinel = 1

    def _inner():
        return _sentinel

    return _inner


_set_unpack_closure = _make_set_unpack_closure()
try:
    _x = {*_set_unpack_closure}
    assert False, 'expected TypeError for non-iterable heap closure in set unpack'
except TypeError:
    pass
