# `with` on a value without `__exit__` raises CPython's specific TypeError
# (not a generic AttributeError), even though the bytecode check itself
# fails on `__enter__` first. We surface CPython's text so user-facing
# diagnostics match.
with 5:
    assert False, 'body should never execute'
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__not_context_manager.py", line 5, in <module>
    with 5:
         ~
TypeError: 'int' object does not support the context manager protocol (missed __exit__ method)
"""
