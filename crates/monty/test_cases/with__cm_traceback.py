# When `__exit__` returns None (the default passthrough behavior), an
# exception raised inside the `with` body propagates with its original
# traceback intact — no `__exit__` frame added, no "During handling..."
# chained context. This is the case that should match byte-for-byte
# between Monty and CPython, because the synthetic `_test_cm` shim's
# `__exit__` just returns and never sees the exception.
#
# The Monty-side `WithExceptStart` opcode call into `py_exit` is the
# matching code path; if it ever started inadvertently rewriting the
# in-flight exception (e.g. by clearing the original traceback) this
# test would catch the divergence.
with _test_cm() as cm:
    raise ValueError('inside passthrough')
"""
TRACEBACK:
Traceback (most recent call last):
  File "with__cm_traceback.py", line 13, in <module>
    raise ValueError('inside passthrough')
ValueError: inside passthrough
"""
