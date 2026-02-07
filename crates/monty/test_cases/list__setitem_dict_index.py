# Test using a dict as a list setitem index (should raise TypeError)
# This covers the code path where a non-LongInt Ref type is used as an index
lst = [1, 2, 3]
d = {'key': 'value'}
lst[d] = 42
"""
TRACEBACK:
Traceback (most recent call last):
  File "list__setitem_dict_index.py", line 5, in <module>
    lst[d] = 42
    ~~~~~~
TypeError: list indices must be integers or slices, not dict
"""
