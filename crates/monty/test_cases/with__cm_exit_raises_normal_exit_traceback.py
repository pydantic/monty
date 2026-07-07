# `__exit__` raises on the normal-exit path (the body completed cleanly,
# but the cleanup itself fails). `__exit__` is a real frame on both sides,
# so it appears in the traceback under the `with` line.
class CM:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        raise ValueError('fail-exit')


with CM():
    pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_exit_raises_normal_exit_traceback.py", line 12, in <module>
    with CM():
         ~~~~
  File "with__cm_exit_raises_normal_exit_traceback.py", line 9, in __exit__
    raise ValueError('fail-exit')
ValueError: fail-exit
"""
