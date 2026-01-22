b'hello'[::0]
"""
TRACEBACK:
Traceback (most recent call last):
  File "slice__step_zero_bytes.py", line 1, in <module>
    b'hello'[::0]
    ~~~~~~~~~~~~~
ValueError: slice step cannot be zero
"""
