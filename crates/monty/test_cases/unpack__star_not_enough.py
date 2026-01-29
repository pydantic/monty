a, *b, c = [1]
"""
TRACEBACK:
Traceback (most recent call last):
  File "unpack__star_not_enough.py", line 1, in <module>
    a, *b, c = [1]
    ~~~~~~~~
ValueError: not enough values to unpack (expected at least 2, got 1)
"""
