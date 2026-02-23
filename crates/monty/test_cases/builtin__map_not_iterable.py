# map() with non-iterable second argument
map(abs, 42)

"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__map_not_iterable.py", line 2, in <module>
    map(abs, 42)
    ~~~~~~~~~~~~
TypeError: 'int' object is not iterable
"""
