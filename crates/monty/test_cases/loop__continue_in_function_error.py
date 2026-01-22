def foo():
    continue


foo()
"""
TRACEBACK:
Traceback (most recent call last):
  File "loop__continue_in_function_error.py", line 2
    continue
    ~~~~~~~~
SyntaxError: 'continue' not properly in loop
"""
