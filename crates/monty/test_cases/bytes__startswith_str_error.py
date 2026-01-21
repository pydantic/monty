# Test that bytes.startswith with str raises TypeError
b'hello'.startswith('h')
"""
TRACEBACK:
Traceback (most recent call last):
  File "bytes__startswith_str_error.py", line 2, in <module>
    b'hello'.startswith('h')
    ~~~~~~~~~~~~~~~~~~~~~~~~
TypeError: startswith first arg must be bytes or a tuple of bytes, not str
"""
