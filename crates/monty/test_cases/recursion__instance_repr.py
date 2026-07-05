# Recursive `__repr__`/`__str__` re-enter via `evaluate_function`; Monty must
# raise `RecursionError` instead of overflowing the native Rust stack.

import sys


def assert_recursion_message(exc, context, *, while_calling=False):
    msg = str(exc)
    if sys.platform == 'monty':
        assert msg == 'maximum recursion depth exceeded', f'unexpected {context} recursion message: {msg}'
    else:
        stack_msg = msg.startswith('Stack overflow (used ') and msg.endswith(
            ' kB) while calling a Python object' if while_calling else ' kB'
        )
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
    assert_recursion_message(exc, '__init__', while_calling=True)
