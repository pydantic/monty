# `__enter__` raises before the body runs: the body never executes and
# `__exit__` is never called. `__enter__` is a real frame on both sides,
# so it appears in the traceback under the `with` line.
class CM:
    def __enter__(self):
        raise ValueError('fail-enter')

    def __exit__(self, typ, val, tb):
        return None


with CM():
    raise RuntimeError('body should not run')
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_enter_raises_traceback.py", line 12, in <module>
    with CM():
         ~~~~
  File "with__cm_enter_raises_traceback.py", line 6, in __enter__
    raise ValueError('fail-enter')
ValueError: fail-enter
"""
