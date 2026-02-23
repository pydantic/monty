# filter() with non-iterable second argument
filter(None, 42)

"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__filter_not_iterable.py", line 2, in <module>
    filter(None, 42)
    ~~~~~~~~~~~~~~~~
TypeError: 'int' object is not iterable
"""
