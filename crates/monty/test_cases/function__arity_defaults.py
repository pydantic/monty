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


# === More than 64 named parameters ===
def many_params(
    p00,
    p01,
    p02,
    p03,
    p04,
    p05,
    p06,
    p07,
    p08,
    p09,
    p10,
    p11,
    p12,
    p13,
    p14,
    p15,
    p16,
    p17,
    p18,
    p19,
    p20,
    p21,
    p22,
    p23,
    p24,
    p25,
    p26,
    p27,
    p28,
    p29,
    p30,
    p31,
    p32,
    p33,
    p34,
    p35,
    p36,
    p37,
    p38,
    p39,
    p40,
    p41,
    p42,
    p43,
    p44,
    p45,
    p46,
    p47,
    p48,
    p49,
    p50,
    p51,
    p52,
    p53,
    p54,
    p55,
    p56,
    p57,
    p58,
    p59,
    p60,
    p61,
    p62,
    p63,
    p64=640,
    *,
    flag,
):
    return p00, p63, p64, flag


assert many_params(*range(64), flag=True) == (0, 63, 640, True)
assert many_params(*range(64), p64=64, flag=False) == (0, 63, 64, False)
assert many_params(*range(65), flag=True) == (0, 63, 64, True)

try:
    many_params(*range(65), p64=65, flag=True)
    assert False, 'expected duplicate argument error'
except TypeError as e:
    assert str(e) == "many_params() got multiple values for argument 'p64'"

try:
    many_params(*range(65))
    assert False, 'expected missing keyword-only argument error'
except TypeError as e:
    assert str(e) == "many_params() missing 1 required keyword-only argument: 'flag'"
