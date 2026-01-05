x = [1, 2, 3]
x.foo = 1
"""
TRACEBACK:
Traceback (most recent call last):
  File "attr__set_list_error.py", line 2, in <module>
    x.foo = 1
    ~~~~~
AttributeError: 'list' object has no attribute 'foo' and no __dict__ for setting new attributes
"""
