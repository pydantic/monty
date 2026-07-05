# Recursive `__repr__`/`__str__` re-enter via `evaluate_function`; Monty must
# raise `RecursionError` instead of overflowing the native Rust stack.

import sys


def assert_recursion_message(exc, context):
    msg = str(exc)
    if sys.platform == 'monty':
        assert msg == 'maximum recursion depth exceeded', f'unexpected {context} recursion message: {msg}'
    else:
        # CPython may hit its recursion counter, or (on smaller C stacks like the
        # datatest worker / macOS / Windows CI) a native stack-overflow guard. That
        # guard's message reports the kB used and appends a context-specific suffix
        # (`) while calling a Python object`, `) while getting the repr of an
        # object`, ... or nothing), all of which contain ` kB)`.
        stack_msg = msg.startswith('Stack overflow (used ') and ' kB)' in msg
        assert msg == 'maximum recursion depth exceeded' or stack_msg, f'unexpected {context} recursion message: {msg}'


class SelfRepr:
    def __repr__(self):
        return repr(self)


try:
    repr(SelfRepr())
    raise AssertionError('expected RecursionError from self-referential __repr__')
except RecursionError as exc:
    assert_recursion_message(exc, 'repr')


class SelfStr:
    def __str__(self):
        return str(self)


try:
    str(SelfStr())
    raise AssertionError('expected RecursionError from self-referential __str__')
except RecursionError as exc:
    assert_recursion_message(exc, 'str')


# === Positive case: a modest finite chain still reprs correctly ===
class Node:
    def __init__(self, value, child=None):
        self.value = value
        self.child = child

    def __repr__(self):
        if self.child is None:
            return f'Node({self.value})'
        return f'Node({self.value}, {self.child!r})'


chain = None
for i in range(5):
    chain = Node(i, chain)
result = repr(chain)
assert result == 'Node(4, Node(3, Node(2, Node(1, Node(0)))))', f'unexpected repr: {result}'


# === Guard-placement regression ===
# A class-valued `__init__` recurses inside `call_function` before any frame is
# pushed, so the re-entry guard must be charged at `evaluate_function` entry.
class A:
    pass


A.__init__ = A

try:
    A()
    raise AssertionError('expected RecursionError from class-valued __init__ cycle')
except RecursionError as exc:
    assert_recursion_message(exc, '__init__')
