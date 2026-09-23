# eval skips leading spaces and tabs but keeps newlines, so the line counts them
eval(' \n\n1 + (1 / 0)')
"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__eval_traceback_line.py", line 2, in <module>
    eval(' \n\n1 + (1 / 0)')
    ~~~~~~~~~~~~~~~~~~~~~~~~
  File "<string>", line 3, in <module>
ZeroDivisionError: division by zero
"""
