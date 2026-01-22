print('xxx', **{"foo'": 123})
"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__print_invalid_kwarg.py", line 1, in <module>
    print('xxx', **{"foo'": 123})
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
TypeError: print() got an unexpected keyword argument 'foo''
"""
