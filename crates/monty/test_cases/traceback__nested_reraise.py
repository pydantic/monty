def g():
    raise ValueError('original')


def f():
    try:
        g()
    except ValueError as outer:
        try:
            raise TypeError('inner')
        except TypeError:
            pass
        try:
            raise
        except ValueError as reraised:
            assert reraised is outer
            raise


f()
"""
TRACEBACK:
Traceback (most recent call last):
  File "traceback__nested_reraise.py", line 20, in <module>
    f()
    ~~~
  File "traceback__nested_reraise.py", line 7, in f
    g()
    ~~~
  File "traceback__nested_reraise.py", line 2, in g
    raise ValueError('original')
ValueError: original
"""
