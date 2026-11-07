# === Construction ===
from collections import OrderedDict
import collections
import copy
import json

assert collections.OrderedDict is OrderedDict, 'module attribute is the type'
assert OrderedDict() == {}, 'empty'
assert repr(OrderedDict()) == 'OrderedDict()', 'empty repr'
assert repr(OrderedDict({'a': 1})) == "OrderedDict({'a': 1})", 'repr'
assert list(OrderedDict([('b', 2), ('a', 1)])) == ['b', 'a'], 'pairs keep order'
assert list(OrderedDict(b=2, a=1)) == ['b', 'a'], 'kwargs keep order'
assert list(OrderedDict({'x': 1}, y=2)) == ['x', 'y'], 'mapping plus kwargs'

try:
    OrderedDict(1, 2)
    assert False, 'two positionals'
except TypeError as e:
    assert str(e) == 'expected at most 1 argument, got 2', str(e)

# === Type and isinstance ===
od = OrderedDict(a=1)
assert isinstance(od, dict), 'is a dict'
assert isinstance(od, OrderedDict), 'is an OrderedDict'
assert not isinstance({}, OrderedDict), 'plain dict is not'
assert type(od) is OrderedDict, 'type()'
assert type(od).__name__ == 'OrderedDict', 'name'

# === Equality ===
assert OrderedDict(a=1, b=2) != OrderedDict(b=2, a=1), 'order-sensitive'
assert OrderedDict(a=1, b=2) == OrderedDict(a=1, b=2), 'same order'
assert OrderedDict(a=1, b=2) == {'b': 2, 'a': 1}, 'order-insensitive vs dict'
assert {'b': 2, 'a': 1} == OrderedDict(a=1, b=2), 'order-insensitive vs dict, reversed'
assert OrderedDict(a=1) != OrderedDict(a=2), 'values differ'

# === move_to_end / popitem ===
od = OrderedDict([('a', 1), ('b', 2), ('c', 3)])
od.move_to_end('a')
assert list(od) == ['b', 'c', 'a'], 'move to end'
od.move_to_end('a', last=False)
assert list(od) == ['a', 'b', 'c'], 'move to front'
od.move_to_end('b', False)
assert list(od) == ['b', 'a', 'c'], 'positional last'
od.move_to_end('c')
assert list(od) == ['b', 'a', 'c'], 'already last'
try:
    od.move_to_end('z')
    assert False, 'missing key'
except KeyError as e:
    assert e.args == ('z',), 'key in KeyError'

assert od.popitem() == ('c', 3), 'popitem last'
assert od.popitem(last=False) == ('b', 2), 'popitem first'
assert od.popitem(False) == ('a', 1), 'popitem positional'
try:
    od.popitem()
    assert False, 'empty popitem'
except KeyError as e:
    assert str(e) == "'dictionary is empty'", 'empty message'

# === reversed, views ===
od = OrderedDict([('a', 1), ('b', 2), ('c', 3)])
assert list(reversed(od)) == ['c', 'b', 'a'], 'reversed'
assert list(od.keys()) == ['a', 'b', 'c'], 'keys'
assert list(od.values()) == [1, 2, 3], 'values'
assert list(od.items()) == [('a', 1), ('b', 2), ('c', 3)], 'items'
od.move_to_end('a')
assert list(od.items()) == [('b', 2), ('c', 3), ('a', 1)], 'items after move'
assert list(reversed(od.keys())) == ['a', 'c', 'b'], 'reversed keys'

# === copy, fromkeys ===
od = OrderedDict([('b', 1), ('a', 2)])
c = od.copy()
assert c == od and list(c) == ['b', 'a'], 'copy'
assert type(c) is OrderedDict, 'copy type'
c['z'] = 0
assert 'z' not in od, 'copy is independent'
f = OrderedDict.fromkeys(['x', 'y', 'z'], 0)
assert type(f) is OrderedDict and list(f) == ['x', 'y', 'z'], 'fromkeys'
assert repr(f) == "OrderedDict({'x': 0, 'y': 0, 'z': 0})", 'fromkeys repr'
f2 = od.fromkeys('ab')
assert type(f2) is OrderedDict, 'instance fromkeys'
cc = copy.copy(od)
assert type(cc) is OrderedDict and list(cc) == ['b', 'a'], 'copy.copy'
dc = copy.deepcopy(od)
assert type(dc) is OrderedDict and dc == od, 'deepcopy'

# === | and |= ===
a = OrderedDict([('a', 1), ('b', 2)])
r = a | {'c': 3, 'a': 9}
assert type(r) is OrderedDict and list(r.items()) == [('a', 9), ('b', 2), ('c', 3)], 'or'
r = {'z': 0} | a
assert type(r) is OrderedDict, 'dict | OrderedDict'
assert type(a | OrderedDict(q=1)) is OrderedDict, 'od | od'
a |= {'d': 4}
assert type(a) is OrderedDict and list(a) == ['a', 'b', 'd'], 'ior'

# === Interop ===
od = OrderedDict([('b', 1), ('a', 2)])
assert json.dumps(od) == '{"b": 1, "a": 2}', 'json'
d = dict(od)
assert type(d) is dict and list(d) == ['b', 'a'], 'dict()'
assert OrderedDict(od) == od and list(OrderedDict(od)) == ['b', 'a'], 'OrderedDict(od)'
assert len(od) == 2 and 'a' in od and od['b'] == 1, 'dict protocol'
od.pop('b')
assert list(od) == ['a'], 'pop'
od['n'] = 5
assert list(od) == ['a', 'n'], 'insert at end'
od['a'] = 7
assert list(od) == ['a', 'n'], 'reassign keeps position'
assert od.pop('a') == 7 and list(od) == ['n'], 'pop'
assert od.setdefault('s', 1) == 1 and list(od) == ['n', 's'], 'setdefault'
od.update(t=2)
assert list(od) == ['n', 's', 't'], 'update'
assert sorted(od) == ['n', 's', 't'], 'sorted'

# === Misc ===
assert OrderedDict(a=1) == OrderedDict(a=1.0), 'value equality'
assert OrderedDict([(1, 2)]) != [(1, 2)], 'not equal to a list'
assert str(OrderedDict(a=1)) == "OrderedDict({'a': 1})", 'str'
assert repr(type(OrderedDict())) == "<class 'collections.OrderedDict'>", 'type repr'
o = OrderedDict(a=1)
o.clear()
assert repr(o) == 'OrderedDict()' and not o, 'clear'
assert {**OrderedDict(a=1)} == {'a': 1}, 'unpack'

# Reordering while iterating is an error (the message differs from CPython).
o = OrderedDict(a=1, b=2, c=3)
try:
    for k in o:
        o.move_to_end(k)
    assert False, 'no error'
except RuntimeError:
    pass

# === Subscripting ===
assert repr(OrderedDict[str, int]) == 'collections.OrderedDict[str, int]', 'class_getitem'
