x = []
x.append(1, 2)
"""
TRACEBACK:
Traceback (most recent call last):
  File "args__list_append_too_many.py", line 2, in <module>
    x.append(1, 2)
    ~~~~~~~~~~~~~~
[monty] TypeError: list.append() takes exactly 1 argument (2 given)
[cpython] TypeError: list.append() takes exactly one argument (2 given)
"""
