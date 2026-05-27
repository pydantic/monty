# `__exit__` raises on the normal-exit path (the body completed cleanly,
# but the cleanup itself fails). The traceback points at the `with` line:
# Monty surfaces it from the `WithExit` opcode's location, CPython from
# the call-site frame. The shim's `__exit__` frame inside
# `test_fixtures.py` is filtered out, matching the Monty side which has
# no equivalent Python-level frame.
with _test_cm('raise_on_exit', 'fail-exit'):
    pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_exit_raises_normal_exit_traceback.py", line 7, in <module>
    with _test_cm('raise_on_exit', 'fail-exit'):
         ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: fail-exit
"""
