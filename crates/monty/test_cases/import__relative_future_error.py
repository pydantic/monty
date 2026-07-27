# `from __future__ import ...` is a compiler directive Monty accepts as a no-op,
# but only in its absolute form: a relative import of a module that happens to
# share the name is an ordinary import, and there is no package system.
from .__future__ import annotations

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__relative_future_error.py", line 4, in <module>
    from .__future__ import annotations
ImportError: attempted relative import with no known parent package
"""
