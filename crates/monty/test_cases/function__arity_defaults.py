# Arity errors for functions whose positional params have defaults: CPython
# reports the range form for too-many ('takes from X to Y positional
# arguments') and counts only *required* params as missing.


def f(a, b=1):
    return a + b


def g(a, b, c=1, d=2):
    return a + b + c + d


try:
    f(1, 2, 3)
    assert False, 'f(1, 2, 3) should raise TypeError'
except TypeError as e:
    assert str(e) == 'f() takes from 1 to 2 positional arguments but 3 were given', f'range too-many: {e}'

try:
    f()
    assert False, 'f() should raise TypeError'
except TypeError as e:
    assert str(e) == "f() missing 1 required positional argument: 'a'", f'defaults not missing: {e}'

try:
    g(1)
    assert False, 'g(1) should raise TypeError'
except TypeError as e:
    assert str(e) == "g() missing 1 required positional argument: 'b'", f'only required missing: {e}'

try:
    g()
    assert False, 'g() should raise TypeError'
except TypeError as e:
    assert str(e) == "g() missing 2 required positional arguments: 'a' and 'b'", f'two missing joined: {e}'

try:
    g(1, 2, 3, 4, 5)
    assert False, 'g(1, 2, 3, 4, 5) should raise TypeError'
except TypeError as e:
    assert str(e) == 'g() takes from 2 to 4 positional arguments but 5 were given', f'range too-many g: {e}'


def h(a, b, c):
    return a


try:
    h()
    assert False, 'h() should raise TypeError'
except TypeError as e:
    assert str(e) == "h() missing 3 required positional arguments: 'a', 'b', and 'c'", f'oxford comma: {e}'


# === keyword errors beat too-many-positional (CPython binds kwargs first) ===
def k1(a):
    return a


try:
    k1(1, 2, bad=3)
    assert False, 'k1(1, 2, bad=3) should raise TypeError'
except TypeError as e:
    assert str(e) == "k1() got an unexpected keyword argument 'bad'", f'unknown kwarg beats overflow: {e}'

try:
    k1(1, 2, a=3)
    assert False, 'k1(1, 2, a=3) should raise TypeError'
except TypeError as e:
    assert str(e) == "k1() got multiple values for argument 'a'", f'duplicate beats overflow: {e}'


def k2(a, *, c=1):
    return a


# A kwarg that binds cleanly to a keyword-only param leaves the overflow to
# fire, counted in the `(and N keyword-only argument(s))` suffix — only
# *bound* kw-only params count, defaults and unknown names do not.
try:
    k2(1, 2, c=3)
    assert False, 'k2(1, 2, c=3) should raise TypeError'
except TypeError as e:
    assert (
        str(e) == 'k2() takes 1 positional argument but 2 positional arguments (and 1 keyword-only argument) were given'
    ), f'kwonly suffix counts bound params: {e}'

try:
    k2(1, 2, c=3, bad=4)
    assert False, 'k2(1, 2, c=3, bad=4) should raise TypeError'
except TypeError as e:
    assert str(e) == "k2() got an unexpected keyword argument 'bad'", f'unknown kwarg beats overflow with kwonly: {e}'

try:
    k2(1, 2)
    assert False, 'k2(1, 2) should raise TypeError'
except TypeError as e:
    assert str(e) == 'k2() takes 1 positional argument but 2 were given', f'no suffix when no kwonly bound: {e}'


def k3(a, b=1, *, c, d=2):
    return a


try:
    k3(1, 2, 3, c=5, d=6)
    assert False, 'k3(1, 2, 3, c=5, d=6) should raise TypeError'
except TypeError as e:
    assert (
        str(e)
        == 'k3() takes from 1 to 2 positional arguments but 3 positional arguments (and 2 keyword-only arguments) were given'
    ), f'plural kwonly suffix: {e}'
