def g():
    raise ValueError('original')


def f():
    try:
        g()
    finally:
        pass


f()
"""
TRACEBACK:
Traceback (most recent call last):
  File "traceback__finally_reraise.py", line 12, in <module>
    f()
    ~~~
  File "traceback__finally_reraise.py", line 7, in f
    g()
    ~~~
  File "traceback__finally_reraise.py", line 2, in g
    raise ValueError('original')
ValueError: original
"""
