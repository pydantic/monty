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
