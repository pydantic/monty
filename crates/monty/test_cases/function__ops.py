# === Basic function calls ===
def f_no_args():
    return 1


assert f_no_args() == 1, 'no args'


def f_one_arg(x):
    return x


assert f_one_arg(42) == 42, 'one arg'


def add(a, b):
    return a + b


assert add(1, 2) == 3, 'two args'


def sum3(a, b, c):
    return a + b + c


assert sum3(1, 2, 3) == 6, 'three args'


# === Local variables ===
def f_local():
    x = 42
    return x


assert f_local() == 42, 'local var'


def f_local_from_arg(x):
    y = x + 1
    return y


assert f_local_from_arg(10) == 11, 'local var from arg'


def f_local_list():
    items = [1, 2, 3]
    return items


assert f_local_list() == [1, 2, 3], 'local var list'


def f_local_modify_list():
    items = [1, 2]
    items.append(3)
    return items


assert f_local_modify_list() == [1, 2, 3], 'local var modify list'


def f_local_multiple():
    a = 1
    b = 2
    c = 3
    return a + b + c


assert f_local_multiple() == 6, 'local var multiple'


def f_local_reassign():
    x = 1
    x = 2
    x = 3
    return x


assert f_local_reassign() == 3, 'local var reassign'


# === Nested functions ===
def nested_basic():
    def bar():
        return 1

    return bar() + 1


assert nested_basic() == 2, 'nested basic'


def nested_deep():
    def level2():
        def level3():
            return 42

        return level3()

    return level2()


assert nested_deep() == 42, 'nested deep'


def nested_multiple_calls():
    def inner():
        return 10

    return inner() + inner() + inner()


assert nested_multiple_calls() == 30, 'nested multiple calls'


def nested_two_inner():
    def add():
        return 1

    def sub():
        return 2

    return add() + sub()


assert nested_two_inner() == 3, 'nested two inner'


def nested_with_args(x):
    def inner(y):
        return y + y

    return inner(x) + 1


assert nested_with_args(5) == 11, 'nested with args'


# === Function equality ===
def eq_test():
    return 1


def eq_test2():
    return 1


# Same function is equal to itself
assert eq_test == eq_test, 'function equals itself'
assert not (eq_test != eq_test), 'function not-not-equals itself'

# Different functions are not equal (even with same body)
assert not (eq_test == eq_test2), 'different functions not equal'
assert eq_test != eq_test2, 'different functions are not equal'

# Function assigned to variable is still equal
f_alias = eq_test
assert f_alias == eq_test, 'function alias equals original'
assert eq_test == f_alias, 'original equals function alias'


# === Builtin equality ===
# Same builtin is equal to itself
assert len == len, 'builtin equals itself'
assert print == print, 'print equals itself'
assert not (len != len), 'builtin not-not-equals itself'

# Builtin identity (is)
assert print is print, 'print is print'
assert len is len, 'len is len'
assert not (len is print), 'len is not print'

# Different builtins are not equal
assert not (len == print), 'different builtins not equal'
assert len != print, 'different builtins are not equal'

# Builtin assigned to variable is still equal
len_alias = len
assert len_alias == len, 'builtin alias equals original'
assert len_alias is len, 'builtin alias is original'


# === Exception type equality ===
# Note: Using == instead of 'is' to explicitly test the __eq__ implementation
assert ValueError == ValueError, 'exc type equals itself'
assert TypeError == TypeError, 'exc type equals itself 2'
assert not (ValueError != ValueError), 'exc type not-not-equals itself'

assert not (ValueError == TypeError), 'different exc types not equal'
assert ValueError != TypeError, 'different exc types are not equal'

exc_alias = ValueError
assert exc_alias == ValueError, 'exc type alias equals original'


# === Closure equality ===
def make_adder(n):
    def adder(x):
        return x + n

    return adder


add1 = make_adder(1)
add2 = make_adder(2)
add1_again = make_adder(1)

# Same closure instance equals itself
assert add1 == add1, 'closure equals itself'
assert not (add1 != add1), 'closure not-not-equals itself'

# Different closure instances are not equal (even with same captured value)
assert not (add1 == add1_again), 'different closure instances not equal'
assert add1 != add1_again, 'different closure instances are not equal'

# Different closure instances with different captured values
assert not (add1 == add2), 'closures with diff captured values not equal'
assert add1 != add2, 'closures with diff captured values are not equal'


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
assert shadow_single(10) == 11, 'param shadows global - single param'

y = 3


def shadow_multiple(x, y):
    return x + y


# When called with (20, 30), params should be used, not globals x=5, y=3
assert shadow_multiple(20, 30) == 50, 'param shadows global - multiple params'


def shadow_uses_global_too(x):
    # x is param, y is global
    return x + y


# x=100 (param), y=3 (global), so 100 + 3 = 103
assert shadow_uses_global_too(100) == 103, 'param shadows but can still access other globals'


def shadow_with_default(x=99):
    return x + 1


# When called with argument, param shadows global
assert shadow_with_default(10) == 11, 'param with default shadows global'
# When called without argument, default is used (not global)
assert shadow_with_default() == 100, 'param default used, not global'


# Global is still accessible outside the function
assert x == 5, 'global still accessible after function that shadows it'
assert y == 3, 'other global still accessible'


# Verify global can still be used as argument
def double(x):
    return x * 2


assert double(x) == 10, 'global used as argument, param shadows inside'
