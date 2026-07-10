# === Dict literals ===
assert {} == {}
assert {'a': 1} == {'a': 1}
assert {'a': 1, 'b': 2} == {'a': 1, 'b': 2}
assert {1: 'a', 2: 'b'} == {1: 'a', 2: 'b'}

# === Dict length ===
assert len({}) == 0
assert len({'a': 1, 'b': 2, 'c': 3}) == 3

# === Dict equality ===
assert ({'a': 1, 'b': 2} == {'b': 2, 'a': 1}) == True
assert ({'a': 1} == {'a': 2}) == False

# === Dict subscript get ===
d = {'name': 'Alice', 'age': 30}
assert d['name'] == 'Alice'
assert d['age'] == 30

d = {1: 'one', 2: 'two'}
assert d[1] == 'one'

# === Dict subscript set ===
d = {'a': 1}
d['b'] = 2
assert d == {'a': 1, 'b': 2}

d = {'a': 1}
d['a'] = 99
assert d == {'a': 99}

# === Dict subscript augmented assignment ===
totals = {'photo': 1}
rtype = 'photo'
likes = 2
totals[rtype] += likes
assert totals == {'photo': 3}

calls = 0


def key():
    global calls
    calls += 1
    return 'photo'


totals = {'photo': 10}
totals[key()] += 5
assert totals == {'photo': 15}
assert calls == 1

captured_total = {'photo': 1}
captured_likes = 2


def apply_captured_increment():
    captured_total['photo'] += captured_likes


apply_captured_increment()
assert captured_total == {'photo': 3}

walrus_key = None
walrus_total = {'photo': 10}
walrus_total[(walrus_key := 'photo')] += 4
assert walrus_key == 'photo'
assert walrus_total == {'photo': 14}

try:
    missing = {}
    missing['photo'] += 1
    assert False, 'subscript += on a missing dict key should raise KeyError'
except KeyError as e:
    assert e.args == ('photo',)

try:
    existing = {'photo': 'a'}
    existing['photo'] += 1
    assert False, 'subscript += with incompatible operand types should raise TypeError'
except TypeError as e:
    assert e.args == ('can only concatenate str (not "int") to str',)
    assert existing == {'photo': 'a'}

# === Dict.get() method ===
d = {'a': 1, 'b': 2}
assert d.get('a') == 1
assert d.get('missing') is None
assert d.get('missing', 'default') == 'default'

# === Dict.pop() method ===
d = {'a': 1, 'b': 2}
assert d.pop('a') == 1
assert d == {'b': 2}

d = {'a': 1}
assert d.pop('missing', 'default') == 'default'

# === Dict with tuple key ===
d = {(1, 2): 'value'}
assert d[(1, 2)] == 'value'

# === Dict repr ===
assert repr({}) == '{}'
assert repr({'a': 1}) == "{'a': 1}"

# === Dict self-reference ===
d = {}
d['self'] = d
assert d['self'] is d

d = {}
assert d.get('missing', d) is d

# === Dict unpacking (PEP 448) ===
a = {'x': 1, 'y': 2}
b = {'y': 99, 'z': 3}
assert {**a} == {'x': 1, 'y': 2}
assert {**a, **b} == {'x': 1, 'y': 99, 'z': 3}
assert {**a, 'y': 0} == {'x': 1, 'y': 0}
assert {'y': 0, **a} == {'y': 2, 'x': 1}
assert {**a, 'z': 3} == {'x': 1, 'y': 2, 'z': 3}
assert {**{}} == {}
assert {**a, **b, 'w': 4} == {'x': 1, 'y': 99, 'z': 3, 'w': 4}
assert list({**a, 'z': 3}.keys()) == ['x', 'y', 'z']

# === Dict unpack TypeError for non-mapping heap ref ===
# Unpacking a Ref that is NOT a dict (e.g. a list) should raise TypeError
try:
    x = {**[1, 2, 3]}
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'list' object is not a mapping", f'wrong error: {e}'

# === Duplicate literal keys: last value wins, replaced value is released ===
dup = {'k': [1], 'k': [2]}
assert dup == {'k': [2]}
assert len(dup) == 1
