import math

math.sin(math.inf)
"""
TRACEBACK:
Traceback (most recent call last):
  File "math__sin_inf_error.py", line 3, in <module>
    math.sin(math.inf)
    ~~~~~~~~~~~~~~~~~~
ValueError: expected a finite input, got inf
"""
