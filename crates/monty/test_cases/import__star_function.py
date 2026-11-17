# `from m import *` binds names unknown until runtime, so it is only allowed at module level.
def f():
    from math import *


"""
TRACEBACK:
Traceback (most recent call last):
  File "import__star_function.py", line 3
    from math import *
                     ~
SyntaxError: import * only allowed at module level
"""
