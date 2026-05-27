# Exception raised by the context expression itself (the `_test_cm(...)`
# call), before `__enter__` is ever invoked. The traceback points at the
# `with` line because that's where the call expression lives; neither
# `BeforeWith` (Monty) nor `__enter__` (CPython) has run yet.
with _test_cm('not-a-behavior'):
    raise RuntimeError('body should not run')
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_context_expr_raises_traceback.py", line 5, in <module>
    with _test_cm('not-a-behavior'):
         ~~~~~~~~~~~~~~~~~~~~~~~~~~
TypeError: _test_cm() unknown behavior 'not-a-behavior'
"""
