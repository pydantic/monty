def reciprocal(z):
    return z**-1


reciprocal(0j)
"""
TRACEBACK:
Traceback (most recent call last):
  File "complex__error_zero_power.py", line 5, in <module>
    reciprocal(0j)
    ~~~~~~~~~~~~~~
  File "complex__error_zero_power.py", line 2, in reciprocal
    return z**-1
           ~~~~~
ZeroDivisionError: zero to a negative or complex power
"""
