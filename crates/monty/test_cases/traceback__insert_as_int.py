a = []
a.insert({1: 2}, 2)
"""
TRACEBACK:
Traceback (most recent call last):
  File "traceback__insert_as_int.py", line 2, in <module>
    a.insert({1: 2}, 2)
    ~~~~~~~~~~~~~~~~~~~
TypeError: 'dict' object cannot be interpreted as an integer
"""
