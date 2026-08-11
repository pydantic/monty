# A passthrough class context manager: an exception raised inside the
# `with` body propagates with its original traceback intact — `__exit__`
# returns None, never raises, and so adds no frame of its own.
class CM:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        return None


with CM() as cm:
    raise ValueError('inside passthrough')
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_traceback.py", line 13, in <module>
    raise ValueError('inside passthrough')
ValueError: inside passthrough
"""
