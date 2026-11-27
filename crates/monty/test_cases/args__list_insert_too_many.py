x = []
x.insert(1, 2, 3)
"""
TRACEBACK:
Traceback (most recent call last):
  File "args__list_insert_too_many.py", line 2, in <module>
    x.insert(1, 2, 3)
    ~~~~~~~~~~~~~~~~~
[monty] TypeError: list.insert() takes exactly 2 arguments (3 given)
[cpython] TypeError: insert expected 2 arguments, got 3
"""
