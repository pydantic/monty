x = {}
x.get(1, 2, 3)
"""
TRACEBACK:
Traceback (most recent call last):
  File "args__dict_get_too_many.py", line 2, in <module>
    x.get(1, 2, 3)
    ~~~~~~~~~~~~~~
[monty] TypeError: get() takes at most 2 arguments (3 given)
[cpython] TypeError: get expected at most 2 arguments, got 3
"""
