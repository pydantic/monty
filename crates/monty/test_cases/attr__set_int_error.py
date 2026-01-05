x = 5
x.foo = 1
"""
TRACEBACK:
Traceback (most recent call last):
  File "attr__set_int_error.py", line 2, in <module>
    x.foo = 1
    ~~~~~
AttributeError: 'int' object has no attribute 'foo' and no __dict__ for setting new attributes
"""
