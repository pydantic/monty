class BrokenStrError(Exception):
    def __str__(self):
        raise RuntimeError('str failed')


raise BrokenStrError()
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__custom_exc_bad_str.py", line 6, in <module>
    raise BrokenStrError()
BrokenStrError: <exception str() failed>
"""
