# A `__future__` import after any other statement is a `SyntaxError`, as in CPython.
x = 1
from __future__ import annotations

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__future_late.py", line 3
    from __future__ import annotations
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
SyntaxError: from __future__ imports must occur at the beginning of the file
"""
