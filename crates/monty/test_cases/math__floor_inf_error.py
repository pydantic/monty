import math

math.floor(float('inf'))
"""
TRACEBACK:
Traceback (most recent call last):
  File "math__floor_inf_error.py", line 3, in <module>
    math.floor(float('inf'))
    ~~~~~~~~~~~~~~~~~~~~~~~~
OverflowError: cannot convert float infinity to integer
"""
