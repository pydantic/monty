# === functions defined under a globals dict read it at call time ===
ns = {}
exec('def f():\n    return k\nk = 3', ns)
assert ns['f']() == 3
ns['k'] = 4
assert ns['f']() == 4

# === global statements inside such functions write the dict ===
exec('def g():\n    global k\n    k = 5', ns)
ns['g']()
assert ns['k'] == 5

# === imports land in the dict ===
exec('import math\ndef area(r):\n    return math.pi * r * r', ns)
assert round(ns['area'](2), 3) == 12.566
assert ns['math'].pi == ns['area'](1)

# === classes and methods ===
exec('class C:\n    k = 5\n    def m(self):\n        return k2\nk2 = 9', ns)
assert ns['C']().m() == 9
assert ns['C'].k == 5

# === closures ===
exec('def outer(n):\n    def inner():\n        return n + k\n    return inner', ns)
assert ns['outer'](1)() == 6

# === defaults ===
exec('def d(a, b=k):\n    return a + b', ns)
assert ns['d'](1) == 6

# === nested exec and eval inherit the dict namespace ===
exec('exec("inner_k = 11")', ns)
assert ns['inner_k'] == 11
exec('def w(p):\n    return eval("p + k")', ns)
assert ns['w'](1) == 6

# === comprehensions read the dict ===
assert eval('[k + i for i in range(2)]', {'k': 1}) == [1, 2]

# === separate globals and locals ===
g, loc = {}, {}
exec('a = 1\nglobal b\nb = 2', g, loc)
assert loc == {'a': 1}
assert g['b'] == 2
assert 'b' not in loc, 'global binds in the globals dict'

# === eval with only locals ===
assert eval('q * 2', None, {'q': 7}) == 14

# === dict names shadow builtins ===
assert eval('len', {'len': 3}) == 3

# === names missing from an explicit namespace ===
try:
    exec('qq', {})
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'qq' is not defined"
outer_name = 1
try:
    eval('outer_name', {})
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'outer_name' is not defined"
