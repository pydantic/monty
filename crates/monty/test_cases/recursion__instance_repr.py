# Recursive `__repr__`/`__str__` re-enter the interpreter on the native Rust
# stack via `evaluate_function`, exactly like the map/filter/sorted callbacks
# in recursion__nested_eval.py. See that file's header for why both
# interpreters raise RecursionError here rather than one crashing.


class SelfRepr:
    def __repr__(self):
        return repr(self)


try:
    repr(SelfRepr())
    raise AssertionError('expected RecursionError from self-referential __repr__')
except RecursionError:
    pass


class SelfStr:
    def __str__(self):
        return str(self)


try:
    str(SelfStr())
    raise AssertionError('expected RecursionError from self-referential __str__')
except RecursionError:
    pass


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
for i in range(10):
    chain = Node(i, chain)
result = repr(chain)
assert result.startswith('Node(9,'), f'unexpected repr: {result}'


# === CRITICAL regression test: guard-placement pin ===
# `A.__init__ = A` makes `A`'s own initializer a *class value* — an "exotic"
# initializer per `instantiate_class`. Calling `A()` cycles
# `evaluate_function -> call_function -> instantiate_class -> evaluate_function
# -> ...` entirely inside `call_function`, WITHOUT ever pushing a VM frame and
# WITHOUT ever reaching `self.run()`. A native-reentry guard placed only
# around the `self.run()` call inside `evaluate_function` (rather than at
# `evaluate_function`'s entry, before `call_function` is even invoked) would
# NOT catch this cycle, and this test would crash the test process instead of
# cleanly raising RecursionError. If this test starts crashing the test
# runner instead of passing, check that the native-reentry guard in
# `evaluate_function` has not been "simplified" back to wrapping only the
# `self.run()` call.
class A:
    pass


A.__init__ = A

try:
    A()
    raise AssertionError('expected RecursionError from class-valued __init__ cycle')
except RecursionError:
    pass
