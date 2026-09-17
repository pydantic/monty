Record = tuple[int, int]
Bad = int[str]
"""
TRACEBACK:
Traceback (most recent call last):
  File "generic_alias__not_subscriptable.py", line 2, in <module>
    Bad = int[str]
          ~~~~~~~~
TypeError: type 'int' is not subscriptable
"""
