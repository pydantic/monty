import math

math.ldexp(1.0, 1075)
"""
TRACEBACK:
Traceback (most recent call last):
  File "math__ldexp_overflow_error.py", line 3, in <module>
    math.ldexp(1.0, 1075)
    ~~~~~~~~~~~~~~~~~~~~~
OverflowError: math range error
"""
