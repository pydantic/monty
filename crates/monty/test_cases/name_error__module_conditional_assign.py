# Module-scope read of a name whose later assignment didn't execute must raise
# `NameError`, not `UnboundLocalError` — module scope only has `NameError`.
#
# Regression: the previous compile path tagged module-scope reads of an
# already-known name as `Local`, which called `register_assigned_local` on the
# shared global slot. Once the slot was flagged, any later runtime path that
# hit the slot while `Undefined` (because the assignment was skipped by an
# exception, an `if False:`, etc.) would raise `UnboundLocalError`. Fixed by
# always tagging module-scope references as `NameScope::Global`, which never
# calls `register_assigned_local`.


def boom():
    raise ValueError('nope')


try:
    boom()
    foo = 1
except ValueError:
    pass

print(foo)
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__module_conditional_assign.py", line 23, in <module>
    print(foo)
          ~~~
NameError: name 'foo' is not defined
"""
