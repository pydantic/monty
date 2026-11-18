# Too many positional arguments to a plain signature: the traceback shows the
# calling frames, not the `def` line.
def f(a):
    return a


def g():
    f(1, 2)


g()
"""
TRACEBACK:
Traceback (most recent call last):
  File "function__err_too_many_simple.py", line 11, in <module>
    g()
    ~~~
  File "function__err_too_many_simple.py", line 8, in g
    f(1, 2)
    ~~~~~~~
TypeError: f() takes 1 positional argument but 2 were given
"""
