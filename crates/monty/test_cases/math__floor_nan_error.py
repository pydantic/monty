import math

math.floor(float('nan'))
"""
TRACEBACK:
Traceback (most recent call last):
  File "math__floor_nan_error.py", line 3, in <module>
    math.floor(float('nan'))
    ~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: cannot convert float NaN to integer
"""
