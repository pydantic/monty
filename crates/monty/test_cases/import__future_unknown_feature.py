# `__future__` imports are accepted as no-ops, but only for real feature names,
# so an eventual change to one of them (see limitations/typing.md) stays visible
# only to code that asked for it.
from __future__ import teleportation

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__future_unknown_feature.py", line 4
    from __future__ import teleportation
                           ~~~~~~~~~~~~~
SyntaxError: future feature teleportation is not defined
"""
