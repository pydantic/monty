x = []
x.insert(1)
"""
TRACEBACK:
Traceback (most recent call last):
  File "args__list_insert_too_few.py", line 2, in <module>
    x.insert(1)
    ~~~~~~~~~~~
[monty] TypeError: list.insert() takes exactly 2 arguments (1 given)
[cpython] TypeError: insert expected 2 arguments, got 1
"""
