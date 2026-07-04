# Nested `with` blocks with a passthrough class cm at each level: an
# exception raised in the innermost body propagates through both
# `__exit__` calls (both return None) and surfaces with the body's frame
# intact. Verifies that `WithExceptStart` doesn't perturb the propagating
# exception's traceback when `__exit__` is a no-op cleanup.
class CM:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        return None


with CM() as outer:
    with CM() as inner:
        raise ValueError('from inner body')
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_nested_body_raises_traceback.py", line 16, in <module>
    raise ValueError('from inner body')
ValueError: from inner body
"""
