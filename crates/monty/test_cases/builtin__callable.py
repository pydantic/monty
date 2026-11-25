"""Tests for the callable() builtin function."""

import functools
import math

# --- Builtin functions and types ---
assert callable(len) is True
assert callable(print) is True
assert callable(abs) is True
assert callable(callable) is True
assert callable(int) is True
assert callable(float) is True
assert callable(str) is True
assert callable(list) is True
assert callable(dict) is True
assert callable(tuple) is True
assert callable(object) is True


# --- User functions, lambdas, closures ---
def f():
    pass


assert callable(f) is True

g = lambda x: x + 1
assert callable(g) is True


def outer():
    x = 10

    def inner():
        return x

    return inner


assert callable(outer()) is True


# --- Classes and instances ---
class Plain:
    pass


assert callable(Plain) is True
p = Plain()
assert callable(p) is False


class HasCall:
    def __call__(self):
        return 42


assert callable(HasCall) is True
h = HasCall()
assert callable(h) is True
assert h() == 42


class BaseCall:
    def __call__(self):
        return 1


class SubCall(BaseCall):
    pass


assert callable(SubCall()) is True


class CallNone:
    __call__ = None


assert callable(CallNone()) is True

# Instance attribute __call__ does NOT make it callable per CPython protocol
p.call_attr = lambda: 1
p.__call__ = lambda: 1
assert callable(p) is False

# Bound methods
assert callable(h.__call__) is True

# Module functions and partials
assert callable(math.sin) is True
part = functools.partial(len)
assert callable(part) is True
assert callable(list[int]) is True

# --- Non-callable primitives ---
assert callable(0) is False
assert callable(42) is False
assert callable(-100) is False
assert callable(3.14) is False
assert callable(True) is False
assert callable(False) is False
assert callable(None) is False
assert callable('string') is False
assert callable(b'bytes') is False
assert callable([1, 2, 3]) is False
assert callable((1, 2)) is False
assert callable({'a': 1}) is False
assert callable({1, 2}) is False

# --- Error paths: argument counts and kwargs ---
try:
    callable()
except TypeError as e:
    assert str(e) == 'callable() takes exactly one argument (0 given)'
else:
    raise AssertionError('expected TypeError')

try:
    callable(1, 2)
except TypeError as e:
    assert str(e) == 'callable() takes exactly one argument (2 given)'
else:
    raise AssertionError('expected TypeError')

try:
    callable(1, 2, 3)
except TypeError as e:
    assert str(e) == 'callable() takes exactly one argument (3 given)'
else:
    raise AssertionError('expected TypeError')

try:
    callable(obj=len)
except TypeError as e:
    assert str(e) == 'callable() takes no keyword arguments'
else:
    raise AssertionError('expected TypeError')

try:
    callable(len, x=1)
except TypeError as e:
    assert str(e) == 'callable() takes no keyword arguments'
else:
    raise AssertionError('expected TypeError')
