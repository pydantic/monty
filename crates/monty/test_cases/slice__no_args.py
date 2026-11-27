# /// monty
# xfail = ["cpython"]  # Monty reports the declared parameter names and arity.
# ///
slice()
"""
TRACEBACK:
Traceback (most recent call last):
  File "slice__no_args.py", line 4, in <module>
    slice()
    ~~~~~~~
TypeError: slice() missing 1 required positional argument: 'start'
"""
