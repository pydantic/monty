t = (1, 2, 3)
del t[0]
"""
TRACEBACK:
Traceback (most recent call last):
  File "del__type_error.py", line 2, in <module>
    del t[0]
        ~~~~
TypeError: 'tuple' object does not support item deletion
"""
