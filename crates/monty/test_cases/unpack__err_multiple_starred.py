# `UnpackEx` encodes one starred slot as "n before, m after", so a second star
# has nowhere to go and the assignment is rejected, as CPython rejects it.
a, *b, *c = [1, 2, 3, 4]
"""
TRACEBACK:
Traceback (most recent call last):
  File "unpack__err_multiple_starred.py", line 3
    a, *b, *c = [1, 2, 3, 4]
    ~~~~~~~~~
SyntaxError: multiple starred expressions in assignment
"""
