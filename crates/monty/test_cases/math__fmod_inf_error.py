import math

math.fmod(math.inf, 3)
"""
TRACEBACK:
Traceback (most recent call last):
  File "math__fmod_inf_error.py", line 3, in <module>
    math.fmod(math.inf, 3)
    ~~~~~~~~~~~~~~~~~~~~~~
ValueError: math domain error
"""
