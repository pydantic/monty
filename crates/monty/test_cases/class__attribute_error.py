# Accessing an attribute an instance does not have raises AttributeError, with a
# traceback whose frames, line numbers and caret markers match CPython.


class Empty:
    pass


e = Empty()
e.missing
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__attribute_error.py", line 10, in <module>
    e.missing
AttributeError: 'Empty' object has no attribute 'missing'
"""
