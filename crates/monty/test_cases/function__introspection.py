# /// monty
# cpython-main-module = true
# ///

"""Tests for function, builtin, module function, and bound method introspection attributes."""


# --- Plain def function ---
def my_function():
    pass


assert my_function.__name__ == 'my_function'
assert my_function.__qualname__ == 'my_function'
assert my_function.__doc__ is None
assert my_function.__module__ == '__main__'


# --- Function with defaults ---
def with_defaults(x=1, y='hello'):
    return x


assert with_defaults.__name__ == 'with_defaults'
assert with_defaults.__qualname__ == 'with_defaults'
assert with_defaults.__doc__ is None
assert with_defaults.__module__ == '__main__'

# --- Lambda expression ---
lam = lambda x: x * 2
assert lam.__name__ == '<lambda>'
assert lam.__qualname__ == '<lambda>'
assert lam.__doc__ is None
assert lam.__module__ == '__main__'


# --- Async and Generator functions ---
async def async_fn():
    pass


assert async_fn.__name__ == 'async_fn'
assert async_fn.__qualname__ == 'async_fn'
assert async_fn.__doc__ is None
assert async_fn.__module__ == '__main__'


def gen_fn():
    yield 1


assert gen_fn.__name__ == 'gen_fn'
assert gen_fn.__qualname__ == 'gen_fn'
assert gen_fn.__doc__ is None
assert gen_fn.__module__ == '__main__'


# --- Closures ---
def make_counter():
    count = 0

    def step():
        nonlocal count
        count += 1
        return count

    return step


counter = make_counter()
assert counter.__name__ == 'step'
assert counter.__doc__ is None
assert counter.__module__ == '__main__'

# --- Builtin functions ---
assert len.__name__ == 'len'
assert len.__qualname__ == 'len'
assert len.__doc__ is None or isinstance(len.__doc__, str)
assert len.__module__ == 'builtins'

assert print.__name__ == 'print'
assert print.__qualname__ == 'print'
assert print.__doc__ is None or isinstance(print.__doc__, str)
assert print.__module__ == 'builtins'

assert abs.__name__ == 'abs'
assert abs.__qualname__ == 'abs'
assert abs.__module__ == 'builtins'

assert object.__init__.__name__ == '__init__'
assert object.__init__.__qualname__ == 'object.__init__'

assert object.__new__.__name__ == '__new__'
assert object.__new__.__qualname__ == 'object.__new__'

# --- Module functions ---
import math

assert math.sqrt.__name__ == 'sqrt'
assert math.sqrt.__qualname__ == 'sqrt'
assert math.sqrt.__doc__ is None or isinstance(math.sqrt.__doc__, str)
assert math.sqrt.__module__ == 'math'

import os

assert os.getcwd.__name__ == 'getcwd'
assert os.getcwd.__qualname__ == 'getcwd'
assert os.getcwd.__module__ in ('os', 'posix', 'nt')

import os.path

assert os.path.join.__name__ == 'join'
assert os.path.join.__qualname__ == 'join'
assert os.path.join.__module__ in ('posixpath', 'ntpath')

import json

assert json.dumps.__name__ == 'dumps'
assert json.dumps.__qualname__ == 'dumps'
assert json.dumps.__module__ == 'json'

import re

assert re.compile.__name__ == 'compile'
assert re.compile.__qualname__ == 'compile'
assert re.compile.__module__ == 're'

import time

assert time.time.__name__ == 'time'
assert time.time.__qualname__ == 'time'
assert time.time.__module__ == 'time'

# --- Dynamic __module__ from globals dict ---
ns_custom = {'__name__': 'custom_module'}
exec('def custom_fn(): pass', ns_custom)
assert ns_custom['custom_fn'].__module__ == 'custom_module'

exec(
    """
def outer_fn():
    x = 1
    def inner_fn():
        return x
    return inner_fn
""",
    ns_custom,
)
assert ns_custom['outer_fn']().__module__ == 'custom_module'

ns_no_name = {}
exec('def no_name_fn(): pass', ns_no_name)
assert ns_no_name['no_name_fn'].__module__ is None


# --- Bound methods on user classes ---
class Greeter:
    def greet(self):
        return 'hi'


g = Greeter()
assert g.greet.__name__ == 'greet'
assert g.greet.__doc__ is None
assert g.greet.__module__ == '__main__'
assert g.greet.__func__ is not None
assert g.greet.__self__ is g

# --- Error handling for missing attributes ---
try:
    my_function.__nonexistent__
except AttributeError:
    pass
else:
    raise AssertionError('expected AttributeError for non-existent attribute')

try:
    len.__nonexistent__
except AttributeError:
    pass
else:
    raise AssertionError('expected AttributeError for non-existent attribute')

try:
    math.sqrt.__nonexistent__
except AttributeError:
    pass
else:
    raise AssertionError('expected AttributeError for non-existent attribute')
