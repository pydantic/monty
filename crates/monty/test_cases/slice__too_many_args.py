# /// monty
# xfail = ["cpython"]  # Monty reports the declared parameter names and arity.
# ///
slice(1, 2, 3, 4)
"""
TRACEBACK:
Traceback (most recent call last):
  File "slice__too_many_args.py", line 4, in <module>
    slice(1, 2, 3, 4)
    ~~~~~~~~~~~~~~~~~
TypeError: slice() takes at most 3 arguments (4 given)
"""
