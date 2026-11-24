x = {}
x.pop(1, 2, 3)
"""
TRACEBACK:
Traceback (most recent call last):
  File "args__dict_pop_too_many.py", line 2, in <module>
    x.pop(1, 2, 3)
    ~~~~~~~~~~~~~~
[monty] TypeError: pop() takes at most 2 arguments (3 given)
[cpython] TypeError: pop expected at most 2 arguments, got 3
"""
