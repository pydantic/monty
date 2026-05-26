# `__enter__` raises before the body runs. The traceback shows the `with`
# line — Monty's `BeforeWith` opcode location, CPython's call-site frame.
# The shim's `__enter__` frame inside `test_fixtures.py` is filtered by
# the traceback runner (see `scripts/run_traceback.py`) so the visible
# output matches the Monty side, which has no equivalent Python-level
# frame for the synthetic context manager.
with _test_cm('raise_on_enter', 'fail-enter'):
    raise RuntimeError('body should not run')
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_enter_raises_traceback.py", line 7, in <module>
    with _test_cm('raise_on_enter', 'fail-enter'):
         ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: fail-enter
"""
