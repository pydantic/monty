# A missing positional argument to a plain signature: the traceback shows the
# calling frames, not the `def` line.
def f(a, b):
    return a + b


def g():
    f(1)


g()
"""
TRACEBACK:
Traceback (most recent call last):
  File "function__err_missing_simple.py", line 11, in <module>
    g()
    ~~~
  File "function__err_missing_simple.py", line 8, in g
    f(1)
    ~~~~
TypeError: f() missing 1 required positional argument: 'b'
"""
