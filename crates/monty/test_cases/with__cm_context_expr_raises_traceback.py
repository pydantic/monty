# Exception raised by the context expression itself (constructing the
# manager), before `__enter__` is ever invoked: neither `__enter__` nor
# `__exit__` runs and the `__init__` frame shows under the `with` line.
class CM:
    def __init__(self):
        raise TypeError('ctor boom')

    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        return None


with CM():
    raise RuntimeError('body should not run')
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_context_expr_raises_traceback.py", line 15, in <module>
    with CM():
         ~~~~
  File "with__cm_context_expr_raises_traceback.py", line 6, in __init__
    raise TypeError('ctor boom')
TypeError: ctor boom
"""
