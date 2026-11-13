# === Construction ===
from collections import OrderedDict
import collections
import copy
import json

assert collections.OrderedDict is OrderedDict
assert OrderedDict() == {}
assert repr(OrderedDict()) == 'OrderedDict()'
assert repr(OrderedDict({'a': 1})) == "OrderedDict({'a': 1})"
assert list(OrderedDict([('b', 2), ('a', 1)])) == ['b', 'a']
assert list(OrderedDict(b=2, a=1)) == ['b', 'a']
assert list(OrderedDict({'x': 1}, y=2)) == ['x', 'y']

try:
    OrderedDict(1, 2)
    assert False, 'two positionals'
except TypeError as e:
    assert str(e) == 'expected at most 1 argument, got 2'

# === Type and isinstance ===
od = OrderedDict(a=1)
assert isinstance(od, dict)
assert isinstance(od, OrderedDict)
assert not isinstance({}, OrderedDict), 'plain dict is not'
assert type(od) is OrderedDict
assert type(od).__name__ == 'OrderedDict'

# === Equality ===
assert OrderedDict(a=1, b=2) != OrderedDict(b=2, a=1)
assert OrderedDict(a=1, b=2) == OrderedDict(a=1, b=2)
assert OrderedDict(a=1, b=2) == {'b': 2, 'a': 1}
assert {'b': 2, 'a': 1} == OrderedDict(a=1, b=2)
assert OrderedDict(a=1) != OrderedDict(a=2)

# === move_to_end / popitem ===
od = OrderedDict([('a', 1), ('b', 2), ('c', 3)])
od.move_to_end('a')
assert list(od) == ['b', 'c', 'a']
od.move_to_end('a', last=False)
assert list(od) == ['a', 'b', 'c']
od.move_to_end('b', False)
assert list(od) == ['b', 'a', 'c']
od.move_to_end('c')
assert list(od) == ['b', 'a', 'c']
try:
    od.move_to_end('z')
    assert False, 'missing key'
except KeyError as e:
    assert e.args == ('z',)

assert od.popitem() == ('c', 3)
assert od.popitem(last=False) == ('b', 2)
assert od.popitem(False) == ('a', 1)
try:
    od.popitem()
    assert False, 'empty popitem'
except KeyError as e:
    assert str(e) == "'dictionary is empty'"

# === reversed, views ===
od = OrderedDict([('a', 1), ('b', 2), ('c', 3)])
assert list(reversed(od)) == ['c', 'b', 'a']
assert list(od.keys()) == ['a', 'b', 'c']
assert list(od.values()) == [1, 2, 3]
assert list(od.items()) == [('a', 1), ('b', 2), ('c', 3)]
od.move_to_end('a')
assert list(od.items()) == [('b', 2), ('c', 3), ('a', 1)]
assert list(reversed(od.keys())) == ['a', 'c', 'b']

# === copy, fromkeys ===
od = OrderedDict([('b', 1), ('a', 2)])
c = od.copy()
assert c == od and list(c) == ['b', 'a'], 'copy'
assert type(c) is OrderedDict
c['z'] = 0
assert 'z' not in od
f = OrderedDict.fromkeys(['x', 'y', 'z'], 0)
assert type(f) is OrderedDict and list(f) == ['x', 'y', 'z'], 'fromkeys'
assert repr(f) == "OrderedDict({'x': 0, 'y': 0, 'z': 0})"
f2 = od.fromkeys('ab')
assert type(f2) is OrderedDict
cc = copy.copy(od)
assert type(cc) is OrderedDict and list(cc) == ['b', 'a'], 'copy.copy'
dc = copy.deepcopy(od)
assert type(dc) is OrderedDict and dc == od, 'deepcopy'

# === | and |= ===
a = OrderedDict([('a', 1), ('b', 2)])
r = a | {'c': 3, 'a': 9}
assert type(r) is OrderedDict and list(r.items()) == [('a', 9), ('b', 2), ('c', 3)], 'or'
r = {'z': 0} | a
assert type(r) is OrderedDict
assert type(a | OrderedDict(q=1)) is OrderedDict
a |= {'d': 4}
assert type(a) is OrderedDict and list(a) == ['a', 'b', 'd'], 'ior'

# === Interop ===
od = OrderedDict([('b', 1), ('a', 2)])
assert json.dumps(od) == '{"b": 1, "a": 2}'
d = dict(od)
assert type(d) is dict and list(d) == ['b', 'a'], 'dict()'
assert OrderedDict(od) == od and list(OrderedDict(od)) == ['b', 'a'], 'OrderedDict(od)'
assert len(od) == 2 and 'a' in od and od['b'] == 1, 'dict protocol'
od.pop('b')
assert list(od) == ['a']
od['n'] = 5
assert list(od) == ['a', 'n']
od['a'] = 7
assert list(od) == ['a', 'n']
assert od.pop('a') == 7 and list(od) == ['n'], 'pop'
assert od.setdefault('s', 1) == 1 and list(od) == ['n', 's'], 'setdefault'
od.update(t=2)
assert list(od) == ['n', 's', 't']
assert sorted(od) == ['n', 's', 't']

# === Misc ===
assert OrderedDict(a=1) == OrderedDict(a=1.0)
assert OrderedDict([(1, 2)]) != [(1, 2)]
assert str(OrderedDict(a=1)) == "OrderedDict({'a': 1})"
assert repr(type(OrderedDict())) == "<class 'collections.OrderedDict'>"
o = OrderedDict(a=1)
o.clear()
assert repr(o) == 'OrderedDict()' and not o, 'clear'
assert {**OrderedDict(a=1)} == {'a': 1}

# Reordering while iterating raises when an entry would be revisited (the message
# differs from CPython).
o = OrderedDict(a=1, b=2, c=3)
try:
    for k in o:
        o.move_to_end(k)
    assert False, 'no error'
except RuntimeError:
    pass

# === Subscripting ===
assert repr(OrderedDict[str, int]) == 'collections.OrderedDict[str, int]'
