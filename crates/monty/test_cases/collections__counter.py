# Tests for `collections.Counter`.
#
# Every expectation here is derived from CPython 3.14. This file is dual-run
# against CPython and Monty, so both must agree exactly.
#
# NOT covered in this version (documented in limitations/collections.md):
# - `del c[key]` (the `del` statement is unimplemented Monty-wide).
# - `elements()` returns a list rather than a lazy iterator.

from collections import Counter

# === Construction ===
assert Counter('abracadabra') == {'a': 5, 'b': 2, 'r': 2, 'c': 1, 'd': 1}, 'count characters of a string'
assert Counter(['a', 'b', 'a']) == {'a': 2, 'b': 1}, 'count items of a list'
assert Counter({'a': 2, 'b': 1}) == {'a': 2, 'b': 1}, 'from a mapping'
assert Counter(a=2, b=3) == {'a': 2, 'b': 3}, 'from keyword counts'
assert Counter() == {}, 'empty counter'
assert dict(Counter(range(3))) == {0: 1, 1: 1, 2: 1}, 'count an iterable of ints'

# === Missing key returns 0 without inserting ===
c = Counter('aab')
assert c['z'] == 0, 'missing key counts as zero'
assert dict(c) == {'a': 2, 'b': 1}, 'reading a missing key does not insert it'
assert c.get('z') is None, 'get() still returns None for a missing key'
assert c.get('z', -1) == -1, 'get() honours its default'

# === most_common ===
assert Counter('abracadabra').most_common(2) == [('a', 5), ('b', 2)], 'top-n by count'
assert Counter('aabbc').most_common() == [('a', 2), ('b', 2), ('c', 1)], 'all, ties in insertion order'
assert Counter('aab').most_common(None) == [('a', 2), ('b', 1)], 'None means all'
assert Counter('aab').most_common(0) == [], 'n=0 yields nothing'
# `n` is taken through __index__: a bool is 0/1, and a big int clamps (a huge
# positive returns everything, a negative returns nothing).
assert Counter('aabbc').most_common(True) == [('a', 2)], 'True means top 1'
assert Counter('aabbc').most_common(False) == [], 'False means top 0'
assert Counter('aab').most_common(2**70) == [('a', 2), ('b', 1)], 'huge n returns all'
assert Counter('aab').most_common(-(2**70)) == [], 'huge negative n returns none'
assert Counter('aab').most_common(-1) == [], 'negative n returns none'
try:
    Counter('aab').most_common(1.5)
    assert False, 'expected a float n to fail'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer", str(exc)

# === elements (skips zero and negative counts) ===
assert sorted(Counter(a=2, b=1).elements()) == ['a', 'a', 'b'], 'elements repeats each key by its count'
assert list(Counter(a=2, b=-1, c=0).elements()) == ['a', 'a'], 'non-positive counts produce no elements'

# === total ===
assert Counter(a=2, b=3).total() == 5, 'total sums the counts'
assert Counter().total() == 0, 'total of an empty counter is zero'
assert Counter(a=2, b=-1).total() == 1, 'total includes negative counts'

# === update (adds counts) ===
c = Counter(a=1)
c.update(['a', 'b', 'b'])
assert dict(c) == {'a': 2, 'b': 2}, 'update from an iterable adds counts'
c = Counter(a=1)
c.update(Counter(a=2))
assert dict(c) == {'a': 3}, 'update from a mapping adds counts'
c = Counter(a=1)
c.update()
assert dict(c) == {'a': 1}, 'update with no argument is a no-op'
c = Counter(a=1)
c.update(a=5, b=2)
assert dict(c) == {'a': 6, 'b': 2}, 'update from keyword counts'

# === An explicit None source is a no-op (kwargs still apply) ===
# Counter special-cases None so `Counter(None)` and `c.update(None)` mirror the
# no-argument forms rather than raising "'NoneType' object is not iterable".
assert dict(Counter(None)) == {}, 'Counter(None) is empty'
assert dict(Counter(None, x=5)) == {'x': 5}, 'Counter(None, **kwargs) keeps the kwargs'
c = Counter(a=1)
c.update(None)
assert dict(c) == {'a': 1}, 'update(None) is a no-op'
c = Counter(a=1)
c.update(None, b=2)
assert dict(c) == {'a': 1, 'b': 2}, 'update(None, **kwargs) applies the kwargs'
c = Counter(a=3)
c.subtract(None)
assert dict(c) == {'a': 3}, 'subtract(None) is a no-op'

# === subtract (keeps negative counts) ===
c = Counter(a=3, b=1)
c.subtract(['a', 'b', 'b'])
assert dict(c) == {'a': 2, 'b': -1}, 'subtract from an iterable, keeping negatives'
c = Counter(a=3)
c.subtract(Counter(a=1, b=2))
assert dict(c) == {'a': 2, 'b': -2}, 'subtract from a mapping'

# update/subtract take a single positional (`self, iterable=None, /, **kwargs`),
# so an extra positional reports the qualified method name and counts self.
try:
    Counter().update(1, 2)
    assert False, 'expected update with two positionals to raise'
except TypeError as e:
    assert str(e) == 'Counter.update() takes from 1 to 2 positional arguments but 3 were given', 'update arity message'
try:
    Counter().subtract(1, 2)
    assert False, 'expected subtract with two positionals to raise'
except TypeError as e:
    assert str(e) == 'Counter.subtract() takes from 1 to 2 positional arguments but 3 were given', (
        'subtract arity message'
    )

# === Arithmetic operators (results keep only positive counts) ===
assert Counter(a=3, b=1) + Counter(a=1, c=2) == {'a': 4, 'b': 1, 'c': 2}, 'addition sums counts'
assert Counter(a=3, b=1) - Counter(a=1, b=2) == {'a': 2}, 'subtraction drops non-positive results'
assert Counter(a=3, b=1) & Counter(a=1, b=2) == {'a': 1, 'b': 1}, 'intersection takes the minimum'
assert Counter(a=3, b=1) | Counter(a=1, b=2) == {'a': 3, 'b': 2}, 'union takes the maximum'

# === Unary operators ===
assert +Counter(a=2, b=-1) == {'a': 2}, 'unary plus keeps positive counts'
assert -Counter(a=2, b=-1) == {'b': 1}, 'unary minus negates and keeps positive counts'

# === Multiset comparisons (element-wise over the union of keys) ===
assert (Counter(a=1, b=2) <= Counter(a=2, b=2)) is True, '<= holds when every count is at most'
assert (Counter(a=1) < Counter(a=2)) is True, '< is <= and not equal'
assert (Counter(a=1) < Counter(a=1)) is False, '< is false when equal'
assert (Counter(a=3) >= Counter(a=1, b=1)) is False, '>= fails on a key present only on the right'
assert (Counter(a=2) > Counter(a=1)) is True, '> is >= and not equal'

# === NaN counts follow IEEE: every ordering comparison is false, and NaN != NaN ===
nan = float('nan')
assert (Counter(a=nan) <= Counter(a=nan)) is False, 'nan <= nan is false'
assert (Counter(a=nan) < Counter(a=nan)) is False, 'nan < nan is false'
assert (Counter(a=nan) >= Counter(a=nan)) is False, 'nan >= nan is false'
assert (Counter(a=nan) > Counter(a=nan)) is False, 'nan > nan is false'
# In | / &, `count < other` is false for NaN, so the non-strict branch wins; the
# `> 0` keep-positive filter then strips any NaN that survives.
assert dict(Counter(a=1) | Counter(a=nan)) == {'a': 1}, '| keeps the left 1 over a NaN'
assert dict(Counter(a=nan) | Counter(a=1)) == {}, '| drops a NaN maximum'
assert dict(Counter(a=1) & Counter(a=nan)) == {}, '& drops a NaN minimum'
assert dict(Counter(a=nan) & Counter(a=1)) == {'a': 1}, '& keeps the orderable 1'
assert dict(+Counter(a=nan)) == {}, 'unary + strips a NaN count'
assert dict(-Counter(a=nan)) == {}, 'unary - strips a NaN count'


# === Unorderable counts report the operation-specific operator (CPython wording) ===
# Each Counter operation runs a particular Python comparison, and the operator in
# the TypeError is the one that actually ran, with the operands in that order.
def cmp_err(fn):
    try:
        fn()
        assert False, 'expected an unorderable-count TypeError'
    except TypeError as exc:
        return str(exc)


assert cmp_err(lambda: Counter(a='x') <= Counter(a=1)) == "'<=' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: Counter(a='x') < Counter(a=1)) == "'<=' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: Counter(a='x') >= Counter(a=1)) == "'>=' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: Counter(a='x') > Counter(a=1)) == "'>=' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: Counter(a='x') | Counter(a=1)) == "'<' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: Counter() | Counter(a='x')) == "'>' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: Counter(a='x') & Counter(a=1)) == "'<' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: +Counter(a='x')) == "'>' not supported between instances of 'str' and 'int'"
assert cmp_err(lambda: -Counter(a='x')) == "'<' not supported between instances of 'str' and 'int'"


# In-place |= / &= run `other_count > self[elem]` and `other[elem] < count`
# respectively (the mutated operand's count second), so the wording differs.
def _ior_err():
    c = Counter(a=1)
    c |= Counter(a='x')


def _iand_err():
    c = Counter(a=1)
    c &= Counter(a='x')


assert cmp_err(_ior_err) == "'>' not supported between instances of 'str' and 'int'"
assert cmp_err(_iand_err) == "'<' not supported between instances of 'str' and 'int'"

# === Result types are Counters ===
assert type(Counter(a=1) + Counter(a=1)).__name__ == 'Counter', 'addition returns a Counter'
assert type(+Counter(a=1)).__name__ == 'Counter', 'unary plus returns a Counter'

# === repr (most-common order), type, dict-compatibility ===
assert repr(Counter('aab')) == "Counter({'a': 2, 'b': 1})", 'repr in count-descending order'
assert repr(Counter({'a': -1, 'b': 2})) == "Counter({'b': 2, 'a': -1})", 'repr orders by count, negatives last'
assert repr(Counter()) == 'Counter()', 'empty repr'
assert type(Counter()).__name__ == 'Counter', 'type name is Counter'
assert isinstance(Counter(), dict), 'Counter is a dict subclass'

# `Counter` is the type object itself, not a factory function wrapping it
assert type(Counter(a=1)) is Counter, 'instance type is the Counter type object'
assert isinstance(Counter(a=1), Counter), 'isinstance against the type object'
assert not isinstance({'a': 1}, Counter), 'a plain dict is not a Counter'
assert Counter.__name__ == 'Counter', 'the type object carries its name'
# CPython disables fromkeys on Counter; reachable from the type as well as an instance
try:
    Counter.fromkeys('ab')
    assert False, 'expected type-level Counter.fromkeys to be disabled'
except NotImplementedError as exc:
    assert str(exc) == 'Counter.fromkeys() is undefined.  Use Counter(iterable) instead.', str(exc)
assert Counter(a=1) == {'a': 1}, 'compares equal to a plain dict'

# === Ordinary dict operations still work ===
c = Counter(a=1, b=2)
assert len(c) == 2, 'len'
assert sorted(c.keys()) == ['a', 'b'], 'keys()'
c['c'] = 5
assert c['c'] == 5, 'direct assignment'

# === Errors ===
# `fromkeys` is disabled on Counter. Tested via an instance because Monty models
# `Counter` as a factory function, not a class with classmethods (so the
# type-level `Counter.fromkeys` is not available; see limitations/collections.md).
try:
    Counter(a=1).fromkeys('abc')
    assert False, 'expected fromkeys to raise'
except NotImplementedError as e:
    assert str(e) == 'Counter.fromkeys() is undefined.  Use Counter(iterable) instead.', 'fromkeys message'

try:
    Counter(1)
    assert False, 'expected non-iterable to raise'
except TypeError as e:
    assert str(e) == "'int' object is not iterable", 'non-iterable message'

# An unhashable element raises TypeError, and the counter (plus the elements
# after the bad one) must not leak — regression for counter_bump / counter_update.
# Only the type is asserted: Monty's unhashable-key message is a pre-existing
# dict divergence from CPython's, so the exact text can't be dual-run.
try:
    Counter([1, [2], 3])
    assert False, 'expected unhashable element to raise'
except TypeError:
    pass

# === Non-integer counts are preserved ===
# A Counter is a dict, so its values are ordinary Python objects; CPython never
# coerces them. Monty stored an i64 "count" and silently rewrote anything that
# was not a small int to 0, destroying floats and big ints.
assert dict(Counter({'a': 1.5})) == {'a': 1.5}, 'a float count survives construction'
assert dict(Counter(a=1.5)) == {'a': 1.5}, 'a float count survives a kwarg'
assert dict(Counter({'a': 2**70})) == {'a': 1180591620717411303424}, 'a big int count survives'
assert repr(Counter({'a': True})) == "Counter({'a': True})", 'a bool count is not normalised to 1'

# === total() sums the real values ===
assert Counter({'a': 1.5, 'b': 2}).total() == 3.5, 'total() adds floats as floats'
assert Counter({'a': 2**70}).total() == 1180591620717411303424, 'total() preserves big ints'
assert Counter({'a': 3.5, 'b': 2**70}).total() == 1.1805916207174113e21
assert Counter({'a': 2**70, 'b': 3.5}).total() == 1.1805916207174113e21
assert Counter().total() == 0, 'total() of an empty Counter is 0'
# A count that cannot be added raises part-way through the fold, so the running
# total and the counts after the bad one must not leak.
try:
    Counter({'a': 1, 'b': 'x', 'c': 2**70}).total()
    assert False, 'expected an unaddable count to raise'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for +: 'int' and 'str'"

# === Arithmetic uses real numeric addition ===
assert dict(Counter({'a': 1.5}) + Counter({'a': 1.5})) == {'a': 3.0}, '+ adds floats'
assert dict(Counter({'a': 3.5}) + Counter({'a': 2**70})) == {'a': 1.1805916207174113e21}
assert dict(Counter({'a': 2**70}) + Counter({'a': 3.5})) == {'a': 1.1805916207174113e21}
assert dict(Counter({'a': 3.5}) - Counter({'a': 1.0})) == {'a': 2.5}, '- subtracts floats'
assert dict(Counter({'a': 3.5}) & Counter({'a': 1.5})) == {'a': 1.5}, '& takes the float minimum'
assert dict(Counter({'a': 3.5}) | Counter({'a': 1.5})) == {'a': 3.5}, '| takes the float maximum'
assert dict(-Counter({'a': -1.5})) == {'a': 1.5}, 'unary - negates a float'
assert dict(+Counter({'a': 1.5})) == {'a': 1.5}, 'unary + keeps a positive float'

# === subtract() and update() keep float values ===
c = Counter({'a': 2})
c.subtract({'a': 0.5})
assert dict(c) == {'a': 1.5}, 'subtract() produces a float'
assert dict(Counter({'a': 1}) | Counter({'a': 1.5})) == {'a': 1.5}, '| against a float operand'

# === most_common orders by the real value ===
assert Counter({'a': 1.5, 'b': 9.5, 'c': 2}).most_common() == [('b', 9.5), ('c', 2), ('a', 1.5)], (
    'most_common sorts floats and ints together by value'
)
assert Counter({'a': 2**70, 'b': 5}).most_common(1) == [('a', 1180591620717411303424)], (
    'most_common ranks a big int above a small one'
)

# === elements() requires whole-number counts, like CPython ===
assert list(Counter({'a': 3}).elements()) == ['a', 'a', 'a'], 'elements() repeats by an int count'
# `list(...)` forces the iteration: CPython's elements() is lazy and raises when
# consumed, whereas Monty's is eager and raises from the call itself. Either way
# the same TypeError reaches the caller.
try:
    list(Counter({'a': 1.5}).elements())
    assert False, 'expected a float count to raise'
except TypeError as e:
    assert str(e) == "'float' object cannot be interpreted as an integer", 'elements() float message'

# A count outside ssize_t raises OverflowError whichever sign it carries:
# CPython converts the count before it inspects the sign, so a huge negative is
# rejected rather than skipped the way a small negative one is. The boundary
# sits exactly at the int64 edge.
for big_count in (2**63, 2**70, -(2**63) - 1, -(2**70)):
    try:
        list(Counter({'a': big_count}).elements())
        assert False, 'expected an out-of-range count to raise'
    except OverflowError as e:
        assert str(e) == 'Python int too large to convert to C ssize_t'

# === repeated bumps of the same key hand the key to the dict ===
# Keys and counts built at runtime are heap values rather than interned ones, so
# a bump that replaces an existing entry has to release exactly the old count —
# leaking it, or dropping the key it just stored, shows up here.
bump_key = 'k' + str(1)
bumped = Counter()
bumped.update({bump_key: 2**70})  # into an empty Counter: the value is stored as-is
bumped.update({bump_key: 2**70})  # into a non-empty one: the count is replaced
assert bumped[bump_key] == 2361183241434822606848
bumped.subtract({bump_key: 2**70})
assert bumped[bump_key] == 1180591620717411303424
assert dict(bumped) == {'k1': 1180591620717411303424}

# counting elements repeats the same transfer, one increment at a time
assert dict(Counter(['x' + str(i % 2) for i in range(5)])) == {'x0': 3, 'x1': 2}

# a Counter updated from itself reads every count before writing any of them
self_update = Counter({'a' + str(2): 3})
self_update.update(self_update)
assert dict(self_update) == {'a2': 6}

# === copy() returns a working Counter ===
# `dict.copy` builds a fresh dict, so it has to carry the Counter tag across or
# the result silently degrades to a plain dict.
c = Counter(a=1, b=2).copy()
assert type(c).__name__ == 'Counter', 'copy() is still a Counter'
assert repr(c) == "Counter({'b': 2, 'a': 1})", 'copy() reprs as a Counter, in count order'
assert c['zz'] == 0, 'copy() keeps the missing-key-returns-0 behaviour'
assert dict(c) == {'a': 1, 'b': 2}, 'reading a missing key on the copy does not insert'
assert dict(c + Counter(a=1)) == {'a': 2, 'b': 2}, 'Counter arithmetic works on the copy'
assert c.most_common(1) == [('b', 2)], 'most_common works on the copy'
assert c.total() == 3, 'total works on the copy'
# the copy is an independent object
orig = Counter(a=1)
cp = orig.copy()
cp.update({'a': 5})
assert dict(orig) == {'a': 1}, 'copy() is a distinct object'

# === In-place operators mutate the left Counter ===
# CPython's __iadd__/__isub__/__iand__/__ior__ mutate self and return it, so the
# object identity survives and any alias observes the update — unlike the binary
# operators, which build a fresh Counter. Each ends with _keep_positive().
c = Counter(a=1, b=2)
c += Counter(a=2, b=-5)
assert dict(c) == {'a': 3}, '+= adds counts and strips non-positive results'
assert type(c).__name__ == 'Counter', '+= keeps the Counter type'

c = Counter(a=1, b=-3)
c += Counter(cc=1)
assert dict(c) == {'a': 1, 'cc': 1}, '+= strips a pre-existing non-positive count'

c = Counter(a=3, b=1)
c -= Counter(a=1, b=5)
assert dict(c) == {'a': 2}, '-= subtracts and drops non-positive results'

c = Counter(a=3, b=1)
c &= Counter(a=1, b=5)
assert dict(c) == {'a': 1, 'b': 1}, '&= keeps the minimum of each count'

# &= walks self's keys, so a key missing from the right reads as 0 and is stripped
c = Counter(a=3, b=1)
c &= Counter(a=2)
assert dict(c) == {'a': 2}, '&= drops keys absent from the right operand'

c = Counter(a=3, b=1)
c |= Counter(a=1, b=5)
assert dict(c) == {'a': 3, 'b': 5}, '|= keeps the maximum of each count'

# the left operand is mutated in place, so an alias sees the result
c = Counter(a=3)
alias = c
c -= Counter(a=1)
assert dict(alias) == {'a': 2}, '-= mutates in place, so an alias observes it'
c += Counter(a=5)
assert dict(alias) == {'a': 7}, '+= mutates in place too'
c &= Counter(a=4)
assert dict(alias) == {'a': 4}, '&= mutates in place too'
c |= Counter(a=6)
assert dict(alias) == {'a': 6}, '|= mutates in place too'

# === In-place operators accept any mapping on the right, not only a Counter ===
# CPython's __iadd__/__isub__/__ior__ iterate other.items() and __iand__
# subscripts other[elem], so a plain dict works with the same semantics.
c = Counter(a=1)
c += {'a': 2, 'b': 5}
assert dict(c) == {'a': 3, 'b': 5}, '+= folds in a plain-dict mapping'
c = Counter(a=5, b=1)
c -= {'a': 2}
assert dict(c) == {'a': 3, 'b': 1}, '-= subtracts a plain-dict mapping'
c = Counter(a=1)
c |= {'a': 5, 'b': 2}
assert dict(c) == {'a': 5, 'b': 2}, '|= takes the max against a plain dict'
c = Counter(a=3, b=1)
c &= {'a': 1, 'b': 5}
assert dict(c) == {'a': 1, 'b': 1}, '&= takes the min against a plain dict'

# &= subscripts other[elem]: unlike a Counter (0 for a missing key), a plain
# dict raises KeyError for a key present in self but not the dict.
try:
    c = Counter(a=3, b=2)
    c &= {'a': 1}
    assert False, 'expected &= with a missing plain-dict key to raise'
except KeyError as exc:
    assert str(exc) == "'b'", str(exc)


# +=/-=/|= call other.items(), so a non-mapping raises AttributeError naming
# the operand's type.
def cmp_att(fn):
    try:
        fn()
        assert False, 'expected an AttributeError'
    except AttributeError as exc:
        return str(exc)


def _iadd_bad():
    c = Counter(a=1)
    c += [1, 2]


def _isub_bad():
    c = Counter(a=1)
    c -= 5


def _ior_bad():
    c = Counter(a=1)
    c |= [1, 2]


assert cmp_att(_iadd_bad) == "'list' object has no attribute 'items'"
assert cmp_att(_isub_bad) == "'int' object has no attribute 'items'"
assert cmp_att(_ior_bad) == "'list' object has no attribute 'items'"

# &= subscripts, so an int right operand is "not subscriptable"; but an empty
# left Counter never subscripts, so it is a no-op.
try:
    c = Counter(a=1)
    c &= 5
    assert False, 'expected &= with an int to raise'
except TypeError as exc:
    assert str(exc) == "'int' object is not subscriptable", str(exc)
c = Counter()
c &= [1, 2]
assert dict(c) == {}, '&= on an empty Counter never subscripts the operand'

# === Multiset comparisons ===
# CPython compares over the union of both key sets, with a missing key reading
# as 0: `all(self[e] <= other[e] for c in (self, other) for e in c)`.
# `<` is `<= and !=`, `>` is `>= and !=`.
assert Counter(a=1) <= Counter(a=2, b=1), '<= is a subset test'
assert Counter(a=1) <= Counter(a=1), '<= is reflexive'
assert not (Counter(a=3) <= Counter(a=2)), '<= fails when a count is larger'
assert Counter(a=1) < Counter(a=2), '< is a proper subset'
assert not (Counter(a=1) < Counter(a=1)), '< is false for equal counters'
assert Counter(a=2, b=1) >= Counter(a=1), '>= is a superset test'
assert Counter(a=2) > Counter(a=1), '> is a proper superset'
assert not (Counter(a=1) > Counter(a=1)), '> is false for equal counters'

# keys present in only one operand read as 0 on the other side
assert Counter(a=1) <= Counter(a=1, b=1), 'a key absent from the left reads as 0'
assert not (Counter(a=1, b=1) <= Counter(a=1)), 'a key absent from the right reads as 0'
assert Counter(a=-1) <= Counter(), 'negative counts compare against an implicit 0'
assert Counter() <= Counter(), 'two empty Counters compare equal'

# float counts compare numerically
assert Counter({'a': 1.5}) <= Counter({'a': 2}), '<= compares float against int counts'
assert Counter({'a': 2.5}) > Counter({'a': 2}), '> compares float against int counts'

# a non-Counter operand is not ordered against a Counter
try:
    Counter(a=1) < {'a': 2}
    assert False, 'expected ordering against a plain dict to raise'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'Counter' and 'dict'", 'ordering vs dict message'

# === Multiset equality ignores zero counts ===
# CPython compares over the union of both key sets with a missing key reading as
# 0, so a zero count is indistinguishable from an absent one — but ONLY between
# two Counters. Against anything else `Counter.__eq__` returns NotImplemented,
# so the comparison falls back to plain dict equality, where a zero count is a
# real entry.
assert Counter(a=1, b=0) == Counter(a=1), 'a zero count equals an absent key'
assert Counter(a=1) == Counter(a=1, b=0), 'and in the other direction'
assert not (Counter(a=1, b=0) != Counter(a=1)), '!= mirrors =='
assert Counter() == Counter(a=0), 'an empty Counter equals an all-zero one'
assert Counter(a=0) == Counter(), 'and in the other direction'
assert Counter(a=-1) == Counter(a=-1), 'negative counts compare normally'
assert not (Counter(a=-1) == Counter()), 'a negative count is not an absent key'
assert not (Counter(a=1) == Counter(a=2)), 'genuinely different counts are unequal'
assert Counter(a=1.0) == Counter(a=1), 'counts compare numerically across types'

# a plain dict operand keeps plain dict equality, in both directions
assert Counter(a=1) == {'a': 1}, 'equal to a plain dict with the same entries'
assert not (Counter(a=1, b=0) == {'a': 1}), 'a zero count is a real entry to a plain dict'
assert {'a': 1} == Counter(a=1), 'plain dict on the left'
assert not ({'a': 1} == Counter(a=1, b=0)), 'plain dict on the left, zero count'
assert Counter(a=1, b=0) != {'a': 1}, '!= against a plain dict'
assert not (Counter(a=1) == 5), 'a Counter is never equal to a non-mapping'

# === Iteration ===
# Counter inherits dict's __iter__, so iteration is over KEYS in INSERTION order.
# That is deliberately not the order repr and most_common use, which is by count
# descending. 'abb' separates the two: inserted a-then-b, but counted b-then-a.
c9 = Counter('abb')
assert repr(c9) == "Counter({'b': 2, 'a': 1})"
assert c9.most_common() == [('b', 2), ('a', 1)]
assert list(c9) == ['a', 'b']
assert [k for k in c9] == ['a', 'b']
assert next(iter(c9)) == 'a'
assert [*c9] == ['a', 'b']
assert list(enumerate(c9)) == [(0, 'a'), (1, 'b')]
assert list(zip(c9, [1, 2])) == [('a', 1), ('b', 2)]
assert list(reversed(c9)) == ['b', 'a']
assert sorted(c9) == ['a', 'b']

# iteration yields keys, not (key, count) pairs
assert list(c9) == list(c9.keys())

# a zero or negative count is still a real entry, so it is still iterated
c10 = Counter(a=1, b=0, c=-1)
assert list(c10) == ['a', 'b', 'c']

# === Mutation during iteration (inherited from dict) ===
try:
    it9 = iter(Counter('ab'))
    next(it9)
    Counter('ab')['x'] = 1  # mutating a DIFFERENT counter is fine
    next(it9)
except RuntimeError:
    assert False, 'mutating an unrelated counter must not invalidate the iterator'

c11 = Counter('ab')
it11 = iter(c11)
next(it11)
c11['zzz'] = 1
try:
    next(it11)
    assert False, 'expected mutation during iteration to raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration'
