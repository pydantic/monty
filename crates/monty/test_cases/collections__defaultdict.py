# Tests for `collections.defaultdict`.
#
# Every expectation here is derived from CPython 3.14. This file is dual-run
# against CPython and Monty, so both must agree exactly.
#
# NOT tested here, and why:
# - `type(d).__name__`: CPython gives the bare 'defaultdict'; Monty gives the
#   qualified 'collections.defaultdict', as it does for every module-scoped
#   type. That one name is what CPython spells qualified in *every* other
#   surface (repr, and every error message), so matching it there is the better
#   trade. Same divergence as deque, see limitations/collections.md.

from collections import defaultdict

# === Missing-key access invokes the factory and inserts ===
d = defaultdict(int)
d['a'] += 1
d['a'] += 1
assert d['a'] == 2, 'int factory accumulates'
assert d['new'] == 0, 'missing key gets the factory default'
assert dict(d) == {'a': 2, 'new': 0}, 'accessing a missing key inserts it'

dl = defaultdict(list)
dl['x'].append(1)
dl['x'].append(2)
assert dict(dl) == {'x': [1, 2]}, 'list factory builds per-key lists'

assert defaultdict(lambda: 42)['q'] == 42, 'arbitrary callable factory'

# === default_factory attribute ===
assert defaultdict(int).default_factory is int, 'default_factory exposes the callable'
assert defaultdict().default_factory is None, 'no factory means default_factory is None'

# default_factory is writable, including to None
dw = defaultdict(int)
dw.default_factory = list
assert dw['k'] == [], 'reassigned factory takes effect'
dw.default_factory = None
try:
    dw['gone']
    assert False, 'expected KeyError once factory is None'
except KeyError as e:
    assert str(e) == "'gone'", 'KeyError carries the key repr'

# The setter accepts a non-callable and defers the error to the missing-key
# lookup that actually calls it — unlike the constructor, which rejects eagerly.
dn = defaultdict(int)
dn.default_factory = 42
assert dn.default_factory == 42, 'setter stores a non-callable unvalidated'
assert repr(dn) == 'defaultdict(42, {})', 'repr shows the non-callable factory'
try:
    dn['missing']
    assert False, 'expected calling a non-callable factory to fail'
except TypeError as e:
    assert str(e) == "'int' object is not callable", str(e)

try:
    defaultdict(42)
    assert False, 'expected a non-callable constructor argument to fail'
except TypeError as e:
    assert str(e) == 'first argument must be callable or None', str(e)

# === No factory: missing keys raise KeyError ===
# (Only string keys are checked: Monty's KeyError text for non-string keys is a
# pre-existing divergence, not specific to defaultdict.)
dn = defaultdict()
try:
    dn['missing']
    assert False, 'expected KeyError with no factory'
except KeyError as e:
    assert str(e) == "'missing'", 'KeyError carries the key repr'

# === Initial data (positional mapping and keyword args) ===
d2 = defaultdict(int, {'a': 5})
d2['b'] += 1
assert dict(d2) == {'a': 5, 'b': 1}, 'initial mapping is preserved'
assert dict(defaultdict(int, x=1, y=2)) == {'x': 1, 'y': 2}, 'keyword initial data'

# === get() and `in` do NOT invoke the factory ===
d3 = defaultdict(int)
assert d3.get('z') is None, 'get() returns None without inserting'
assert 'z' not in d3, 'membership test does not insert'
assert dict(d3) == {}, 'neither get() nor `in` mutated the dict'

# === __missing__ can be called directly ===
assert defaultdict(int).__missing__('k') == 0, '__missing__ returns the factory value'

# === Type, repr, dict-compatibility ===
assert repr(type(defaultdict(int))) == "<class 'collections.defaultdict'>"
assert isinstance(defaultdict(int), dict), 'defaultdict is a dict subclass'

# `defaultdict` is the type object itself, not a factory function wrapping it
assert type(defaultdict(int)) is defaultdict, 'instance type is the defaultdict type object'
assert isinstance(defaultdict(int), defaultdict), 'isinstance against the type object'
assert not isinstance({'a': 1}, defaultdict), 'a plain dict is not a defaultdict'
assert repr(defaultdict) == "<class 'collections.defaultdict'>"

# CPython spells the name qualified in error messages too; these pin the
# surfaces that the qualified type name buys us.
try:
    defaultdict(int) + 1
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'collections.defaultdict' and 'int'"
try:
    defaultdict(int).nope
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'collections.defaultdict' object has no attribute 'nope'"
# unlike Counter, defaultdict inherits a working dict.fromkeys
_dfk = defaultdict.fromkeys('ab')
assert repr(_dfk) == "defaultdict(None, {'a': None, 'b': None})", 'type-level fromkeys builds a defaultdict'
assert repr(defaultdict.fromkeys('ab', 0)) == "defaultdict(None, {'a': 0, 'b': 0})", 'fromkeys with a value'
assert defaultdict(int, {'a': 1}) == {'a': 1}, 'compares equal to a plain dict'
assert repr(defaultdict(int, {'a': 1})) == "defaultdict(<class 'int'>, {'a': 1})", 'repr shows factory and data'
assert repr(defaultdict(list)) == "defaultdict(<class 'list'>, {})", 'repr with empty data'
assert repr(defaultdict()) == 'defaultdict(None, {})', 'repr with no factory'

# === Ordinary dict operations still work ===
d4 = defaultdict(int)
d4['a'] = 10
d4['b'] = 20
assert len(d4) == 2, 'len'
assert sorted(d4.keys()) == ['a', 'b'], 'keys()'
assert d4.pop('a') == 10, 'pop()'
assert list(d4.items()) == [('b', 20)], 'items() after pop'
d4.update({'c': 30})
assert d4['c'] == 30, 'update()'

# === copy() keeps the factory and type ===
c = defaultdict(int, {'a': 1}).copy()
assert type(c) is defaultdict, 'copy is a defaultdict'
assert c.default_factory is int, 'copy keeps the factory'
assert dict(c) == {'a': 1}, 'copy keeps the data'

# === Factory closure that captures the defaultdict (reference cycle) ===
dc = defaultdict()
dc.default_factory = lambda: dc
assert dc['self'] is dc, 'factory may return the dict itself'

# === Construction error: non-callable factory ===
try:
    defaultdict(5)
    assert False, 'expected non-callable factory to raise'
except TypeError as e:
    assert str(e) == 'first argument must be callable or None', 'non-callable factory message'

# === fromkeys() returns a defaultdict ===
# CPython's `dict.fromkeys` is a classmethod, so it builds `cls()` — a
# defaultdict, but constructed with no argument, hence a None factory.
d = defaultdict(list)
d['x'].append(1)
fk = d.fromkeys('ab', 0)
assert type(fk) is defaultdict, 'fromkeys returns a defaultdict'
assert fk.default_factory is None, 'fromkeys builds cls() so the factory is None'
assert dict(fk) == {'a': 0, 'b': 0}, 'fromkeys fills the given keys'
try:
    fk['zz']
    assert False, 'expected a missing key to raise with a None factory'
except KeyError as e:
    assert str(e) == "'zz'", 'None factory still raises KeyError'

# fromkeys is inherited from dict, so its arg-count errors name the bare method
# (`fromkeys`), not `dict.fromkeys` or `defaultdict.fromkeys`, matching CPython.
try:
    defaultdict.fromkeys()
    assert False, 'expected fromkeys with no args to raise'
except TypeError as e:
    assert str(e) == 'fromkeys expected at least 1 argument, got 0', 'fromkeys arity message'
try:
    defaultdict.fromkeys('ab', 0, 9)
    assert False, 'expected fromkeys with too many args to raise'
except TypeError as e:
    assert str(e) == 'fromkeys expected at most 2 arguments, got 3', 'fromkeys too-many message'

# === Iteration ===
# defaultdict inherits dict's __iter__, so iteration yields keys in insertion
# order. Crucially it does NOT go through __missing__: unlike `d[k]`, iterating
# never invokes the factory and never inserts.
d9 = defaultdict(int, {'z': 1, 'a': 2})
assert list(d9) == ['z', 'a']
assert [k for k in d9] == ['z', 'a']
assert next(iter(d9)) == 'z'
assert [*d9] == ['z', 'a']
assert list(enumerate(d9)) == [(0, 'z'), (1, 'a')]
assert list(zip(d9, [1, 2])) == [('z', 1), ('a', 2)]
assert list(reversed(d9)) == ['a', 'z']
assert sorted(d9) == ['a', 'z']

# a full pass inserts nothing, and the factory is never called
calls9 = []


def counting_factory():
    calls9.append(1)
    return 0


d10 = defaultdict(counting_factory, {'a': 1, 'b': 2})
before9 = len(d10)
assert [k for k in d10] == ['a', 'b']
assert len(d10) == before9
assert calls9 == []
# but a missing-key access still does invoke it
assert d10['c'] == 0
assert calls9 == [1]
assert len(d10) == before9 + 1

# === Mutation during iteration (inherited from dict) ===
d11 = defaultdict(int, {'a': 1, 'b': 2})
it11 = iter(d11)
next(it11)
d11['zzz'] = 1
try:
    next(it11)
    assert False, 'expected mutation during iteration to raise'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration'

# a missing-key access DURING iteration inserts, so it invalidates too
d12 = defaultdict(int, {'a': 1, 'b': 2})
it12 = iter(d12)
next(it12)
d12['new']
try:
    next(it12)
    assert False, 'expected the factory insert to invalidate the iterator'
except RuntimeError as e:
    assert str(e) == 'dictionary changed size during iteration'
