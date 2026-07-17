# === Basic function calls ===
def f_no_args():
    return 1


assert f_no_args() == 1


def f_one_arg(x):
    return x


assert f_one_arg(42) == 42


def add(a, b):
    return a + b


assert add(1, 2) == 3


def sum3(a, b, c):
    return a + b + c


assert sum3(1, 2, 3) == 6


# === Local variables ===
def f_local():
    x = 42
    return x


assert f_local() == 42


def f_local_from_arg(x):
    y = x + 1
    return y


assert f_local_from_arg(10) == 11


def f_local_list():
    items = [1, 2, 3]
    return items


assert f_local_list() == [1, 2, 3]


def f_local_modify_list():
    items = [1, 2]
    items.append(3)
    return items


assert f_local_modify_list() == [1, 2, 3]


def f_local_multiple():
    a = 1
    b = 2
    c = 3
    return a + b + c


assert f_local_multiple() == 6


def f_local_reassign():
    x = 1
    x = 2
    x = 3
    return x


assert f_local_reassign() == 3


# === Nested functions ===
def nested_basic():
    def bar():
        return 1

    return bar() + 1


assert nested_basic() == 2


def nested_deep():
    def level2():
        def level3():
            return 42

        return level3()

    return level2()


assert nested_deep() == 42


def nested_multiple_calls():
    def inner():
        return 10

    return inner() + inner() + inner()


assert nested_multiple_calls() == 30


def nested_two_inner():
    def add():
        return 1

    def sub():
        return 2

    return add() + sub()


assert nested_two_inner() == 3


def nested_with_args(x):
    def inner(y):
        return y + y

    return inner(x) + 1


assert nested_with_args(5) == 11


# === Function equality ===
def eq_test():
    return 1


def eq_test2():
    return 1


# Same function is equal to itself
assert eq_test == eq_test
assert not (eq_test != eq_test), 'function not-not-equals itself'

# Different functions are not equal (even with same body)
assert not (eq_test == eq_test2), 'different functions not equal'
assert eq_test != eq_test2

# Function assigned to variable is still equal
f_alias = eq_test
assert f_alias == eq_test
assert eq_test == f_alias


# === Builtin equality ===
# Same builtin is equal to itself
assert len == len
assert print == print
assert not (len != len), 'builtin not-not-equals itself'

# Builtin identity (is)
assert print is print
assert len is len
assert not (len is print), 'len is not print'

# Different builtins are not equal
assert not (len == print), 'different builtins not equal'
assert len != print

# Builtin assigned to variable is still equal
len_alias = len
assert len_alias == len
assert len_alias is len


# === Exception type equality ===
# Note: Using == instead of 'is' to explicitly test the __eq__ implementation
assert ValueError == ValueError
assert TypeError == TypeError
assert not (ValueError != ValueError), 'exc type not-not-equals itself'

assert not (ValueError == TypeError), 'different exc types not equal'
assert ValueError != TypeError

exc_alias = ValueError
assert exc_alias == ValueError


# === Closure equality ===
def make_adder(n):
    def adder(x):
        return x + n

    return adder


add1 = make_adder(1)
add2 = make_adder(2)
add1_again = make_adder(1)

# Same closure instance equals itself
assert add1 == add1
assert not (add1 != add1), 'closure not-not-equals itself'

# Different closure instances are not equal (even with same captured value)
assert not (add1 == add1_again), 'different closure instances not equal'
assert add1 != add1_again

# Different closure instances with different captured values
assert not (add1 == add2), 'closures with diff captured values not equal'
assert add1 != add2


# === Cross-type inequality ===
def cross_test():
    return 1


assert not (cross_test == len), 'function not equal to builtin'
assert not (len == cross_test), 'builtin not equal to function'
assert not (cross_test == ValueError), 'function not equal to exc type'
assert not (ValueError == cross_test), 'exc type not equal to function'
assert not (len == ValueError), 'builtin not equal to exc type'
assert not (ValueError == len), 'exc type not equal to builtin'

# Callables not equal to other types
assert not (len == 1), 'builtin not equal to int'
assert not (len == 'len'), 'builtin not equal to string'
assert not (cross_test == None), 'function not equal to None'
assert not (ValueError == None), 'exc type not equal to None'


# === Parameter shadowing global variables ===
# Function parameters should shadow global variables with the same name
x = 5


def shadow_single(x):
    return x + 1


# When called with 10, param x=10 should be used, not global x=5
assert shadow_single(10) == 11

y = 3


def shadow_multiple(x, y):
    return x + y


# When called with (20, 30), params should be used, not globals x=5, y=3
assert shadow_multiple(20, 30) == 50


def shadow_uses_global_too(x):
    # x is param, y is global
    return x + y


# x=100 (param), y=3 (global), so 100 + 3 = 103
assert shadow_uses_global_too(100) == 103


def shadow_with_default(x=99):
    return x + 1


# When called with argument, param shadows global
assert shadow_with_default(10) == 11
# When called without argument, default is used (not global)
assert shadow_with_default() == 100


# Global is still accessible outside the function
assert x == 5
assert y == 3


# Verify global can still be used as argument
def double(x):
    return x * 2


assert double(x) == 10
