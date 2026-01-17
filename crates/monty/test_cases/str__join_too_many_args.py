','.join(['a'], ['b'])
"""
TRACEBACK:
Traceback (most recent call last):
  File "str__join_too_many_args.py", line 1, in <module>
    ','.join(['a'], ['b'])
    ~~~~~~~~~~~~~~~~~~~~~~
TypeError: str.join() takes exactly one argument (2 given)
"""
