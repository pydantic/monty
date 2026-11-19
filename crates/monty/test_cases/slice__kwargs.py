# /// monty
# xfail = ["cpython"]  # Monty reports the declared parameter names and arity.
# ///
slice(stop=5)
"""
TRACEBACK:
Traceback (most recent call last):
  File "slice__kwargs.py", line 4, in <module>
    slice(stop=5)
    ~~~~~~~~~~~~~
TypeError: slice() got some positional-only arguments passed as keyword arguments: 'stop'
"""
