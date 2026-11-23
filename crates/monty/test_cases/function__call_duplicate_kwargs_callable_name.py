# Duplicate keyword errors from `**` unpacking name the function being called,
# not the expression used to call it.
def f(**kwargs):
    return kwargs


# === an alias reports the def name ===
g = f
try:
    g(a=1, **{'a': 2})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "f() got multiple values for keyword argument 'a'", str(exc)


# === two mappings through an alias ===
try:
    g(**{'a': 1}, **{'a': 2})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "f() got multiple values for keyword argument 'a'", str(exc)


# === a lambda reports <lambda> ===
lam = lambda **kwargs: kwargs
try:
    lam(a=1, **{'a': 2})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "<lambda>() got multiple values for keyword argument 'a'", str(exc)


# === a function with defaults, through a dict lookup ===
def h(x=0, **kwargs):
    return x


funcs = {'k': h}
try:
    funcs['k'](x=1, **{'x': 2})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "h() got multiple values for keyword argument 'x'", str(exc)


# === builtins keep their own name ===
try:
    print(sep='', **{'sep': ''})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "print() got multiple values for keyword argument 'sep'", str(exc)
