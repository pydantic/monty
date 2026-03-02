def f(*args, **kwargs):
    return args, kwargs


# === Multiple *args ===
assert f(*[1, 2], *[3, 4]) == ((1, 2, 3, 4), {}), 'multiple star args'
assert f(0, *[1, 2], 3) == ((0, 1, 2, 3), {}), 'positional after star args'
assert f(*[], *[1]) == ((1,), {}), 'unpack empty then non-empty'

# === Multiple **kwargs ===
assert f(**{'a': 1}, **{'b': 2}) == ((), {'a': 1, 'b': 2}), 'multiple star-star kwargs'
assert f(**{'a': 1}, b=2) == ((), {'a': 1, 'b': 2}), 'named after star-star'
assert f(key='before', **{'a': 1}) == ((), {'key': 'before', 'a': 1}), 'named before star-star'

# === Mixed ===
assert f(1, *[2, 3], **{'x': 4}) == ((1, 2, 3), {'x': 4}), 'mixed star and star-star'
