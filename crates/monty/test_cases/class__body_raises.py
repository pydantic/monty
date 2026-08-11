def f():
    class Inner:
        y = 1 / 0


class Outer:
    a = 1
    b = 2
    c = 3
    d = f() + 0


"""
TRACEBACK:
Traceback (most recent call last):
  File "class__body_raises.py", line 6, in <module>
    class Outer:
    ...<3 lines>...
        d = f() + 0
  File "class__body_raises.py", line 10, in Outer
    d = f() + 0
        ~~~
  File "class__body_raises.py", line 2, in f
    class Inner:
        y = 1 / 0
  File "class__body_raises.py", line 3, in Inner
    y = 1 / 0
        ~~~~~
ZeroDivisionError: division by zero
"""
