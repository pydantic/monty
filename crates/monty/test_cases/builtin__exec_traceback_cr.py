# a bare `\r` ends a line like `\n` and `\r\n` do, so the traceback counts it
exec('x = 1\ry = 2\r\n1 / 0')
"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__exec_traceback_cr.py", line 2, in <module>
    exec('x = 1\ry = 2\r\n1 / 0')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
  File "<string>", line 3, in <module>
ZeroDivisionError: division by zero
"""
