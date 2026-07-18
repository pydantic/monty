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


# === Default with keyword args ===
def f_kw(a, b=10):
    return a + b


assert f_kw(1, b=20) == 21
assert f_kw(a=5) == 15
assert f_kw(a=5, b=3) == 8


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
