# Test that bytes.endswith with str raises TypeError
b'hello'.endswith('o')
"""
TRACEBACK:
Traceback (most recent call last):
  File "bytes__endswith_str_error.py", line 2, in <module>
    b'hello'.endswith('o')
    ~~~~~~~~~~~~~~~~~~~~~~
TypeError: endswith first arg must be bytes or a tuple of bytes, not str
"""
