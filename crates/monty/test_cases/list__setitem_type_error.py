lst = [1, 2, 3]
lst['key'] = 'value'
"""
TRACEBACK:
Traceback (most recent call last):
  File "list__setitem_type_error.py", line 2, in <module>
    lst['key'] = 'value'
    ~~~~~~~~~~
TypeError: list indices must be integers or slices, not str
"""
