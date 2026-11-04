# eval() strips the leading tab, and the error's line counts from the start of the source
eval('\t\n[1,\n 1 / 0]')
"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__eval_traceback.py", line 2, in <module>
    eval('\t\n[1,\n 1 / 0]')
    ~~~~~~~~~~~~~~~~~~~~~~~~
  File "<string>", line 3, in <module>
ZeroDivisionError: division by zero
"""
