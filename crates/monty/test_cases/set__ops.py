# === Construction ===
s = set()
assert len(s) == 0
assert s == set()

s = set([1, 2, 3])
assert len(s) == 3

# === Basic Methods ===
s = set()
s.add(1)
s.add(2)
s.add(1)  # duplicate
assert len(s) == 2

# === Discard and Remove ===
s = set([1, 2, 3])
s.discard(2)
assert len(s) == 2
s.discard(99)  # should not raise
assert len(s) == 2

# === Pop ===
s = set([1])
v = s.pop()
assert v == 1
assert len(s) == 0

# === Clear ===
s = set([1, 2, 3])
s.clear()
assert len(s) == 0

# === Copy ===
s = set([1, 2, 3])
s2 = s.copy()
assert s == s2
s.add(4)
assert s != s2

# === Update ===
s = set([1, 2])
s.update([2, 3, 4])
assert len(s) == 4

# === Union ===
s1 = set([1, 2])
s2 = set([2, 3])
u = s1.union(s2)
assert len(u) == 3

# === Intersection ===
s1 = set([1, 2, 3])
s2 = set([2, 3, 4])
i = s1.intersection(s2)
assert len(i) == 2

# === Difference ===
s1 = set([1, 2, 3])
s2 = set([2, 3, 4])
d = s1.difference(s2)
assert len(d) == 1

# === Symmetric Difference ===
s1 = set([1, 2, 3])
s2 = set([2, 3, 4])
sd = s1.symmetric_difference(s2)
assert len(sd) == 2

# === Binary operators ===
s = {1, 2}
t = {2, 3}
fs = frozenset([2, 3])

assert s & t == {2}
assert s | t == {1, 2, 3}
assert s ^ t == {1, 3}
assert s - t == {1}

assert s & fs == {2}
assert s | fs == {1, 2, 3}
assert s ^ fs == {1, 3}
assert s - fs == {1}

keys = {'a': 1, 'b': 2}.keys()
items = {'a': 1, 'b': 2}.items()
assert {'a'} & keys == {'a'}
assert {'a'} | keys == {'a', 'b'}
assert {('a', 1)} ^ items == {('b', 2)}
assert {('a', 1), ('b', 2)} - items == set()

assert type(s & fs).__name__ == 'set'

try:
    s & [1, 2]
    assert False, 'set operators reject non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for &: 'set' and 'list'"

try:
    s | [1, 2]
    assert False, 'set union operator rejects non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for |: 'set' and 'list'"

try:
    s ^ [1, 2]
    assert False, 'set xor operator rejects non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for ^: 'set' and 'list'"

try:
    s - [1, 2]
    assert False, 'set subtraction operator rejects non-set rhs'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for -: 'set' and 'list'"

# === Issubset ===
s1 = set([1, 2])
s2 = set([1, 2, 3])
assert s1.issubset(s2) == True
assert s2.issubset(s1) == False
# non-Ref iterable argument (range) must not raise
assert set([1, 2, 3]).issubset(range(10)) == True

# === Issuperset ===
s1 = set([1, 2, 3])
s2 = set([1, 2])
assert s1.issuperset(s2) == True
assert s2.issuperset(s1) == False
assert set([1, 2, 3]).issuperset(range(1, 3)) == True

# === Isdisjoint ===
s1 = set([1, 2])
s2 = set([3, 4])
s3 = set([2, 3])
assert s1.isdisjoint(s2) == True
assert s1.isdisjoint(s3) == False
assert set([1, 2, 3]).isdisjoint(range(10, 20)) == True

# === Bool ===
assert bool(set()) == False
assert bool(set([1])) == True

# === repr ===
assert repr(set()) == 'set()'
# non-empty set repr has no type prefix; frozenset repr does
assert repr({1, 2}) == '{1, 2}' or repr({1, 2}) == '{2, 1}', 'set repr should not have a type prefix'
assert repr(frozenset()) == 'frozenset()'
fs_repr = repr(frozenset({1, 2}))
assert fs_repr == 'frozenset({1, 2})' or fs_repr == 'frozenset({2, 1})', 'frozenset repr should include the type name'

# === Construction with nested heap objects ===
# The temporary list argument is dropped after construction; a missed refcount
# increment on the nested tuple would corrupt these.
assert repr(set([(1, 2)])) == '{(1, 2)}'
assert repr(set([(3, 4)])) == '{(3, 4)}'
assert repr(frozenset([(5, 6)])) == 'frozenset({(5, 6)})'

# === Set literals ===
s = {1, 2, 3}
assert len(s) == 3

s = {1, 1, 2, 2, 3}
assert len(s) == 3

# Set literal with expressions
x = 5
s = {x, x + 1, x + 2}
assert len(s) == 3

# === Set unpacking (PEP 448) ===
a = [1, 2]
b = [3, 4]
assert {*a} == {1, 2}
assert {*a, *b} == {1, 2, 3, 4}
assert {0, *a, 5} == {0, 1, 2, 5}
assert {*[]} == set()
assert {*(1, 2)} == {1, 2}
assert {*{'a': 1, 'b': 2}} == {'a', 'b'}
assert {*'aab'} == {'a', 'b'}
# Heap-allocated set: covers the HeapData::Set arm in set_extend
inner_set = {1, 2, 3}
assert {*inner_set} == {1, 2, 3}
# Heap-allocated Str (result of concat, not interned): covers HeapData::Str in set_extend
hs = 'hel' + 'lo'
assert {*hs} == {'h', 'e', 'l', 'o'}


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
