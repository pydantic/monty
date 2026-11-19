# A `__future__` import inside a body is rejected before its feature is checked,
# so it cannot change how the enclosing or later definitions are compiled.
"""Only a docstring may precede a `__future__` import."""

from __future__ import annotations


def f():
    from __future__ import teleportation


"""
TRACEBACK:
Traceback (most recent call last):
  File "import__future_nested.py", line 9
    from __future__ import teleportation
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
SyntaxError: from __future__ imports must occur at the beginning of the file
"""
