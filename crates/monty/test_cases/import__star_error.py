# /// monty
# xfail = ['cpython']
# ///
from sys import *

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__star_error.py", line 4, in <module>
    from sys import *
    ~~~~~~~~~~~~~~~~~
NotImplementedError: Wildcard imports (`from ... import *`) are not supported
"""
