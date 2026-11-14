slice(0, 5, 0).indices(10)
"""
TRACEBACK:
Traceback (most recent call last):
  File "slice__indices_zero_step.py", line 1, in <module>
    slice(0, 5, 0).indices(10)
    ~~~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: slice step cannot be zero
"""
