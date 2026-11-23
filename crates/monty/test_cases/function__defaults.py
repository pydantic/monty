# Tests for default parameter values in function definitions

# === Basic default values ===
def f_basic(a, b=10):
    return a + b


assert f_basic(1) == 11
assert f_basic(1, 2) == 3
assert f_basic(5) == 15


# === Multiple defaults ===
def f_multi(a=1, b=2):
    return a + b


assert f_multi() == 3
assert f_multi(10) == 12
assert f_multi(10, 20) == 30


# === Mixed required and default ===
def f_mixed(a, b, c=3, d=4):
    return a + b + c + d


assert f_mixed(1, 2) == 10
assert f_mixed(1, 2, 30) == 37
assert f_mixed(1, 2, 30, 40) == 73


# === Trailing defaults across positional-only parameters ===
def f_posonly(a, b=2, /, c=3, d=4):
    return a, b, c, d


assert f_posonly(1) == (1, 2, 3, 4)
assert f_posonly(1, 20, 30) == (1, 20, 30, 4)
assert f_posonly(1, d=40) == (1, 2, 3, 40)
try:
    f_posonly()
    assert False, 'expected TypeError for a missing required parameter'
except TypeError as exc:
    assert str(exc) == "f_posonly() missing 1 required positional argument: 'a'"


# === Default with keyword args ===
def f_kw(a, b=10):
    return a + b


assert f_kw(1, b=20) == 21
assert f_kw(a=5) == 15
assert f_kw(a=5, b=3) == 8


# === Reordered keywords with omitted and positional-only defaults ===
def f_sparse(a, b=2, c=3, d=4):
    return a, b, c, d


assert f_sparse(d=40, a=10) == (10, 2, 3, 40)
assert f_sparse(b=20, c=30, a=10) == (10, 20, 30, 4)

try:
    f_sparse(d=[], b=[])
    assert False, 'expected TypeError for a missing required parameter'
except TypeError as exc:
    assert str(exc) == "f_sparse() missing 1 required positional argument: 'a'"

try:
    f_sparse([], d=[], a=[])
    assert False, 'expected TypeError for a supplied parameter repeated by keyword'
except TypeError as exc:
    assert str(exc) == "f_sparse() got multiple values for argument 'a'"


def f_omitted_posonly(a=1, /, b=2, c=3):
    return a, b, c


assert f_omitted_posonly(c=30) == (1, 2, 30)


# === Default expressions evaluated at definition ===
# Test that default is evaluated once at definition time
def value_maker():
    return 42


def f_eval(x=value_maker()):
    return x


# value_maker was called once at function definition time
assert f_eval() == 42
assert f_eval() == 42


# === Mutable default (Python gotcha - shared across calls) ===
def f_mutable(lst=[]):
    lst.append(1)
    return lst


first_result = f_mutable()
assert first_result == [1]
second_result = f_mutable()
assert second_result == [1, 1]
assert first_result is second_result


assert (lambda value=[42]: value)() == [42]
assert (lambda left=[42], right=None: (left, right))(right=[43]) == ([42], [43])

namespace = {'marker': 17}
exec('def f(value=[42]):\n    return marker, value', namespace)
assert namespace['f']() == (17, [42])
assert namespace['f'](value=[43]) == (17, [43])


# === Multiple functions with separate defaults ===
def f_sep1(x=[]):
    x.append('a')
    return x


def f_sep2(x=[]):
    x.append('b')
    return x


r1 = f_sep1()
r2 = f_sep2()
assert r1 == ['a']
assert r2 == ['b']
assert r1 is not r2


# === Default referencing earlier param (not supported, different test) ===


# === Closure with defaults ===
def make_adder(n):
    def add(x, y=n):
        return x + y

    return add


add5 = make_adder(5)
assert add5(10) == 15
assert add5(10, 3) == 13

add10 = make_adder(10)
assert add10(1) == 11

# Verify the two closures have independent defaults
assert add5(1) == 6


# === Keyword-only defaults interleaved ===
def kwonly_mix(*, head=1, mid, tail=3):
    return head, mid, tail


assert kwonly_mix(mid=2) == (1, 2, 3)
assert kwonly_mix(head=5, mid=7) == (5, 7, 3)
