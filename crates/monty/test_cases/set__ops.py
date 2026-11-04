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

# === In-place operators change the set itself ===
s = {1, 2, 3}
alias = s
s &= {2, 3, 4}
assert alias == {2, 3}
s |= frozenset({5})
assert alias == {2, 3, 5}
s -= {2}
assert alias == {3, 5}
s ^= {5, 6}
assert alias == {3, 6}
assert s is alias

# Only a `set` or `frozenset` updates in place: a `dict` view gives a new set, as
# the binary operator does, and anything else is an error.
s = {'a', 'c'}
alias = s
s |= {'b': 1, 'c': 2}.keys()
assert s == {'a', 'b', 'c'}
assert alias == {'a', 'c'}
keys = {'b': 1, 'c': 2}.keys()
s = alias = {'a', 'c'}
s &= keys
assert (s, alias) == ({'c'}, {'a', 'c'})
s = alias = {'a', 'c'}
s -= keys
assert (s, alias) == ({'a'}, {'a', 'c'})
s = alias = {'a', 'c'}
s ^= keys
assert (s, alias) == ({'a', 'b'}, {'a', 'c'})
for operand in ([1], 'ab', None):
    s = {1}
    try:
        s |= operand
        assert False, 'expected TypeError'
    except TypeError:
        pass
    assert s == {1}

# Other types without an in-place `^` fall back to the binary operator
x = 5
x ^= 3
assert x == 6
items = [1]
try:
    items ^= [2]
    assert False, 'expected TypeError'
except TypeError:
    pass

# A frozenset is immutable, so its in-place operators still build a new frozenset.
f = frozenset({1, 2})
alias = f
f |= {3}
assert f == frozenset({1, 2, 3})
assert alias == frozenset({1, 2})

# === intersection_update / difference_update / symmetric_difference_update ===
s = {1, 2, 3, 4}
alias = s
assert s.intersection_update([2, 3, 4, 5], (3, 4, 6)) is None
assert alias == {3, 4}
s.difference_update({4}, 'x', [7])
assert alias == {3}
s.symmetric_difference_update(range(2, 5))
assert alias == {2, 4}
s.intersection_update()
s.difference_update()
assert alias == {2, 4}
assert s is alias

# === update() takes any number of iterables ===
s = {1}
s.update([2], (3,), {4: 'x'}, range(5, 7))
assert s == {1, 2, 3, 4, 5, 6}
s.update()
assert s == {1, 2, 3, 4, 5, 6}

for compute, message in [
    (lambda: {1}.intersection_update([1], 2), "'int' object is not iterable"),
    (lambda: {1}.difference_update(None), "'NoneType' object is not iterable"),
    (
        lambda: {1}.symmetric_difference_update(),
        'set.symmetric_difference_update() takes exactly one argument (0 given)',
    ),
    (
        lambda: {1}.symmetric_difference_update([1], [2]),
        'set.symmetric_difference_update() takes exactly one argument (2 given)',
    ),
    (lambda: {1}.update([2], 3), "'int' object is not iterable"),
    (lambda: {1}.symmetric_difference_update(2), "'int' object is not iterable"),
    (lambda: {1}.update(x=1), 'set.update() takes no keyword arguments'),
    (lambda: {1}.intersection_update(x=1), 'set.intersection_update() takes no keyword arguments'),
    (lambda: {1}.difference_update(x=1), 'set.difference_update() takes no keyword arguments'),
]:
    try:
        compute()
        assert False, f'expected TypeError: {message}'
    except TypeError as e:
        assert str(e) == message

# On an error part way, intersection_update changes nothing (CPython swaps the
# result in at the end), while difference_update keeps the operands it applied.
s = {1, 2, 3}
try:
    s.intersection_update([1, 2], 5)
    assert False, 'expected TypeError'
except TypeError:
    pass
assert s == {1, 2, 3}
try:
    s.difference_update([1], 5)
    assert False, 'expected TypeError'
except TypeError:
    pass
assert s == {2, 3}


# update and difference_update apply each item as it arrives, so an iterator that
# raises part way keeps the items before the error.
class RaiseAtZero:
    def __init__(self, items):
        self.items = list(items)

    def __iter__(self):
        return self

    def __next__(self):
        item = self.items.pop(0)
        if item == 0:
            raise ValueError('stop')
        return item


s = {1, 2, 3}
try:
    s.update(RaiseAtZero([4, 5, 0]))
    assert False, 'expected ValueError'
except ValueError:
    pass
assert s == {1, 2, 3, 4, 5}
try:
    s.difference_update(RaiseAtZero([1, 2, 0]))
    assert False, 'expected ValueError'
except ValueError:
    pass
assert s == {3, 4, 5}
s.difference_update(s)
assert s == set()


# An `__iter__` that raises leaves the set unchanged
class RaisingIter:
    def __iter__(self):
        raise ValueError('iter')


s = {1, 2}
for update in [lambda: s.update(RaisingIter()), lambda: s.difference_update(RaisingIter())]:
    try:
        update()
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == 'iter'
assert s == {1, 2}


# An `__eq__` that raises on a hash collision propagates from every in-place form
class RaisingEq:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        raise ValueError('eq')


def ior(s, t):
    s |= t


def iand(s, t):
    s &= t


def isub(s, t):
    s -= t


def ixor(s, t):
    s ^= t


for update in [ior, iand, isub, ixor, lambda s, t: s.intersection_update(t)]:
    try:
        update({RaisingEq()}, {RaisingEq()})
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == 'eq'

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

# === `in` / `not in` ===
members = {1, 2, 3}
assert 2 in members
assert 9 not in members


# === an asymmetric __eq__ is always asked stored-value-first ===
# CPython's set lookup compares the stored element on the left, so a stored
# element claiming equality wins regardless of what the incoming one says.
class Asym:
    def __init__(self, result):
        self.result = result

    def __hash__(self):
        return 1

    def __eq__(self, other):
        return self.result


def _pair():
    return Asym(True), Asym(False)


yes, no = _pair()
assert (yes == no) is True
assert (no == yes) is False

# every construction route collapses the pair, not just `set.add`
yes, no = _pair()
seeded = {yes}
seeded.add(no)
assert len(seeded) == 1

yes, no = _pair()
assert len({yes, no}) == 1

yes, no = _pair()
assert len(set([yes, no])) == 1

yes, no = _pair()
assert len(frozenset([yes, no])) == 1

yes, no = _pair()
updated = {yes}
updated.update([no])
assert len(updated) == 1

# and the binary operators agree
yes, no = _pair()
assert len({yes} | {no}) == 1

yes, no = _pair()
assert len({yes} & {no}) == 1

# `&` walks the smaller side, and on a tie the right one, so that side's
# elements are the ones kept — visible through equal-but-distinct numbers
assert repr({1} & {1.0}) == '{1.0}'
assert repr({1.0} & {1}) == '{1}'
assert repr({1, 2} & {1.0}) == '{1.0}'
assert repr({1.0} & {1, 2}) == '{1.0}'

# === set-to-set algebra reuses each element's cached hash ===
# CPython takes the hash stored alongside an entry rather than calling
# `__hash__` again, so none of these operations run user hash code.
_hash_calls = []


class Counted:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        _hash_calls.append(self.n)
        return self.n

    def __eq__(self, other):
        return isinstance(other, Counted) and self.n == other.n


def _counted_pair():
    a = {Counted(1), Counted(2)}
    b = {Counted(2), Counted(3)}
    _hash_calls.clear()
    return a, b


a, b = _counted_pair()
assert len(a - b) == 1
assert _hash_calls == []

a, b = _counted_pair()
assert len(a & b) == 1
assert _hash_calls == []

a, b = _counted_pair()
assert len(a | b) == 3
assert _hash_calls == []

a, b = _counted_pair()
assert len(a ^ b) == 2
assert _hash_calls == []

a, b = _counted_pair()
assert a.issubset(b) is False
assert a.isdisjoint(b) is False
assert (a == b) is False
assert _hash_calls == []

a, b = _counted_pair()
a.update(b)
assert len(a) == 3
assert _hash_calls == []

a, b = _counted_pair()
assert len(set(a)) == 2
assert len(frozenset(a)) == 2
assert len(frozenset(a) - b) == 1
assert _hash_calls == []

# a frozenset source is copied the same way, hashes and all
a, b = _counted_pair()
frozen_a = frozenset(a)
_hash_calls.clear()
assert len(set(frozen_a)) == 2
assert len(frozenset(frozen_a)) == 2
assert len(frozen_a - b) == 1
assert _hash_calls == []

# the result holds the right element, and comparing two sets hashes nothing either
a = {Counted(1), Counted(2)}
b = {Counted(2), Counted(3)}
expected = {Counted(1)}
_hash_calls.clear()
assert (a - b) == expected
assert _hash_calls == []

# an arbitrary iterable on the right has no cached hashes, so it is hashed
a, b = _counted_pair()
assert len(a.difference(list(b))) == 1
assert _hash_calls == [2, 3]


# === a `__hash__` that mutates the set is never reached by set algebra ===
# Regression: these walked the left-hand set by index while re-hashing every
# element, so a `__hash__` clearing the set left the walk indexing past its end.
_armed = False


class Clearing:
    def __hash__(self):
        if _armed:
            clearing.clear()
        return 0


clearing = {Clearing(), Clearing()}
_armed = True
assert len(clearing - set()) == 2
assert len(clearing & clearing) == 2
assert len(clearing | set()) == 2
assert len(clearing ^ set()) == 2
assert len(set(clearing)) == 2
assert clearing.isdisjoint(set()) is True
assert clearing.issubset(clearing) is True
assert len(clearing) == 2

# the control: a membership probe does hash, so the same class empties the set
assert (Clearing() in clearing) is False
assert len(clearing) == 0


# === an `__eq__` that raises during insertion propagates ===
# Regression: building a fresh set probed the new table with an `__eq__` whose
# exception was discarded as "not equal", so colliding elements both landed and
# the operation returned a set where CPython raises.
_raising = False
_raising_eq_calls = []


class Raising:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return 0

    def __eq__(self, other):
        if _raising:
            _raising_eq_calls.append(self.n)
            raise ValueError('boom')
        return isinstance(other, Raising) and self.n == other.n


def _capture_error(fn):
    """Runs `fn` and reports the exception it raised as `(type name, message)`."""
    try:
        fn()
    except Exception as exc:
        return type(exc).__name__, str(exc)
    return None


BOOM = ('ValueError', 'boom')

left = {Raising(1)}
right = {Raising(2)}
frozen = frozenset({Raising(3)})
mapping = {Raising(4): 4}
_raising = True

# operations that build the result by inserting into a fresh table
assert _capture_error(lambda: left | right) == BOOM
assert _capture_error(lambda: left.union(right)) == BOOM
assert _capture_error(lambda: frozen | right) == BOOM
assert _capture_error(lambda: {Raising(5), Raising(6)}) == BOOM
assert _capture_error(lambda: {Raising(n) for n in (7, 8)}) == BOOM
assert _capture_error(lambda: set([Raising(9), Raising(10)])) == BOOM
assert _capture_error(lambda: frozenset([Raising(11), Raising(12)])) == BOOM
assert _capture_error(lambda: mapping.keys() | right) == BOOM

# operations that probe an existing set already propagated, and still do
assert _capture_error(lambda: left & right) == BOOM
assert _capture_error(lambda: left - right) == BOOM
assert _capture_error(lambda: left ^ right) == BOOM
assert _capture_error(lambda: left.add(Raising(13))) == BOOM
assert _capture_error(lambda: left.update(right)) == BOOM

# the first raise ends the probe: `crowded` holds two colliding entries, but the
# insertion compares against one of them and gives up
_raising = False
crowded = {Raising(14), Raising(15)}
_raising = True

_raising_eq_calls.clear()
assert _capture_error(lambda: crowded | {Raising(16)}) == BOOM
assert len(_raising_eq_calls) == 1

_raising_eq_calls.clear()
assert _capture_error(lambda: crowded.union([Raising(17)])) == BOOM
assert len(_raising_eq_calls) == 1

# the left-hand set is unchanged by the failed operations
_raising = False
assert len(left) == 1
assert len(crowded) == 2


# === a failed update releases the entries it never reached ===
# Regression: `update` copied the source's entries out and consumed them in a
# plain loop, so a raising insertion abandoned the rest of the copies and their
# reference counts went with them.
class Tripwire:
    def __hash__(self):
        return 0

    def __eq__(self, other):
        raise ValueError('tripped')


class Colliding:
    def __hash__(self):
        return 0


TRIPPED = ('ValueError', 'tripped')


def _tripwire_target():
    return {Tripwire()}


assert _capture_error(lambda: _tripwire_target().update({Colliding(), Colliding()})) == TRIPPED
assert _capture_error(lambda: _tripwire_target().update(frozenset({Colliding(), Colliding()}))) == TRIPPED
assert _capture_error(lambda: _tripwire_target().update([Colliding(), Colliding()])) == TRIPPED
assert _capture_error(lambda: _tripwire_target().update(iter([Colliding(), Colliding()]))) == TRIPPED


def _ior_trip():
    target = _tripwire_target()
    target |= {Colliding(), Colliding()}


assert _capture_error(_ior_trip) == TRIPPED


# === a failed set construction releases the items it already took ===
# Regression: the set literal and `set(iterable)` built into an unguarded local,
# so an item that could not be inserted stranded every item before it.
class Plain:
    pass


UNHASHABLE = ('TypeError', "cannot use 'list' as a set element (unhashable type: 'list')")

assert _capture_error(lambda: {Plain(), [], Plain()}) == UNHASHABLE
assert _capture_error(lambda: set([Plain(), [], Plain()])) == UNHASHABLE
assert _capture_error(lambda: frozenset([Plain(), [], Plain()])) == UNHASHABLE
assert _capture_error(lambda: {x for x in (Plain(), [], Plain())}) == UNHASHABLE
